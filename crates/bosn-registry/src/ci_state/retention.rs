//! Finite CI authority, with a replay floor instead of permanent tombstones.
use super::*;

const CAPACITY: i64 = 10_000;
const KEEP_SECONDS: f64 = 7.0 * 86400.0;
const FLOOR: &str = "ci.retention.floor";
const CURSOR: &str = "ci.retention.cursor";
const MAX_BATCH: usize = 64;

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    at: f64,
    family: i64,
    id: String,
}

fn meta_value(transaction: &mut Transaction<'_>, key: &str) -> Result<Option<String>, Error> {
    let rows = transaction.query(
        "SELECT value FROM meta WHERE key=?",
        &[Value::Text(key.into())],
        QueryLimits {
            max_rows: 1,
            max_bytes: 2048,
        },
    )?;
    rows.first().map(|row| text(row, 0)).transpose()
}

fn floor(transaction: &mut Transaction<'_>) -> Result<Option<f64>, Error> {
    meta_value(transaction, FLOOR)?
        .map(|value| {
            let value = value
                .parse::<f64>()
                .map_err(|_| Error::BadRow("CI retention floor"))?;
            if !value.is_finite() || value < 0.0 {
                return Err(Error::BadRow("CI retention floor"));
            }
            Ok(value)
        })
        .transpose()
}

fn set_meta(transaction: &mut Transaction<'_>, key: &str, value: &str) -> Result<(), Error> {
    transaction.execute("INSERT INTO meta(key,value) VALUES(?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value", &[Value::Text(key.into()),Value::Text(value.into())])?;
    Ok(())
}

pub(super) fn admit(
    transaction: &mut Transaction<'_>,
    snapshot: &Snapshot,
    recovery: bool,
) -> Result<(), Error> {
    let exists = transaction.query(
        "SELECT id FROM ci_current WHERE family=? AND id=?",
        &[
            Value::Integer(snapshot.family),
            Value::Text(snapshot.id.clone()),
        ],
        QueryLimits {
            max_rows: 1,
            max_bytes: 1024,
        },
    )?;
    if !exists.is_empty() {
        return Ok(());
    }
    if floor(transaction)?.is_some_and(|floor| snapshot.created_at <= floor) {
        return Err(Error::BadRow(
            "CI intent predates retained authority; submit fresh work",
        ));
    }
    if recovery {
        return recovery_capacity::admit(transaction, snapshot);
    }
    let rows = transaction.query(
        "SELECT COUNT(*) FROM (SELECT 1 FROM ci_current LIMIT ?)",
        &[Value::Integer(CAPACITY)],
        QueryLimits {
            max_rows: 1,
            max_bytes: 1024,
        },
    )?;
    if integer(&rows[0], 0)? >= CAPACITY {
        return Err(Error::BadRow(
            "CI authority capacity reached; existing recovery retained",
        ));
    }
    Ok(())
}

fn referenced(transaction: &mut Transaction<'_>, snapshot: &Snapshot) -> Result<bool, Error> {
    let (sql, key) = if snapshot.family == ENGINE {
        (
            "SELECT id FROM ci_current WHERE family=2 AND pending=1 AND engine_run=? LIMIT 1",
            snapshot.id.as_str(),
        )
    } else if let Some(run) = snapshot.engine_run.as_deref() {
        (
            "SELECT id FROM ci_current WHERE family=1 AND id=? AND (pending=1 OR tools_pending=1)",
            run,
        )
    } else {
        return Ok(false);
    };
    let rows = transaction.query(
        sql,
        &[Value::Text(key.into())],
        QueryLimits {
            max_rows: 1,
            max_bytes: 1024,
        },
    )?;
    if !rows.is_empty() {
        return Ok(true);
    }
    if snapshot.family == ENGINE {
        let rows = transaction.query(
            "SELECT id FROM leases WHERE resource_id=? LIMIT 1",
            &[Value::Text(format!("act-engine:{}", snapshot.id))],
            QueryLimits {
                max_rows: 1,
                max_bytes: 1024,
            },
        )?;
        return Ok(!rows.is_empty());
    }
    Ok(false)
}

