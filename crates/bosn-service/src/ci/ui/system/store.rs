//! SQLite persistence. Every method (including final flush) runs on a blocking worker.
use super::history::{Bucket, History, METRICS, Metric, TIERS};
use kernal_api::{
    platform::fs,
    sqlite::{Connection, QueryLimits, Value},
};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub struct Store {
    path: PathBuf,
    lock: Option<fs::OwnedFileLock>,
    saved: History,
}
impl Store {
    pub fn new(state: &Path) -> Self {
        Self {
            path: state.join("system-history.sqlite3"),
            lock: None,
            saved: History::default(),
        }
    }
    pub fn flush(&mut self, history: &mut History, now: u64) -> Result<bool> {
        if self.lock.is_none() {
            let file = fs::open_lock_file(&self.path.with_file_name("system-history.lock"))?;
            match fs::try_lock_exclusive_owned(file) {
                Ok(lock) => {
                    let connection = self.open()?;
                    let loaded = load(&connection, now)?;
                    merge(history, loaded);
                    self.lock = Some(lock);
                }
                Err(error) if fs::is_lock_conflict(&error) => return Ok(false),
                Err(error) => return Err(error.into()),
            }
            // Merge the incumbent's last flush before taking over. Local samples
            // were never written while its lock was held.
        }
        let mut connection = self.open()?;
        let sql = insert_sql();
        let mut tx = connection.begin_immediate()?;
        for (tier, buckets) in history.tiers.iter().enumerate() {
            for (slot, bucket) in buckets.iter().enumerate() {
                if let Some(bucket) = bucket
                    && self.saved.tiers[tier][slot].as_ref() != Some(bucket)
                {
                    tx.execute(&sql, &values(tier, bucket))?;
                }
            }
        }
        tx.commit()?;
        self.saved = history.clone();
        Ok(true)
    }
    fn open(&self) -> Result<Connection> {
        let existed = self.path.exists();
        let connection = Connection::open_with_busy_timeout(&self.path, Duration::from_secs(1))?;
        let rows = connection.query("PRAGMA user_version", &[], QueryLimits::default())?;
        let version = rows.first().and_then(|r| r.get(0));
        if !matches!(version, Some(Value::Integer(1)))
            && (existed || !matches!(version, Some(Value::Integer(0))))
        {
            // Closing checkpoints WAL before moving the standalone database.
            drop(connection);
            std::fs::rename(&self.path, self.path.with_extension("sqlite3.old"))?;
            return self.open();
        }
        let fields = METRICS
            .iter()
            .map(|m| format!("{m}_avg REAL, {m}_max REAL, {m}_n INTEGER NOT NULL"))
            .collect::<Vec<_>>()
            .join(",");
        connection.execute(&format!("CREATE TABLE IF NOT EXISTS samples(tier INTEGER, slot INTEGER, at_ms INTEGER, n INTEGER, {fields}, PRIMARY KEY(tier,slot))"), &[])?;
        connection.execute("PRAGMA user_version=1", &[])?;
        Ok(connection)
    }
}
fn insert_sql() -> String {
    format!(
        "INSERT OR REPLACE INTO samples VALUES ({})",
        vec!["?"; 4 + METRICS.len() * 3].join(",")
    )
}
fn values(tier: usize, b: &Bucket) -> Vec<Value> {
    let (step, capacity) = TIERS[tier];
    let mut values = vec![
        Value::Integer(tier as i64),
        Value::Integer((b.at_ms / step % capacity as u64) as i64),
        Value::Integer(b.at_ms as i64),
        Value::Integer(b.n as i64),
    ];
    for metric in b.metrics {
        values.extend([
            metric.avg.map_or(Value::Null, Value::Real),
            metric.max.map_or(Value::Null, Value::Real),
            Value::Integer(metric.n as i64),
        ]);
    }
    values
}
fn integer(value: Option<&Value>) -> Result<u64> {
    match value {
        Some(Value::Integer(n)) if *n >= 0 => Ok(*n as u64),
        _ => Err("invalid history integer".into()),
    }
}
fn number(value: Option<&Value>) -> Result<Option<f64>> {
    match value {
        Some(Value::Null) => Ok(None),
        Some(Value::Real(n)) if n.is_finite() => Ok(Some(*n)),
        _ => Err("invalid history metric".into()),
    }
}
fn load(connection: &Connection, now: u64) -> Result<History> {
    let mut history = History::default();
    let rows = connection.query(
        "SELECT * FROM samples",
        &[],
        QueryLimits {
            max_rows: TIERS.iter().map(|t| t.1).sum(),
            max_bytes: 4 * 1024 * 1024,
        },
    )?;
    for row in rows {
        let tier = usize::try_from(integer(row.get(0))?)?;
        if tier >= TIERS.len() {
            return Err("invalid history tier".into());
        }
        let mut bucket = Bucket::empty(integer(row.get(2))?);
        bucket.n = integer(row.get(3))?;
        for (i, metric) in bucket.metrics.iter_mut().enumerate() {
            *metric = Metric {
                avg: number(row.get(4 + i * 3))?,
                max: number(row.get(5 + i * 3))?,
                n: integer(row.get(6 + i * 3))?,
            };
        }
        history.load(tier, bucket, now);
    }
    Ok(history)
}
fn merge(history: &mut History, loaded: History) {
    for (tier, (local, persisted)) in history.tiers.iter_mut().zip(loaded.tiers).enumerate() {
        for (slot, old) in local.iter_mut().zip(persisted) {
            let Some(old) = old else { continue };
            match slot {
                Some(new) if new.at_ms == old.at_ms => {
                    if tier != 0 {
                        merge_bucket(new, &old);
                    }
                }
                Some(new) if new.at_ms > old.at_ms => {}
                _ => *slot = Some(old),
            }
        }
    }
}
fn merge_bucket(new: &mut Bucket, old: &Bucket) {
    new.n += old.n;
    for (new, old) in new.metrics.iter_mut().zip(old.metrics) {
        if old.n == 0 {
            continue;
        }
        let n = new.n + old.n;
        new.avg = Some(
            (new.avg.unwrap_or(0.0) * new.n as f64 + old.avg.unwrap_or(0.0) * old.n as f64)
                / n as f64,
        );
        new.max = match (new.max, old.max) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        };
        new.n = n;
    }
}

#[cfg(test)]
mod tests;
