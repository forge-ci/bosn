//! Bounded shared-cache accounting. File lengths and allocated blocks are
//! distinct samples; neither Docker layer sizes nor component totals are
//! added to the volume's measured total.

use super::{
    ActEngineCacheVolume, CONTROL_DEADLINE, DockerActBackend, ENGINE_CACHE, engine_image, owned,
};
use crate::ci::{CacheClass, CacheComponent, CacheUsage};

#[cfg(test)]
mod durability_tests;
mod footprint;
#[cfg(test)]
mod footprint_live;
mod footprint_program;
#[cfg(test)]
mod footprint_tests;
pub(super) mod helper;
pub(super) mod journal;
#[cfg(all(test, target_os = "linux"))]
mod native_accounting_tests;
#[cfg(test)]
mod reference_tests;
mod retry;
mod seed;
#[cfg(test)]
use reference_tests::{SCRIPT, parse};
pub use retry::HelperCleanupRetry;

const MAX_NAMESPACES: usize = 256;

struct Captured {
    text: String,
    diagnostic: String,
    exit_code: i32,
}

#[derive(Clone, Copy)]
enum Reader {
    Policy,
    Footprint,
}

impl Reader {
    fn memory(self) -> &'static str {
        match self {
            Self::Policy => "128m",
            Self::Footprint => "512m",
        }
    }
    fn tmpfs(self) -> &'static str {
        match self {
            Self::Policy => "/var/lib/docker",
            Self::Footprint => "/var/lib/docker:exec,size=64m",
        }
    }
    fn output_limit(self) -> usize {
        match self {
            Self::Policy => super::CONTROL_OUTPUT,
            Self::Footprint => 256 << 10,
        }
    }
}
const CLASSES: [CacheClass; 5] = [
    CacheClass::Tools,
    CacheClass::Images,
    CacheClass::Actions,
    CacheClass::Toolcache,
    CacheClass::Actcache,
];

/// Only Docker's explicit absence verdict can mean an absent cache. Engine
/// outages, permission failures and invalid responses remain errors.
pub(super) fn volume_present(ok: bool, stderr: &[u8]) -> Result<bool, String> {
    if ok {
        return Ok(true);
    }
    let detail = String::from_utf8_lossy(stderr);
    if detail.to_ascii_lowercase().contains("no such volume") {
        return Ok(false);
    }
    Err(format!("cache volume inspection failed: {}", detail.trim()))
}

impl DockerActBackend {
    pub(super) async fn measure_cache(&self, volume: &str) -> Result<CacheUsage, String> {
        self.measure_cache_tracked(volume, None).await
    }
    pub(super) async fn measure_cache_tracked(
        &self,
        volume: &str,
        context: Option<(&crate::RegistryActor, &str)>,
    ) -> Result<CacheUsage, String> {
        if !self.volume_exists(volume).await? {
            return Ok(CacheUsage {
                volume: volume.into(),
                ..CacheUsage::default()
            });
        }
        self.verify_measured_volume(volume).await?;
        let architecture = self
            .checked(
                "accounting image architecture",
                owned(&[
                    "image",
                    "inspect",
                    "--format",
                    "{{.Architecture}}",
                    &engine_image(),
                ]),
                CONTROL_DEADLINE,
            )
            .await?;
        let act = super::act_artifact(&architecture)
            .ok_or("unsupported accounting image architecture")?;
        let mut output = self
            .read_cache_with_profile(
                volume,
                context,
                &footprint_program::program(act),
                Reader::Footprint,
            )
            .await?;
        if output.exit_code == footprint_program::ARCHIVE_UNAVAILABLE {
            // Accounting must not wait for CI preparation to cache the reader.
            self.seed_act_archive(volume, act).await?;
            output = self
                .read_cache_with_profile(
                    volume,
                    context,
                    &footprint_program::program(act),
                    Reader::Footprint,
                )
                .await?;
            if output.exit_code == footprint_program::ARCHIVE_UNAVAILABLE {
                return Err("pinned act archive is still unavailable after seeding".into());
            }
        }
        footprint::parse(volume, &output.text, output.exit_code).map_err(|error| {
            if output.exit_code == 0 {
                error
            } else {
                format!(
                    "{error}; accounting process exited {}: {}",
                    output.exit_code, output.diagnostic
                )
            }
        })
    }

    /// Fixed trusted readers share the same read-only helper and journal.
    pub(in crate::ci::engine) async fn read_cache_tracked(
        &self,
        volume: &str,
        context: Option<(&crate::RegistryActor, &str)>,
        script: &'static str,
    ) -> Result<String, String> {
        self.read_cache_with_profile(volume, context, script, Reader::Policy)
            .await
            .map(|capture| capture.text.trim().to_owned())
    }

