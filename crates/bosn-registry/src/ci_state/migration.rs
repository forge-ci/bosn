//! A resumable reverse journal scan, with no admission during backfill.
use super::*;

const CURSOR: &str = "ci.migration.cursor";
const PAGE: usize = 128;

pub(crate) fn prepare(connection: &mut Connection) -> Result<(), Error> {
    let collisions = connection.query(
        "SELECT name FROM sqlite_master WHERE name IN ('ci_current','ci_snapshot_events')",
        &[],
        QueryLimits {
            max_rows: 2,
            max_bytes: 1024,
        },
    )?;
    if !collisions.is_empty() {
        return Err(Error::InvalidSchema);
    }
    let mut transaction = connection.begin_immediate()?;
    initialize(&mut transaction)?;
    transaction.execute("INSERT INTO meta(key,value) SELECT ?,CAST(COALESCE(MAX(id),0)+1 AS TEXT) FROM events WHERE 1 ON CONFLICT(key) DO NOTHING", &[Value::Text(CURSOR.into())])?;
    // Fence older writers before the first resumable page commits.
    transaction.execute(
        "UPDATE meta SET value=? WHERE key='schema_version'",
        &[Value::Text(SCHEMA_VERSION.to_string())],
    )?;
    transaction.commit()?;
    Ok(())
}

impl Registry {
    /// One bounded schema-v5 backfill page. Call on a blocking worker until
    /// true before admitting registry commands. The writer fence stays held.
    pub fn migrate_ci_step(&mut self) -> Result<bool, Error> {
        let Some(cursor) = meta(&self.connection, CURSOR)? else {
            return Ok(true);
        };
        let cursor = cursor
            .parse::<i64>()
            .map_err(|_| Error::BadRow("CI migration cursor"))?;
        let owner = self.registry_id()?;
        let mut transaction = self.connection.begin_immediate()?;
        let managed = format!(
            "kind GLOB '{}*' OR kind GLOB '{}*'",
            act::PREFIX,
            cache_helper::PREFIX
        );
        let query = format!(
            "SELECT id,CASE WHEN {managed} THEN kind ELSE '' END,CASE WHEN {managed} THEN detail ELSE '' END FROM events WHERE id<? ORDER BY id DESC LIMIT ?"
        );
        let rows = transaction.query(
            &query,
            &[Value::Integer(cursor), Value::Integer(PAGE as i64)],
            QueryLimits {
                max_rows: PAGE,
                max_bytes: 8 * 1024 * 1024,
            },
        )?;
        for row in &rows {
            let event_id = integer(row, 0)?;
            let kind = text(row, 1)?;
            let identity = kind
                .strip_prefix(act::PREFIX)
                .map(|id| (ENGINE, id))
                .or_else(|| {
                    kind.strip_prefix(cache_helper::PREFIX)
                        .map(|id| (HELPER, id))
                });
            if let Some((family, id)) = identity {
                let current = transaction.query(
                    "SELECT id FROM ci_current WHERE family=? AND id=?",
                    &[Value::Integer(family), Value::Text(id.into())],
                    QueryLimits {
                        max_rows: 1,
                        max_bytes: 1024,
                    },
                )?;
                if current.is_empty() {
                    let snapshot = parse(&kind, &text(row, 2)?, &owner)?
                        .ok_or(Error::BadRow("CI migration kind"))?;
                    install(&mut transaction, &snapshot, false)?;
                }
                retain_event(&mut transaction, family, id, event_id)?;
            }
        }
        let complete = rows.len() < PAGE;
        if complete {
            transaction.execute(
                "DELETE FROM meta WHERE key=?",
                &[Value::Text(CURSOR.into())],
            )?;
        } else {
            transaction.execute(
                "UPDATE meta SET value=? WHERE key=?",
                &[
                    Value::Text(integer(rows.last().unwrap(), 0)?.to_string()),
                    Value::Text(CURSOR.into()),
                ],
            )?;
        }
        transaction.commit()?;
        Ok(complete)
    }

    pub(crate) fn require_ci_ready(&self) -> Result<(), Error> {
        if meta(&self.connection, CURSOR)?.is_some() {
            return Err(Error::MigrationInProgress);
        }
        Ok(())
    }
}
