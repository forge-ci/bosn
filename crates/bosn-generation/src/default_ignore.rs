//! Built-in ignore rules for a manifest Dockerfile build in a workspace with
//! no ignore file (#657). Without them a Rust or Python checkout sends its
//! build output (`target/`, `.venv/`) and `.git/` as context and is refused
//! at the collector's limits. An explicit `.dockerignore` (or
//! `<Dockerfile>.dockerignore`) always wins; the defaults are never merged in.

use std::path::Path;

const COMMON: &str = ".git\n";
const RUST: &str = "target\n**/target\n";
const PYTHON: &str = ".venv\nvenv\n**/__pycache__\n**/*.pyc\n**/*.pyo\n.pytest_cache\n\
.mypy_cache\n.ruff_cache\n.tox\n**/*.egg-info\ndist\nbuild\n";

/// Which built-in set a workspace gets, decided from markers at its root.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DefaultIgnore {
    Rust,
    Python,
    RustPython,
}

impl DefaultIgnore {
    /// The set for `root`, or `None` when it is neither a Rust nor a Python
    /// workspace (the context is then collected exactly as before).
    pub fn detect(root: &Path) -> Option<Self> {
        let rust = root.join("Cargo.toml").is_file() || root.join("target").is_dir();
        let python = ["pyproject.toml", "setup.py", "uv.lock"]
            .iter()
            .any(|name| root.join(name).is_file())
            || std::fs::read_dir(root).is_ok_and(|entries| {
                entries.flatten().any(|entry| {
                    entry.file_name().to_str().is_some_and(|name| {
                        name.starts_with("requirements") && name.ends_with(".txt")
                    })
                })
            });
        match (rust, python) {
            (true, true) => Some(Self::RustPython),
            (true, false) => Some(Self::Rust),
            (false, true) => Some(Self::Python),
            (false, false) => None,
        }
    }

    /// `.dockerignore` text for this set.
    pub fn rules(self) -> String {
        let (rust, python) = match self {
            Self::Rust => (RUST, ""),
            Self::Python => ("", PYTHON),
            Self::RustPython => (RUST, PYTHON),
        };
        format!("{COMMON}{rust}{python}")
    }

