//! Suppress identical cleanup deferrals until the durable intent retires.
use std::collections::HashSet;

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub(super) enum Kind {
    Engine,
    Helper,
}

#[derive(Default)]
pub(super) struct Deferrals {
    seen: HashSet<(Kind, String, String)>,
}

impl Deferrals {
    pub(super) fn log_once(&mut self, kind: Kind, id: &str, reason: &str, log: impl FnOnce()) {
        if self.seen.insert((kind, id.into(), reason.into())) {
            log();
        }
    }

    pub(super) fn clear(&mut self, kind: Kind, id: &str) {
        self.seen
            .retain(|(stored_kind, stored_id, _)| *stored_kind != kind || stored_id != id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_passes_log_one_line_per_intent_and_reason() {
        let mut log = Deferrals::default();
        let mut lines = Vec::new();
        for _ in 0..141 {
            log.log_once(Kind::Helper, "helper-a", "No such container", || {
                lines.push("helper-a");
            });
            log.log_once(Kind::Engine, "run-a", "creation unresolved", || {
                lines.push("run-a");
            });
        }
        assert_eq!(lines, ["helper-a", "run-a"]);
        log.log_once(Kind::Helper, "helper-a", "daemon unavailable", || {
            lines.push("helper-a-new-reason");
        });
        log.log_once(Kind::Helper, "helper-b", "No such container", || {
            lines.push("helper-b");
        });
        log.clear(Kind::Helper, "helper-a");
        log.log_once(Kind::Helper, "helper-a", "No such container", || {
            lines.push("helper-a-after-removal");
        });
        assert_eq!(lines.len(), 5);
    }
}
