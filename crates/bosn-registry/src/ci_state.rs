//! Typed current authority and a bounded audit tail for CI ownership records.
use super::*;

pub(crate) mod migration;
mod recovery_capacity;
mod retention;
#[cfg(test)]
mod tests;

pub(crate) const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS ci_current (
 family INTEGER NOT NULL CHECK(family IN (1,2)), id TEXT NOT NULL, detail TEXT NOT NULL,
 pending INTEGER NOT NULL CHECK(pending IN (0,1)),
 tools_pending INTEGER NOT NULL CHECK(tools_pending IN (0,1)),
 source TEXT, release_owner TEXT, engine_run TEXT, created_at REAL NOT NULL,
 updated_at REAL NOT NULL, collect_after REAL NOT NULL, collectible INTEGER NOT NULL CHECK(collectible IN (0,1)),
 PRIMARY KEY(family,id));
CREATE INDEX IF NOT EXISTS ci_pending ON ci_current(family,id) WHERE pending=1;
CREATE INDEX IF NOT EXISTS ci_tools_pending ON ci_current(id) WHERE family=1 AND tools_pending=1;
CREATE INDEX IF NOT EXISTS ci_source ON ci_current(source,id) WHERE family=2 AND pending=1;
CREATE INDEX IF NOT EXISTS ci_release_owner ON ci_current(release_owner,id) WHERE family=2 AND pending=1;
CREATE INDEX IF NOT EXISTS ci_helper_run ON ci_current(engine_run,id) WHERE family=2 AND pending=1;
CREATE INDEX IF NOT EXISTS ci_helper_run_all ON ci_current(engine_run,id) WHERE family=2;
CREATE INDEX IF NOT EXISTS ci_collectible ON ci_current(collect_after,family,id) WHERE collectible=1;
CREATE TABLE IF NOT EXISTS ci_snapshot_events (
 event_id INTEGER PRIMARY KEY, family INTEGER NOT NULL, id TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS ci_snapshot_tail ON ci_snapshot_events(family,id,event_id DESC);
"#;

pub(crate) const ENGINE: i64 = 1;
pub(crate) const HELPER: i64 = 2;
const AUDIT_TAIL: i64 = 3;

pub(crate) enum WriteMode {
    Event,
    Import(i64),
    /// Only after exact recovery phase admission in begin_cache_helper.
    RecoveryHelper,
}

pub(crate) struct Snapshot {
    family: i64,
    id: String,
    detail: String,
    pending: bool,
    tools_pending: bool,
    source: Option<String>,
    release_owner: Option<String>,
    engine_run: Option<String>,
    created_at: f64,
    updated_at: f64,
    collectible: bool,
    collect_after: f64,
}

fn engine(record: act::ActEngineRecord, detail: &str) -> Snapshot {
    let completion = record.tool_recovery_completion.as_ref();
    let tools_pending = record.tool_recovery.is_some()
        && ((record.state == act::ActEngineState::CleanupRequired
            && record.tool_recovery_reserved_at.is_some()
            && record.tool_recovery_source_stopped_at.is_some()
            && completion.is_none())
            || completion.is_some_and(|done| done.reference_released_at.is_none()));
    let pending = record.state != act::ActEngineState::Terminal;
    let collect_after = if completion.is_none() {
        record
            .tool_recovery
            .as_ref()
            .map_or(record.updated_at, |intent| {
                record.updated_at.max(intent.expires_at_seconds as f64)
            })
    } else {
        record.updated_at
    };
    Snapshot {
        family: ENGINE,
        id: record.intent.run_id,
        detail: detail.into(),
        pending,
        tools_pending,
        source: record
            .tool_recovery
            .as_ref()
            .map(|value| value.source_volume.clone()),
        release_owner: None,
        engine_run: None,
        created_at: record.intent.created_at,
        updated_at: record.updated_at,
        // Terminal already acknowledges exact engine/storage absence. Older
        // records need not carry the subsequently added disposal marker.
        // Unpublished abandonment still waits through expiry; a publication
        // with an unacknowledged reference release is never collectible.
        collectible: !pending && completion.is_none_or(|done| done.reference_released_at.is_some()),
        collect_after,
    }
}

fn helper(record: cache_helper::CacheHelperRecord, detail: &str) -> Snapshot {
    use cache_helper::{CacheHelperRole, CacheHelperState, RecoveryHelperPhase};
    let role = record.intent.role.as_ref();
    let binding = role.and_then(CacheHelperRole::recovery_binding);
    let source = match role {
        Some(CacheHelperRole::ToolSourceFinalizationV1 { context, .. }) => {
            Some(context.recovery.source_volume.clone())
        }
        _ => binding.as_ref().and_then(|value| {
            (value.phase == RecoveryHelperPhase::Publish)
                .then(|| value.intent.source_volume.clone())
        }),
    };
    let engine_run = match role {
        Some(CacheHelperRole::ToolSourceFinalizationV1 { context, .. }) => {
            Some(context.intent.run_id.clone())
        }
        _ => binding.as_ref().map(|value| value.run_id.to_owned()),
    };
    let release_owner = binding.and_then(|value| {
        (value.phase == RecoveryHelperPhase::Release).then(|| value.intent.owner.clone())
    });
    let pending = record.state != CacheHelperState::Removed;
    Snapshot {
        family: HELPER,
        id: record.intent.nonce,
        detail: detail.into(),
        pending,
        tools_pending: false,
        source,
        release_owner,
        engine_run,
        created_at: record.intent.created_at,
        updated_at: record.updated_at,
        collectible: !pending,
        collect_after: record.updated_at,
    }
}

pub(crate) fn parse(kind: &str, detail: &str, owner: &str) -> Result<Option<Snapshot>, Error> {
    if let Some(id) = kind.strip_prefix(act::PREFIX) {
        if detail.len() > act::RECORD_BYTES {
            return Err(Error::BadRow("act snapshot size"));
        }
        let record = act::decode(detail, id)?;
        if record.registry_id != owner {
            return Err(Error::ResourceIdentityConflict);
        }
        Ok(Some(engine(record, detail)))
    } else if let Some(id) = kind.strip_prefix(cache_helper::PREFIX) {
        if detail.len() > cache_helper::RECORD_BYTES {
            return Err(Error::BadRow("cache helper snapshot size"));
        }
        Ok(Some(helper(
            cache_helper::decode(detail, id, owner)?,
            detail,
        )))
    } else {
        Ok(None)
    }
}

fn flag(value: bool) -> Value {
    Value::Integer(i64::from(value))
}

fn optional_text(value: &Option<String>) -> Value {
    value
        .as_ref()
        .map_or(Value::Null, |text| Value::Text(text.clone()))
}

pub(crate) fn install(
    transaction: &mut Transaction<'_>,
    snapshot: &Snapshot,
    newest: bool,
) -> Result<(), Error> {
    let conflict = if newest {
        "DO UPDATE SET detail=excluded.detail,pending=excluded.pending,tools_pending=excluded.tools_pending,source=excluded.source,release_owner=excluded.release_owner,engine_run=excluded.engine_run,created_at=excluded.created_at,updated_at=excluded.updated_at,collect_after=excluded.collect_after,collectible=excluded.collectible"
    } else {
        "DO NOTHING"
    };
    transaction.execute(
        &format!("INSERT INTO ci_current(family,id,detail,pending,tools_pending,source,release_owner,engine_run,created_at,updated_at,collect_after,collectible) VALUES(?,?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(family,id) {conflict}"),
        &[
            Value::Integer(snapshot.family), Value::Text(snapshot.id.clone()),
            Value::Text(snapshot.detail.clone()), flag(snapshot.pending), flag(snapshot.tools_pending),
            optional_text(&snapshot.source), optional_text(&snapshot.release_owner), optional_text(&snapshot.engine_run),
            Value::Real(snapshot.created_at), Value::Real(snapshot.updated_at), Value::Real(snapshot.collect_after), flag(snapshot.collectible),
        ],
    )?;
    Ok(())
}

pub(crate) fn retain_event(
    transaction: &mut Transaction<'_>,
    family: i64,
    id: &str,
    event_id: i64,
) -> Result<(), Error> {
    transaction.execute(
        "INSERT OR IGNORE INTO ci_snapshot_events(event_id,family,id) VALUES(?,?,?)",
        &[
            Value::Integer(event_id),
            Value::Integer(family),
            Value::Text(id.into()),
        ],
    )?;
    // At most one new event is added to an already bounded tail.
    let old = transaction.query(
        "SELECT event_id FROM ci_snapshot_events WHERE family=? AND id=? ORDER BY event_id DESC LIMIT 1 OFFSET ?",
        &[Value::Integer(family),Value::Text(id.into()),Value::Integer(AUDIT_TAIL)],
        QueryLimits { max_rows: 1, max_bytes: 1024 },
    )?;
    if let Some(row) = old.first() {
        let event_id = integer(row, 0)?;
        transaction.execute("DELETE FROM events WHERE id=?", &[Value::Integer(event_id)])?;
        transaction.execute(
            "DELETE FROM ci_snapshot_events WHERE event_id=?",
            &[Value::Integer(event_id)],
        )?;
    }
    Ok(())
}

pub(crate) fn append(
    transaction: &mut Transaction<'_>,
    at: f64,
    kind: &str,
    detail: &str,
    mode: WriteMode,
) -> Result<(), Error> {
    let rows = transaction.query(
        "SELECT value FROM meta WHERE key='registry_id'",
        &[],
        QueryLimits {
            max_rows: 1,
            max_bytes: 1024,
        },
    )?;
    let owner = text(rows.first().ok_or(Error::BadRow("registry_id"))?, 0)?;
    let snapshot = parse(kind, detail, &owner)?;
    if !matches!(mode, WriteMode::Import(_))
        && let Some(snapshot) = &snapshot
    {
        retention::admit(
            transaction,
            snapshot,
            matches!(mode, WriteMode::RecoveryHelper),
        )?;
    }
    let mut values = vec![
        Value::Real(at),
        Value::Text(kind.into()),
        Value::Text(detail.into()),
    ];
    let sql = if let WriteMode::Import(id) = mode {
        values.insert(0, Value::Integer(id));
        "INSERT INTO events(id,at,kind,detail) VALUES(?,?,?,?)"
    } else {
        "INSERT INTO events(at,kind,detail) VALUES(?,?,?)"
    };
    transaction.execute(sql, &values)?;
    if let Some(snapshot) = snapshot {
        let rows = transaction.query(
            "SELECT last_insert_rowid()",
            &[],
            QueryLimits {
                max_rows: 1,
                max_bytes: 1024,
            },
        )?;
        let event_id = integer(&rows[0], 0)?;
        let prior = transaction.query("SELECT event_id FROM ci_snapshot_events WHERE family=? AND id=? ORDER BY event_id DESC LIMIT 1", &[Value::Integer(snapshot.family),Value::Text(snapshot.id.clone())],QueryLimits{max_rows:1,max_bytes:1024})?;
        if prior
            .first()
            .map(|row| integer(row, 0))
            .transpose()?
            .is_none_or(|id| id < event_id)
        {
            install(transaction, &snapshot, true)?;
        }
        retain_event(transaction, snapshot.family, &snapshot.id, event_id)?;
    }
    Ok(())
}

pub(crate) fn initialize(transaction: &mut Transaction<'_>) -> Result<(), Error> {
    for statement in SCHEMA
        .split(';')
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        transaction.execute(statement, &[])?;
    }
    Ok(())
}