    async fn read_cache_with_profile(
        &self,
        volume: &str,
        context: Option<(&crate::RegistryActor, &str)>,
        script: &str,
        reader: Reader,
    ) -> Result<Captured, String> {
        self.verify_measured_volume(volume).await?;
        let image = engine_image();
        let mount = format!("type=volume,source={volume},target=/cache,readonly");
        // Name and label the helper before create so a lost acknowledgement
        // can be reconciled without removing an unverified container by name.
        let mut identity = helper::Identity::new().await?;
        let _active = journal::ActiveHelper::claim(self, &identity.nonce);
        let tracker = if let Some((registry, owner)) = context {
            let intent = identity.track(owner, volume)?;
            let tracker = journal::Tracker::new(registry, &identity.nonce);
            tracker.begin(intent).await?;
            Some(tracker)
        } else {
            None
        };
        let mut create_args = owned(&[
            "create",
            "--rm",
            bosn_engine::LOW_CPU_PRIORITY_ARG,
            "--name",
            &identity.name,
            "--label",
            &identity.label(),
            "--pull",
            "never",
            "--network",
            "none",
            "--read-only",
            "--cap-drop",
            "ALL",
            "--memory",
            reader.memory(),
            "--cpus",
            "1",
            "--tmpfs",
            reader.tmpfs(),
            "--mount",
            &mount,
            "--entrypoint",
            "sh",
            &image,
            "-c",
            script,
        ]);
        create_args.splice(2..2, identity.ownership_args());
        let created = self
            .checked("cache measure create", create_args, CONTROL_DEADLINE)
            .await;
        let id = match created {
            Ok(id) if helper::valid_id(&id) => id,
            result => {
                let error = result.err().unwrap_or_else(|| {
                    "cache measure create returned no immutable container ID".into()
                });
                let recovery = self
                    .recover_measurement(&identity, volume, tracker.as_ref())
                    .await;
                return Err(match recovery {
                    Ok(()) => format!("{error}; measurement helper {} recovered", identity.name),
                    Err(recovery) => format!("{error}; {recovery}"),
                });
            }
        };
        if let Some(tracker) = &tracker {
            tracker
                .register(&id)
                .await
                .map_err(|error| format!("helper {} needs cleanup: {error}", identity.name))?;
        }
        // Mounting a named volume can recreate it if a concurrent clear won
        // the race. Repeat its ownership check before starting the helper.
        let measured = match self.verify_measured_volume(volume).await {
            Ok(()) => self.read_measurement(&id, reader).await,
            Err(error) => Err(error),
        };
        self.remove_measurement(&id).await?;
        if let Some(tracker) = &tracker {
            tracker.finish(&id).await?;
        }
        measured
    }

    async fn read_measurement(&self, id: &str, reader: Reader) -> Result<Captured, String> {
        let output = self
            .run_bounded(
                owned(&["start", "--attach", id]),
                super::RunOptions::bounded(CONTROL_DEADLINE, reader.output_limit()),
            )
            .await?;
        let accepted_exit = output.ok()
            || matches!(reader, Reader::Footprint)
                && matches!(output.exit_code, 1 | footprint_program::ARCHIVE_UNAVAILABLE);
        if !accepted_exit {
            return Err(format!(
                "cache measure failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        if output.stdout.len() > reader.output_limit() {
            return Err("raw cache measurement exceeds bound".into());
        }
        let text = String::from_utf8(output.stdout)
            .map_err(|error| format!("cache measurement is not UTF-8: {error}"))?;
        Ok(Captured {
            diagnostic: String::from_utf8_lossy(&output.stderr)
                .trim()
                .chars()
                .take(512)
                .collect(),
            text,
            exit_code: output.exit_code,
        })
    }

    pub(in crate::ci::engine) async fn remove_measurement(&self, id: &str) -> Result<(), String> {
        let result = self
            .run(owned(&["rm", "-f", "-v", id]), CONTROL_DEADLINE)
            .await
            .map_err(|error| format!("{error}; measurement container {id} needs cleanup"))?;
        let detail = String::from_utf8_lossy(&result.stderr);
        if result.ok() {
            return self.confirm_measurement_absent(id).await;
        }
        if result.reports_missing()
            && (detail.to_ascii_lowercase().contains("no such container:")
                || detail.to_ascii_lowercase().contains("no such object:"))
        {
            return Ok(());
        }
        Err(format!(
            "cache measure cleanup failed: {detail}; measurement container {id} needs cleanup"
        ))
    }

    pub(in crate::ci::engine) async fn verify_measured_volume(
        &self,
        volume: &str,
    ) -> Result<(), String> {
        let document = self
            .checked(
                "cache ownership",
                owned(&["volume", "inspect", volume]),
                CONTROL_DEADLINE,
            )
            .await?;
        crate::act_engine::verify_cache_volume(
            document.as_bytes(),
            &ActEngineCacheVolume {
                name: volume.into(),
                target: ENGINE_CACHE.into(),
            },
        )
        .map_err(|error| error.to_string())
    }
}