    /// The line a build prints when it applies this set.
    pub fn notice(self) -> &'static str {
        match self {
            Self::Rust => "no .dockerignore; using bosn's default Rust ignore set",
            Self::Python => "no .dockerignore; using bosn's default Python ignore set",
            Self::RustPython => "no .dockerignore; using bosn's default Rust and Python ignore set",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collector::{
        CollectorError, CollectorLimits, collect_context, collect_context_with_defaults,
    };
    use crate::{ContextEntry, ContextObservation};

    fn write(root: &Path, path: &str, bytes: &[u8]) {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }
    fn filler(root: &Path, dir: &str, n: usize) {
        for i in 0..n {
            write(root, &format!("{dir}/f{i}"), b"x");
        }
    }
    fn paths(context: &ContextObservation) -> Vec<String> {
        context
            .entries
            .iter()
            .map(|entry| match entry {
                ContextEntry::File { path, .. }
                | ContextEntry::Directory { path }
                | ContextEntry::Symlink { path, .. } => path.clone(),
            })
            .collect()
    }
    /// A checkout whose whole tree is the context (`COPY . /src`), with build
    /// output and history far beyond a 32-entry limit.
    fn workspace(marker: &str, output: &str) -> (tempfile::TempDir, CollectorLimits) {
        let root = tempfile::tempdir().unwrap();
        write(root.path(), "Dockerfile", b"FROM x\nCOPY . /src\n");
        write(root.path(), marker, b"");
        write(root.path(), "src/main", b"m");
        filler(root.path(), output, 60);
        filler(root.path(), ".git/objects", 60);
        let limits = CollectorLimits {
            max_entries: 32,
            ..CollectorLimits::default()
        };
        (root, limits)
    }

    #[test]
    fn a_rust_workspace_with_a_large_target_and_no_ignore_file_builds() {
        let (root, limits) = workspace("Cargo.toml", "target/debug/deps");
        // RED on the plain collector: the whole checkout is the context.
        assert!(matches!(
            collect_context(root.path(), Some("Dockerfile"), &limits),
            Err(CollectorError::EntryCount { .. })
        ));
        let (context, default) =
            collect_context_with_defaults(root.path(), "Dockerfile", &limits).unwrap();
        assert_eq!(default, Some(DefaultIgnore::Rust));
        let got = paths(&context);
        assert!(got.contains(&"src/main".into()) && got.contains(&"Cargo.toml".into()));
        assert!(
            !got.iter()
                .any(|p| p.starts_with("target") || p.starts_with(".git"))
        );
        assert!(
            default
                .unwrap()
                .notice()
                .contains("default Rust ignore set")
        );
    }

    #[test]
    fn a_python_workspace_with_a_venv_and_no_ignore_file_builds() {
        let (root, limits) = workspace("pyproject.toml", ".venv/lib/site-packages");
        write(root.path(), "pkg/__pycache__/m.cpython-313.pyc", b"c");
        let (context, default) =
            collect_context_with_defaults(root.path(), "Dockerfile", &limits).unwrap();
        assert_eq!(default, Some(DefaultIgnore::Python));
        let got = paths(&context);
        assert!(got.contains(&"src/main".into()));
        assert!(
            !got.iter()
                .any(|p| p.starts_with(".venv") || p.contains("__pycache__"))
        );
    }

    #[test]
    fn an_explicit_dockerignore_overrides_the_default_set() {
        let (root, mut limits) = workspace("Cargo.toml", "target/debug/deps");
        write(root.path(), ".dockerignore", b".git\n");
        limits.max_entries = 1_000;
        let (context, default) =
            collect_context_with_defaults(root.path(), "Dockerfile", &limits).unwrap();
        assert_eq!(
            default, None,
            "the explicit file is never merged with the defaults"
        );
        let got = paths(&context);
        assert!(got.iter().any(|p| p.starts_with("target/")));
        assert!(!got.iter().any(|p| p.starts_with(".git")));
    }

    #[test]
    fn an_over_limit_context_reports_the_specific_limit_hit() {
        let root = tempfile::tempdir().unwrap();
        write(root.path(), "Dockerfile", b"FROM x\nCOPY . /src\n");
        write(root.path(), "big", &[b'x'; 4096]);
        let limits = CollectorLimits {
            max_total_bytes: 2 * 1024 * 1024,
            max_file_bytes: 1024 * 1024,
            ..CollectorLimits::default()
        };
        write(root.path(), "big", &vec![b'x'; 1024 * 1024 + 1]);
        let error = collect_context_with_defaults(root.path(), "Dockerfile", &limits).unwrap_err();
        assert_eq!(
            error.refusal(&limits),
            "a context file exceeds 1 MiB (bosn's limit); add a .dockerignore"
        );
        for n in 0..3 {
            write(root.path(), &format!("part{n}"), &vec![b'x'; 900 * 1024]);
        }
        std::fs::remove_file(root.path().join("big")).unwrap();
        let error = collect_context_with_defaults(root.path(), "Dockerfile", &limits).unwrap_err();
        assert_eq!(
            error.refusal(&limits),
            "context exceeds 2 MiB total (bosn's limit); add a .dockerignore"
        );
        let entries = CollectorLimits {
            max_entries: 2,
            ..CollectorLimits::default()
        };
        let error = collect_context_with_defaults(root.path(), "Dockerfile", &entries).unwrap_err();
        assert!(
            error
                .refusal(&entries)
                .contains("more than 2 selected entries")
        );
    }

    #[test]
    fn detection_reads_root_markers_only() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(DefaultIgnore::detect(root.path()), None);
        write(root.path(), "requirements-dev.txt", b"");
        assert_eq!(
            DefaultIgnore::detect(root.path()),
            Some(DefaultIgnore::Python)
        );
        write(root.path(), "Cargo.toml", b"");
        assert_eq!(
            DefaultIgnore::detect(root.path()),
            Some(DefaultIgnore::RustPython)
        );
        let rules = DefaultIgnore::RustPython.rules();
        assert!(rules.contains("target") && rules.contains(".venv") && rules.contains(".git"));
    }
}