fn remove(transaction: &mut Transaction<'_>, family: i64, id: &str) -> Result<(), Error> {
    let values = [Value::Integer(family), Value::Text(id.into())];
    let events = transaction.query(
        "SELECT event_id FROM ci_snapshot_events WHERE family=? AND id=? ORDER BY event_id",
        &values,
        QueryLimits {
            max_rows: AUDIT_TAIL as usize,
            max_bytes: 4096,
        },
    )?;
    for row in events {
        transaction.execute(
            "DELETE FROM events WHERE id=?",
            &[Value::Integer(integer(&row, 0)?)],
        )?;
    }
    transaction.execute(
        "DELETE FROM ci_snapshot_events WHERE family=? AND id=?",
        &values,
    )?;
    transaction.execute("DELETE FROM ci_current WHERE family=? AND id=?", &values)?;
    Ok(())
}

/// Persist replay protection before forgetting already-terminal authority.
pub(super) fn forget(transaction: &mut Transaction<'_>, snapshot: &Snapshot) -> Result<(), Error> {
    let previous = floor(transaction)?;
    let advanced = previous.map_or(snapshot.created_at, |value| value.max(snapshot.created_at));
    if previous != Some(advanced) {
        set_meta(transaction, FLOOR, &advanced.to_string())?;
    }
    remove(transaction, snapshot.family, &snapshot.id)
}

pub(super) fn maintain(
    transaction: &mut Transaction<'_>,
    now: f64,
    limit: usize,
) -> Result<usize, Error> {
    if !now.is_finite() || now < 0.0 || limit == 0 || limit > MAX_BATCH {
        return Err(Error::BadRow("CI retention bounds"));
    }
    let after = meta_value(transaction, CURSOR)?
        .map(|value| {
            serde_json::from_str::<Cursor>(&value).map_err(|_| Error::BadRow("CI retention cursor"))
        })
        .transpose()?;
    if after.as_ref().is_some_and(|cursor| {
        !cursor.at.is_finite()
            || cursor.at < 0.0
            || !matches!(cursor.family, ENGINE | HELPER)
            || !is_uuid(&cursor.id)
    }) {
        return Err(Error::BadRow("CI retention cursor"));
    }
    let values = vec![
        Value::Real(now - KEEP_SECONDS),
        Value::Real(after.as_ref().map_or(-1.0, |cursor| cursor.at)),
        Value::Integer(after.as_ref().map_or(0, |cursor| cursor.family)),
        Value::Text(
            after
                .as_ref()
                .map_or(String::new(), |cursor| cursor.id.clone()),
        ),
        Value::Integer(limit as i64),
    ];
    let rows=transaction.query("SELECT family,id,collect_after,detail FROM ci_current WHERE collectible=1 AND collect_after<? AND (collect_after,family,id)>(?,?,?) ORDER BY collect_after,family,id LIMIT ?", &values,QueryLimits{max_rows:limit,max_bytes:limit*(act::RECORD_BYTES+1024)})?;
    let owner = meta_value(transaction, "registry_id")?.ok_or(Error::BadRow("registry_id"))?;
    let mut removed = 0;
    for row in &rows {
        let family = integer(row, 0)?;
        let id = text(row, 1)?;
        let prefix = match family {
            ENGINE => act::PREFIX,
            HELPER => cache_helper::PREFIX,
            _ => return Err(Error::BadRow("CI retention family")),
        };
        let snapshot = parse(&format!("{prefix}{id}"), &text(row, 3)?, &owner)?
            .ok_or(Error::BadRow("CI retention snapshot"))?;
        if !snapshot.collectible || snapshot.collect_after >= now - KEEP_SECONDS {
            return Err(Error::BadRow("CI retention authority mismatch"));
        }
        if !referenced(transaction, &snapshot)? {
            forget(transaction, &snapshot)?;
            removed += 1;
        }
    }
    if rows.len() == limit {
        let row = rows.last().ok_or(Error::BadRow("CI retention page"))?;
        let cursor = Cursor {
            at: real(row, 2)?,
            family: integer(row, 0)?,
            id: text(row, 1)?,
        };
        set_meta(
            transaction,
            CURSOR,
            &serde_json::to_string(&cursor).map_err(|_| Error::BadRow("CI retention cursor"))?,
        )?;
    } else if after.is_some() {
        transaction.execute(
            "DELETE FROM meta WHERE key=?",
            &[Value::Text(CURSOR.into())],
        )?;
    }
    Ok(removed)
}

impl Registry {
    /// One bounded pass. Existing live/recovery authority is never evicted.
    /// Removed identities leave a monotonic floor that rejects old Begin replay.
    pub fn maintain_ci_state(&mut self, now: f64, limit: usize) -> Result<usize, Error> {
        self.require_ci_ready()?;
        let mut transaction = self.connection.begin_immediate()?;
        let removed = maintain(&mut transaction, now, limit)?;
        transaction.commit()?;
        Ok(removed)
    }
}
