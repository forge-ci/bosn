//! Human-only localization notices, stored beside a run, outside its wire reply.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::checkout::Localized;

/// Localization facts, before matrix expansion. Repeated steps/jobs are rendered once.
#[derive(Default, Serialize, Deserialize)]
pub struct Notes {
    pub remote_only: Vec<(String, String)>,
    pub pages_stubbed: Vec<String>,
}

impl From<&Localized> for Notes {
    fn from(localized: &Localized) -> Self {
        Self {
            remote_only: localized.remote_only.clone(),
            pages_stubbed: localized.pages_stubbed.clone(),
        }
    }
}

impl Notes {
    /// Render policy notices without changing any JSON reply or daemon protocol.
    pub fn render(&self, run: &str, details: bool) -> String {
        let remote: BTreeMap<_, _> = self
            .remote_only
            .iter()
            .map(|(job, reason)| (job, reason))
            .collect();
        let stubbed: BTreeSet<_> = self.pages_stubbed.iter().collect();
        if remote.is_empty() && stubbed.is_empty() {
            return String::new();
        }
        if !details {
            return format!(
                "GATE-012: {} remote-only, {} stubbed locally (bosn ci show {run} --notes)",
                remote.len(),
                stubbed.len()
            );
        }
        remote.iter().map(|(job, reason)| format!(
            "job {job} runs only on GitHub (GATE-012): {reason}; it is reported remote_only, not run"
        )).chain(stubbed.iter().map(|job| format!(
            "job {job}: actions/configure-pages is stubbed locally with the repository's GitHub Pages URLs (GATE-012); the build runs"
        ))).collect::<Vec<_>>().join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_notices_produce_no_summary_or_details() {
        for details in [false, true] {
            assert_eq!(Notes::default().render("run", details), "");
        }
    }

    #[test]
    fn typed_notes_survive_reload_and_invalid_storage_is_reported() {
        let root = tempfile::tempdir().unwrap();
        let store = super::super::store::Store::open(root.path()).unwrap();
        assert_eq!(store.load_notes("old").unwrap().render("old", false), "");
        std::fs::create_dir_all(store.run_dir("run")).unwrap();
        let notes = Notes {
            remote_only: vec![("matrix-job".into(), "requires GitHub".into()); 2],
            pages_stubbed: vec!["pages".into(); 2],
        };
        store.save_notes("run", &notes).unwrap();
        let restored = store.load_notes("run").unwrap();
        assert_eq!(
            restored.render("run", false),
            "GATE-012: 1 remote-only, 1 stubbed locally (bosn ci show run --notes)"
        );
        assert_eq!(restored.render("run", true).lines().count(), 2);
        std::fs::write(store.run_dir("run").join("notes.json"), b"not JSON").unwrap();
        assert!(store.load_notes("run").is_err());
    }
}
