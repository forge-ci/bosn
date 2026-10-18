//! Walk-time pruning for the context collector.
//!
//! The collector's first pass lists names before the ignore policy selects
//! anything, and `max_entries` bounds that list. Without pruning, a
//! workspace's ignored build output (`target/`, `.venv/`, a repo-local Cargo
//! registry) exhausts the bound even when the selected context is a handful
//! of files. A directory is skipped only when skipping it cannot change the
//! selection or the Dockerfile's source checks:
//!
//! * the final ignore verdict for the directory is "ignored",
//! * no negation rule could match the directory or anything beneath it,
//! * no `COPY`/`ADD` source could match the directory or anything beneath it
//!   (so a missing-source check sees the same names), and
//! * it is not an ancestor of the Dockerfile or the ignore file in use.

use crate::dockerfile::{
    DockerfileError, Rule, glob, ignore_rules, ignored, local_sources, source_matches,
};

pub(crate) struct WalkPruner {
    rules: Vec<Rule>,
    sources: Vec<Vec<String>>,
    source_patterns: Vec<String>,
    all: bool,
    protected: Vec<String>,
}

impl WalkPruner {
    pub(crate) fn new(
        dockerfile: &str,
        dockerfile_text: &[u8],
        ignore: Option<(&str, &[u8])>,
    ) -> Result<Self, DockerfileError> {
        let text = std::str::from_utf8(dockerfile_text)
            .map_err(|_| DockerfileError::InvalidUtf8(dockerfile.into()))?;
        let rules = match ignore {
            Some((_, bytes)) => std::str::from_utf8(bytes)
                .map(ignore_rules)
                .map_err(|_| DockerfileError::InvalidUtf8(".dockerignore".into()))?,
            None => Vec::new(),
        };
        let (source_patterns, all) = local_sources(text)?;
        let sources = source_patterns
            .iter()
            .map(|s| components(s))
            .filter(|c| !c.is_empty())
            .collect();
        let mut protected = vec![dockerfile.to_owned()];
        if let Some((name, _)) = ignore {
            protected.push(name.to_owned());
        }
        Ok(Self {
            rules,
            sources,
            source_patterns,
            all,
            protected,
        })
    }

    pub(crate) fn source_count(&self) -> usize {
        self.source_patterns.len()
    }

    /// Retain selected entries and one witness for each source, including an
    /// ignored source. The latter preserves `select_context`'s missing-source
    /// validation without storing an ignored subtree.
    pub(crate) fn classify(
        &self,
        path: &str,
        seen_sources: &mut [bool],
    ) -> Result<(bool, bool), DockerfileError> {
        let mut witness = false;
        let mut matches_source = false;
        for (source, seen) in self.source_patterns.iter().zip(seen_sources) {
            if source_matches(source, path)? {
                matches_source = true;
                if !*seen {
                    *seen = true;
                    witness = true;
                }
            }
        }
        let selected = self.protected.iter().any(|p| p == path)
            || ((self.all || matches_source) && !ignored(&self.rules, path)?);
        Ok((selected, selected || witness))
    }

    /// True when the walk may skip everything beneath directory `dir`.
    pub(crate) fn prunes(&self, dir: &str) -> Result<bool, DockerfileError> {
        if self.rules.is_empty() || !ignored(&self.rules, dir)? {
            return Ok(false);
        }
        let prefix = format!("{dir}/");
        if self.protected.iter().any(|p| p.starts_with(&prefix)) {
            return Ok(false);
        }
        let dir: Vec<String> = components(dir);
        for rule in self.rules.iter().filter(|r| r.negated) {
            if reaches(&components(&rule.pattern), &dir)? {
                return Ok(false);
            }
        }
        for source in &self.sources {
            if reaches(source, &dir)? {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

fn components(pattern: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for part in pattern.replace('\\', "/").split('/') {
        match part {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            part => out.push(part.to_owned()),
        }
    }
    out
}

/// Conservative: could `pattern` match `dir`, an ancestor of it, or a path
/// beneath it?
fn reaches(pattern: &[String], dir: &[String]) -> Result<bool, DockerfileError> {
    for (i, part) in dir.iter().enumerate() {
        let Some(p) = pattern.get(i) else {
            return Ok(true);
        };
        if p.contains("**") {
            return Ok(true);
        }
        if !glob(p, part)? {
            return Ok(false);
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pruner(ignore: &str, dockerfile: &str) -> WalkPruner {
        WalkPruner::new(
            "docker/Dockerfile",
            dockerfile.as_bytes(),
            Some((".dockerignore", ignore.as_bytes())),
        )
        .unwrap()
    }

    #[test]
    fn prunes_plainly_ignored_directories() {
        let p = pruner(
            "target/\n**/target/\n**/.cargo/registry/\n",
            "FROM x\nCOPY src /s\n",
        );
        assert!(p.prunes("target").unwrap());
        assert!(p.prunes("dylints/a/target").unwrap());
        assert!(p.prunes("dylints/a/.cargo/registry").unwrap());
        assert!(!p.prunes("src").unwrap());
        assert!(!p.prunes("dylints").unwrap());
    }

    #[test]
    fn keeps_directories_a_negation_or_source_can_reach() {
        let p = pruner(
            "ignored/\n!ignored/sub/keep\nvendor/\n",
            "FROM x\nCOPY vendor/lib /v\n",
        );
        assert!(!p.prunes("ignored").unwrap());
        assert!(!p.prunes("ignored/sub").unwrap());
        assert!(!p.prunes("vendor").unwrap());
    }

    #[test]
    fn never_prunes_an_ancestor_of_the_dockerfile() {
        let p = pruner("docker/\n", "FROM x\n");
        assert!(!p.prunes("docker").unwrap());
    }

    #[test]
    fn no_ignore_file_never_prunes() {
        let p = WalkPruner::new("Dockerfile", b"FROM x\n", None).unwrap();
        assert!(!p.prunes("target").unwrap());
    }
}
