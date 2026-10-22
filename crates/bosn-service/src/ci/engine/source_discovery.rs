//! Fair bounded discovery by immutable source-name prefixes, not creation time.
use crate::ci::engine::{CONTROL_DEADLINE, DockerActBackend, owned, source_context};
use std::collections::VecDeque;

const PAGE: usize = 64;
const HEX: &str = "0123456789abcdef";

#[derive(Debug)]
pub(in crate::ci::engine) struct SourceCursor {
    prefixes: Vec<String>,
    pending: VecDeque<String>,
}

impl Default for SourceCursor {
    fn default() -> Self {
        Self {
            prefixes: vec![String::new()],
            pending: VecDeque::new(),
        }
    }
}

impl SourceCursor {
    #[cfg(test)]
    pub(super) fn for_context(owner: &str) -> Self {
        assert_eq!(owner.len(), 64);
        assert!(
            owner
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        );
        Self {
            prefixes: vec![owner.into()],
            pending: VecDeque::new(),
        }
    }

    pub(super) fn begin_pass(&mut self) {
        if self.prefixes.is_empty() && self.pending.is_empty() {
            self.prefixes.push(String::new());
        }
    }

    fn ingest(&mut self, prefix: &str, text: &str) -> Result<(), String> {
        if text.len() > (PAGE + 1) * 65 {
            return Err("source discovery page exceeds byte bound".into());
        }
        let mut ids: Vec<_> = text.split_whitespace().map(String::from).collect();
        if ids.len() > PAGE + 1
            || ids.iter().any(|id| {
                id.len() != 64
                    || !id
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
        {
            return Err("source discovery identity invalid".into());
        }
        ids.sort();
        if ids.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err("source discovery identities duplicate".into());
        }
        if ids.len() == PAGE + 1 {
            if prefix.len() == 64 || self.prefixes.len() + 16 > 1024 {
                return Err("source discovery partition cannot progress".into());
            }
            // Names contain a canonical 64-hex owner. Docker name uniqueness
            // bounds every complete leaf to one source, even for equal ages.
            self.prefixes
                .extend(HEX.chars().rev().map(|c| format!("{prefix}{c}")));
        } else {
            self.pending.extend(ids);
        }
        Ok(())
    }

    pub(super) async fn next(
        &mut self,
        backend: &DockerActBackend,
        queries: &mut usize,
    ) -> Result<Option<String>, String> {
        loop {
            if let Some(id) = self.pending.pop_front() {
                return Ok(Some(id));
            }
            if *queries == 0 {
                return Ok(None);
            }
            let Some(prefix) = self.prefixes.pop() else {
                return Ok(None);
            };
            *queries -= 1;
            let name = format!(
                "name=^/bosn-act-source-{prefix}[0-9a-f]{{{}}}$",
                64 - prefix.len()
            );
            let text = backend
                .checked(
                    "source context discovery",
                    owned(&[
                        "ps",
                        "-a",
                        "--no-trunc",
                        "--last",
                        "65",
                        "--filter",
                        &name,
                        "--filter",
                        &format!("label={}=v1", source_context::NAMESPACE),
                        "--format",
                        "{{.ID}}",
                    ]),
                    CONTROL_DEADLINE,
                )
                .await?;
            self.ingest(&prefix, &text)?;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dense_same_age_source_inventory_eventually_visits_every_identity() {
        let sources: Vec<_> = (0..5000)
            .map(|n| (format!("abcd{n:060x}"), format!("{n:064x}")))
            .collect();
        let mut cursor = SourceCursor::default();
        let mut seen = std::collections::BTreeSet::new();
        let mut queries = 0;
        while let Some(prefix) = cursor.prefixes.pop() {
            // Newest-first truncation is deliberately hostile to older IDs.
            let page: Vec<_> = sources
                .iter()
                .rev()
                .filter(|(owner, _)| owner.starts_with(&prefix))
                .take(65)
                .map(|(_, id)| id.as_str())
                .collect();
            cursor.ingest(&prefix, &page.join("\n")).unwrap();
            queries += 1;
            while let Some(id) = cursor.pending.pop_front() {
                assert!(seen.insert(id));
            }
            assert!(cursor.prefixes.len() <= 1024);
        }
        assert_eq!(seen.len(), sources.len());
        assert!(queries < 2000, "finite traversal of a dense common prefix");
        cursor.begin_pass();
        assert_eq!(cursor.prefixes, [String::new()]);
    }

    #[test]
    fn source_pages_refuse_truncated_ambiguous_and_unbounded_authority() {
        let mut cursor = SourceCursor::default();
        for text in [
            "bad".into(),
            "a".repeat(65),
            format!("{}\n{}", "a".repeat(64), "a".repeat(64)),
            " ".repeat(4291),
        ] {
            assert!(cursor.ingest("", &text).is_err());
        }
        let ids: Vec<_> = (0..65).map(|n| format!("{n:064x}")).collect();
        assert!(cursor.ingest(&"a".repeat(64), &ids.join("\n")).is_err());
    }

    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct LiveInventory {
        prefix: String,
        ids: Vec<String>,
    }

    #[test]
    #[ignore = "requires explicitly provisioned private Docker metadata containers"]
    fn actual_docker_source_discovery_visits_dense_equal_age_partition() {
        let path = std::env::var("BOSN_SOURCE_DISCOVERY_FIXTURE").unwrap();
        let fixture: LiveInventory =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(fixture.prefix, "f00dbabe");
        assert_eq!(fixture.ids.len(), 70);
        let runtime = kernal_api::async_engine::RuntimeBuilder::multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        runtime.run(async {
            let backend = DockerActBackend::default();
            let mut cursor = SourceCursor {
                prefixes: vec![fixture.prefix],
                pending: VecDeque::new(),
            };
            let mut seen = std::collections::BTreeSet::new();
            let mut queries = Vec::new();
            for _ in 0..128 {
                let mut budget = 8;
                for _ in 0..32 {
                    let Some(id) = cursor.next(&backend, &mut budget).await.unwrap() else {
                        break;
                    };
                    assert!(seen.insert(id));
                }
                queries.push(8 - budget);
                if cursor.prefixes.is_empty() && cursor.pending.is_empty() {
                    break;
                }
            }
            assert_eq!(seen, fixture.ids.into_iter().collect());
            assert!(queries.iter().all(|n| *n <= 8));
            assert!(cursor.prefixes.is_empty() && cursor.pending.is_empty());
            std::fs::write(
                format!("{path}.result.json"),
                serde_json::json!({
                    "passed":true,"ids":seen,"queries_per_pass":queries,
                })
                .to_string(),
            )
            .unwrap();
        });
    }
}
