//! Admitting a run into `act serve`, healing admissions Bosn's slot table
//! no longer backs.
//!
//! The engine has two slot tables: Bosn's lease (`/run/bosn-slots`, which
//! picks the slot) and serve's admissions (which honour it). They diverge
//! when a run's slot lease is released but serve never closed its admission:
//! a `close` that failed or timed out (an act that did not stop within
//! serve's 30 s, a `docker exec` killed under memory pressure) is noted and
//! the slot is released anyway. Serve then holds the admission forever, and
//! the next run leased that slot number is refused with `409 Conflict: slot
//! N is taken` while nothing is running in it.
//!
//! On that refusal Bosn closes every *orphaned* admission, one whose run
//! holds no slot in Bosn's table, then admits once more. The order is what
//! keeps a live run safe: serve's runs are read **before** the slot table.
//! Every run is leased before it is admitted, so a run serve held at the
//! first read either still holds its lease at the second (and is kept) or
//! released it, which every Bosn does only after its run ended. A run
//! admitted between the reads is not in the first and is never touched.

use std::collections::BTreeSet;

use serde::Deserialize;

use super::{
    super::machine::{MachineEngine, Survey},
    CLEANUP_DEADLINE, CONTROL_DEADLINE, DockerActBackend,
    run_scope::{RunScope, close_run_script, status_script},
};

/// What Bosn reads of serve's health: the run IDs it holds admitted.
#[derive(Debug, Deserialize)]
struct Admissions {
    runs: Vec<String>,
}

/// Serve's refusal of a slot it still holds for some run.
fn slot_taken(error: &str) -> bool {
    error.contains("409 Conflict") && error.contains("is taken")
}

/// The admitted runs (read first) that no slot in `survey` (read after)
/// holds: nobody will close them, so they are closed here.
fn orphans(admitted: &[String], survey: &Survey) -> Vec<String> {
    let held: BTreeSet<&str> = survey.held.iter().map(|(_, h)| h.run.as_str()).collect();
    admitted
        .iter()
        .filter(|run| !held.contains(run.as_str()))
        .cloned()
        .collect()
}

impl DockerActBackend {
    /// Admit `scope`'s run into its slot; serve's reply on success.
    pub(super) async fn admit(&self, engine: &str, scope: &RunScope) -> Result<String, String> {
        match self.admit_once(engine, scope).await {
            Err(error) if slot_taken(&error) => {
                let closed = self.close_orphans(engine).await?;
                if closed.is_empty() {
                    return Err(error);
                }
                daemon_log!(
                    "bosn ci: closed {} orphaned act serve admission(s) ({}) before admitting run {}",
                    closed.len(),
                    closed.join(", "),
                    scope.label()
                );
                self.admit_once(engine, scope).await
            }
            other => other,
        }
    }

    async fn admit_once(&self, engine: &str, scope: &RunScope) -> Result<String, String> {
        self.checked(
            "act serve admission",
            Self::exec(engine, &scope.admit_script()),
            CONTROL_DEADLINE,
        )
        .await
    }

    /// Close every admission no slot lease backs; the runs closed.
    async fn close_orphans(&self, engine: &str) -> Result<Vec<String>, String> {
        let status = self
            .checked(
                "act serve status",
                Self::exec(engine, &status_script()),
                CONTROL_DEADLINE,
            )
            .await?;
        let admitted: Admissions =
            serde_json::from_str(&status).map_err(|error| format!("act serve status: {error}"))?;
        let survey = self.survey(engine).await?;
        let mut closed = Vec::new();
        for run in orphans(&admitted.runs, &survey) {
            // Serve's run IDs are canonical UUIDs; refuse anything else
            // rather than put it in a shell command.
            if RunScope::new(&run, 0, super::RunLimits::within_engine(0, 0, 0)).is_err() {
                continue;
            }
            self.checked(
                "act serve close (orphaned admission)",
                Self::exec(engine, &close_run_script(&run)),
                CLEANUP_DEADLINE,
            )
            .await?;
            closed.push(run);
        }
        Ok(closed)
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::machine::{Daemon, Holder};
    use super::*;

    const A: &str = "6fb17d1d-e6a2-4c5c-b9c1-e835e4c5c2e1";
    const B: &str = "0d6f8c2e-1111-4c5c-b9c1-e835e4c5c2e1";

    fn held(slot: u16, run: &str) -> (u16, Holder) {
        (
            slot,
            Holder {
                run: run.into(),
                registry: "r".into(),
                daemon: Daemon::current(),
            },
        )
    }

    #[test]
    fn serve_refusal_of_a_taken_slot_is_recognised() {
        assert!(slot_taken(
            "act serve admission failed: Error: 409 Conflict: slot 2 is taken"
        ));
        assert!(!slot_taken(
            "act serve admission failed: 409 Conflict: run is already admitted with other limits"
        ));
        assert!(!slot_taken(
            "act serve admission failed: 503: all 256 run slots are taken"
        ));
    }

    /// The bug: run A's close failed, its lease was released, serve still
    /// admits it. A is an orphan; B, which holds its lease, is not.
    #[test]
    fn an_admission_without_a_lease_is_orphaned_and_a_leased_one_is_kept() {
        let survey = Survey {
            held: vec![held(0, B)],
            ..Survey::default()
        };
        assert_eq!(orphans(&[A.into(), B.into()], &survey), [A]);
        assert!(orphans(&[B.into()], &survey).is_empty());
    }

    #[test]
    fn serve_status_is_parsed_for_its_runs() {
        let health = format!(
            r#"{{"protocol":1,"version":"0.2.89","max_runs":256,"runs":["{A}"],"docker_proxy":true}}"#
        );
        let parsed: Admissions = serde_json::from_str(&health).unwrap();
        assert_eq!(parsed.runs, [A]);
    }
}
