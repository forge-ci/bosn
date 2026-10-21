//! Bounded keyset pages for engine cleanup and native recovery phases.
use super::*;

const ACTIVE: &str = "SELECT e.kind,e.detail FROM events e JOIN (SELECT MAX(id) AS id FROM events WHERE kind GLOB 'act.engine.v1:*' GROUP BY kind) latest ON latest.id=e.id WHERE e.kind > ? AND (json_extract(e.detail,'$.state') IS NULL OR json_extract(e.detail,'$.state')<>'terminal') ORDER BY e.kind LIMIT ?";
const TOOLS: &str = "SELECT e.kind,e.detail FROM events e JOIN (SELECT MAX(id) AS id FROM events WHERE kind GLOB 'act.engine.v1:*' GROUP BY kind) latest ON latest.id=e.id WHERE e.kind > ? AND json_extract(e.detail,'$.tool_recovery') IS NOT NULL AND ((json_extract(e.detail,'$.state')='cleanup_required' AND json_extract(e.detail,'$.tool_recovery_reserved_at') IS NOT NULL AND json_extract(e.detail,'$.tool_recovery_source_stopped_at') IS NOT NULL AND json_extract(e.detail,'$.tool_recovery_completion') IS NULL) OR (json_extract(e.detail,'$.tool_recovery_completion') IS NOT NULL AND json_extract(e.detail,'$.tool_recovery_completion.reference_released_at') IS NULL)) ORDER BY e.kind LIMIT ?";

impl Registry {
    /// Bounded active engine page in immutable run-ID order. Repeat passes to
    /// cover concurrent inserts at or before the cursor.
    pub fn pending_act_engines(
        &self,
        after_run_id: Option<&str>,
        limit: usize,
    ) -> Result<ActEngineRecoveryPage, Error> {
        page(self, ACTIVE, after_run_id, limit)
    }

    /// Publication-ready sources and completed references needing release,
    /// including terminal records whose source expired before release ack.
    /// The runtime must check a publication's finite lifetime before acting.
    pub fn pending_tool_recoveries(
        &self,
        after_run_id: Option<&str>,
        limit: usize,
    ) -> Result<ActEngineRecoveryPage, Error> {
        page(self, TOOLS, after_run_id, limit)
    }
}

fn page(
    registry: &Registry,
    sql: &str,
    after_run_id: Option<&str>,
    limit: usize,
) -> Result<ActEngineRecoveryPage, Error> {
    if limit == 0 || limit > MAX_PAGE_SIZE {
        return Err(Error::BadRow("act recovery page limit"));
    }
    let after_kind = after_run_id.map(kind).transpose()?.unwrap_or_default();
    let rows = registry.connection.query(
        sql,
        &[Value::Text(after_kind), Value::Integer((limit + 1) as i64)],
        QueryLimits {
            max_rows: limit + 1,
            max_bytes: (limit + 1) * 32768,
        },
    )?;
    let more = rows.len() > limit;
    let items = rows
        .iter()
        .take(limit)
        .map(|r| {
            let k = text(r, 0)?;
            decode(
                &text(r, 1)?,
                k.strip_prefix(PREFIX)
                    .ok_or(Error::BadRow("act event kind"))?,
            )
        })
        .collect::<Result<Vec<_>, Error>>()?;
    let owner = registry.registry_id()?;
    if items.iter().any(|r| r.registry_id != owner) {
        return Err(Error::ResourceIdentityConflict);
    }
    let next_run_id = more.then(|| {
        items
            .last()
            .expect("nonempty bounded page")
            .intent
            .run_id
            .clone()
    });
    Ok(ActEngineRecoveryPage { items, next_run_id })
}
