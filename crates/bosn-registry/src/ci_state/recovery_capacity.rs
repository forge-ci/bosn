//! Recovery owns a finite reserve and recycles only proven-absent attempts.
use super::*;

const PER_ENGINE: usize = 8;
const TOTAL: i64 = 90_000;

pub(super) fn admit(transaction: &mut Transaction<'_>, snapshot: &Snapshot) -> Result<(), Error> {
    let owner = serde_json::from_str::<cache_helper::CacheHelperRecord>(&snapshot.detail)
        .map_err(|_| Error::BadRow("recovery helper capacity snapshot"))?;
    let binding = owner
        .intent
        .role
        .as_ref()
        .and_then(cache_helper::CacheHelperRole::recovery_binding)
        .ok_or(Error::BadRow("recovery helper capacity binding"))?;
    // This clock is independent of intent timestamps. Future-dated authority
    // cannot spend the reserve or advance replay protection through recycling.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| Error::BadRow("recovery capacity observation clock"))?
        .as_secs_f64();
    if snapshot.family != HELPER || !snapshot.pending || snapshot.created_at > now {
        return Err(Error::BadRow("recovery helper capacity admission"));
    }
    let rows = transaction.query(
        "SELECT detail FROM ci_current WHERE family=1 AND id=?",
        &[Value::Text(binding.run_id.into())],
        QueryLimits {
            max_rows: 1,
            max_bytes: act::RECORD_BYTES + 128,
        },
    )?;
    let engine = rows
        .first()
        .ok_or(Error::BadRow("recovery capacity engine missing"))?;
    let engine = act::decode(&text(engine, 0)?, binding.run_id)?;
    if engine.registry_id != owner.intent.registry_id
        || engine.tool_recovery.as_ref() != Some(binding.intent)
    {
        return Err(Error::BadRow("recovery capacity engine mismatch"));
    }
    let attempts = transaction.query(
        "SELECT id,detail FROM ci_current WHERE family=2 AND engine_run=? ORDER BY id LIMIT ?",
        &[
            Value::Text(binding.run_id.into()),
            Value::Integer(PER_ENGINE as i64),
        ],
        QueryLimits {
            max_rows: PER_ENGINE,
            max_bytes: PER_ENGINE * (cache_helper::RECORD_BYTES + 128),
        },
    )?;
    if attempts.len() == PER_ENGINE {
        for row in attempts {
            let id = text(&row, 0)?;
            let detail = text(&row, 1)?;
            let old = cache_helper::decode(&detail, &id, &owner.intent.registry_id)?;
            if old.state != cache_helper::CacheHelperState::Removed
                || old.updated_at > now
                || old.intent.created_at >= snapshot.created_at
                || old
                    .intent
                    .role
                    .as_ref()
                    .and_then(cache_helper::CacheHelperRole::recovery_binding)
                    .is_none_or(|old| old.intent != binding.intent || old.run_id != binding.run_id)
            {
                continue;
            }
            // Removed is exact immutable-ID absence or the late-create absence
            // proof. The parent has its own publication/release authority.
            retention::forget(transaction, &helper(old, &detail))?;
            return Ok(());
        }
        return Err(Error::BadRow(
            "recovery helper reserve held by uncertain attempts",
        ));
    }
    let rows = transaction.query(
        "SELECT COUNT(*) FROM (SELECT 1 FROM ci_current LIMIT ?)",
        &[Value::Integer(TOTAL)],
        QueryLimits {
            max_rows: 1,
            max_bytes: 128,
        },
    )?;
    if integer(&rows[0], 0)? >= TOTAL {
        return Err(Error::BadRow(
            "recovery helper total authority capacity reached",
        ));
    }
    Ok(())
}
