//! Coordinate generic machine draining around act2-owned archive enrollment.
use super::{Lease, SharedEngine, WAIT_TICK};
use crate::ci::{
    cache_cohort::CacheRoute,
    engine::{
        ArchiveEnrollmentPhase as Phase, ArchiveEnrollmentRequest,
        ArchiveEnrollmentStatus as Status,
    },
};
use kernal_api::async_engine::{self, CancellationToken};
use std::time::{Duration, Instant};

/// How long a run waits for archive enrollment contention before it ends with an actionable
/// error (#735). Contention is a busy maintenance lock or migration lease, or a retryable act2
/// report: normally seconds. Without a bound, back-to-back runs from other sessions starved one
/// run for over an hour while it held its admission slot and stranded the queue behind it.
#[cfg(not(test))]
const ENROLLMENT_WAIT: Duration = Duration::from_secs(15 * 60);
#[cfg(test)]
const ENROLLMENT_WAIT: Duration = Duration::from_secs(3);

impl SharedEngine {
    pub(crate) async fn prepare_archives(
        &self,
        lease: &Lease,
        route: &CacheRoute,
        cancellation: &CancellationToken,
        note: &mut (dyn FnMut(&str) + Send),
    ) -> Result<(), String> {
        let CacheRoute::Cohort { namespace, policy } = route else {
            return Ok(());
        };
        let mut phase = Phase::Require;
        let mut said_wait = false;
        let waiting_since = Instant::now();
        loop {
            self.verify(lease).await?;
            let request = ArchiveEnrollmentRequest {
                engine: &lease.engine_id,
                namespace,
                policy: *policy,
                phase,
            };
            let status =
                async_engine::cancellable(cancellation, self.backend.archive_enrollment(request))
                    .await
                    .map_err(|_| "archive enrollment cancelled")??;
            match (phase, status) {
                (_, Status::Ready) => return self.verify(lease).await,
                (Phase::Require, Status::Refused(_)) => {
                    // Fence itself rechecks frozen history. A missing initial
                    // record may enroll; corrupted or changed history refuses.
                    phase = Phase::Fence;
                    said_wait = false;
                }
                (Phase::Fence, Status::Fenced) => {
                    self.wait_archive_sources(lease, cancellation, note).await?;
                    phase = Phase::Complete;
                    said_wait = false;
                }
                (_, Status::Retry) => {
                    if waiting_since.elapsed() >= ENROLLMENT_WAIT {
                        return Err(format!(
                            "archive enrollment still contended after {} s (cache maintenance \
                             or another run holds the shared cache cohort); retry the run",
                            waiting_since.elapsed().as_secs()
                        ));
                    }
                    if !said_wait {
                        note("waiting for archive enrollment coordination");
                        said_wait = true;
                    }
                    archive_wait(cancellation).await?;
                }
                (_, Status::Refused(error)) => {
                    return Err(format!("archive enrollment refused: {error}"));
                }
                _ => return Err("archive enrollment acknowledged the wrong phase".into()),
            }
        }
    }

    async fn wait_archive_sources(
        &self,
        lease: &Lease,
        cancellation: &CancellationToken,
        note: &mut (dyn FnMut(&str) + Send),
    ) -> Result<(), String> {
        let mut said_wait = false;
        loop {
            self.verify(lease).await?;
            let busy = self
                .drain_legacy(self.machine()?, Some(&lease.engine_name))
                .await?;
            if busy.is_empty() {
                return Ok(());
            }
            if !said_wait {
                note("archive source fenced; waiting for older engines to drain");
                said_wait = true;
            }
            archive_wait(cancellation).await?;
        }
    }
}

async fn archive_wait(cancellation: &CancellationToken) -> Result<(), String> {
    async_engine::cancellable(cancellation, async_engine::sleep(WAIT_TICK))
        .await
        .map_err(|_| "archive enrollment cancelled".into())
}

#[cfg(test)]
mod tests;
