//! Bounded keyset pages for engine cleanup and native recovery phases.
use super::*;

const ACTIVE: &str = "SELECT 'act.engine.v1:'||id,detail FROM ci_current WHERE family=1 AND pending=1 AND id>? ORDER BY id LIMIT ?";
const TOOLS: &str = "SELECT 'act.engine.v1:'||id,detail FROM ci_current WHERE family=1 AND tools_pending=1 AND id>? ORDER BY id LIMIT ?";

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
    registry.require_ci_ready()?;
    let after_kind = after_run_id
        .map(|id| kind(id).map(|_| id.to_owned()))
        .transpose()?
        .unwrap_or_default();
    let rows = registry.connection.query(
        sql,
        &[Value::Text(after_kind), Value::Integer((limit + 1) as i64)],
        QueryLimits {
            max_rows: limit + 1,
            max_bytes: (limit + 1) * (RECORD_BYTES + 1024),
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