pub(crate) fn validate(connection: &Connection) -> Result<(), Error> {
    for (table, expected) in [
        (
            "ci_current",
            &[
                "family",
                "id",
                "detail",
                "pending",
                "tools_pending",
                "source",
                "release_owner",
                "engine_run",
                "created_at",
                "updated_at",
                "collect_after",
                "collectible",
            ][..],
        ),
        ("ci_snapshot_events", &["event_id", "family", "id"][..]),
    ] {
        let columns = connection.query(
            &format!("PRAGMA table_info({table})"),
            &[],
            QueryLimits {
                max_rows: 16,
                max_bytes: 8192,
            },
        )?;
        for name in expected {
            if !columns
                .iter()
                .any(|row| matches!(row.get(1),Some(Value::Text(column)) if column==name))
            {
                return Err(Error::InvalidSchema);
            }
        }
    }
    for statement in SCHEMA
        .split(';')
        .map(str::trim)
        .filter(|sql| sql.starts_with("CREATE INDEX"))
    {
        let expected = statement.replace("IF NOT EXISTS ", "");
        let name = expected
            .split_whitespace()
            .nth(2)
            .ok_or(Error::InvalidSchema)?;
        let rows = connection.query(
            "SELECT sql FROM sqlite_master WHERE type='index' AND name=?",
            &[Value::Text(name.into())],
            QueryLimits {
                max_rows: 1,
                max_bytes: 4096,
            },
        )?;
        let actual = rows
            .first()
            .map(|row| text(row, 0))
            .transpose()?
            .ok_or(Error::InvalidSchema)?;
        if actual.split_whitespace().collect::<Vec<_>>()
            != expected.split_whitespace().collect::<Vec<_>>()
        {
            return Err(Error::InvalidSchema);
        }
    }
    Ok(())
}
