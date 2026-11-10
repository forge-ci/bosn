//! Additive v6 extension: durable stack-volume seed completion, never a TTL lease.
use super::*;
use crate::cache_helper::{CacheHelperIntent, CacheHelperRole, CacheHelperState};

const TABLE: &str = "CREATE TABLE IF NOT EXISTS volume_seeds (destination TEXT PRIMARY KEY, source TEXT NOT NULL, nonce TEXT NOT NULL UNIQUE, state TEXT NOT NULL CHECK(state IN ('copying','complete')))";
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SeedState {
    Copying,
    Complete,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VolumeSeed {
    pub destination: String,
    pub source: String,
    pub nonce: String,
    pub state: SeedState,
}
impl VolumeSeed {
    pub fn protects(&self, volume: &str) -> bool {
        self.state == SeedState::Copying && (self.source == volume || self.destination == volume)
    }
}
pub(crate) fn initialize(connection: &Connection) -> Result<(), Error> {
    connection.execute(TABLE, &[])?;
    Ok(())
}
pub(crate) fn validate_names(source: &str, destination: &str) -> Result<(), Error> {
    for name in [source, destination] {
        if !name.starts_with("bosn-v-stack-") {
            return Err(Error::BadRow("seed volume scope"));
        }
        act::ActEngineCacheVolume {
            name: name.into(),
            target: "/volume".into(),
        }
        .validate()?;
    }
    if source == destination {
        return Err(Error::BadRow("seed source equals destination"));
    }
    Ok(())
}
fn read(connection: &Connection) -> Result<Vec<VolumeSeed>, Error> {
    // Read-only diagnostics of an older v6 registry must not initialize it.
    let present = connection.query(
        "SELECT name FROM sqlite_master WHERE type='table' AND name='volume_seeds'",
        &[],
        QueryLimits {
            max_rows: 1,
            max_bytes: 1024,
        },
    )?;
    if present.is_empty() {
        return Ok(Vec::new());
    }
    let rows = connection.query(
        "SELECT destination,source,nonce,state FROM volume_seeds ORDER BY destination",
        &[],
        QueryLimits {
            max_rows: 100_000,
            max_bytes: 64 * 1024 * 1024,
        },
    )?;
    rows.iter()
        .map(|row| {
            let state = match text(row, 3)?.as_str() {
                "copying" => SeedState::Copying,
                "complete" => SeedState::Complete,
                _ => return Err(Error::BadRow("volume seed state")),
            };
            Ok(VolumeSeed {
                destination: text(row, 0)?,
                source: text(row, 1)?,
                nonce: text(row, 2)?,
                state,
            })
        })
        .collect()
}
impl Registry {
    pub fn volume_seeds(&self) -> Result<Vec<VolumeSeed>, Error> {
        read(&self.connection)
    }
}
impl ReadOnlyRegistry {
    pub fn volume_seeds(&self) -> Result<Vec<VolumeSeed>, Error> {
        read(&self.connection)
    }
}
impl Immediate<'_> {
    pub fn begin_volume_seed(
        &mut self,
        seed: &VolumeSeed,
        helper: &CacheHelperIntent,
    ) -> Result<(), Error> {
        validate_names(&seed.source, &seed.destination)?;
        if seed.state != SeedState::Copying
            || seed.nonce != helper.nonce
            || helper.role
                != Some(CacheHelperRole::VolumeSeedV1 {
                    source: seed.source.clone(),
                    destination: seed.destination.clone(),
                })
        {
            return Err(Error::BadRow("volume seed helper binding"));
        }
        self.begin_cache_helper(helper)?;
        self.transaction.execute(
            "INSERT INTO volume_seeds(destination,source,nonce,state) VALUES(?,?,?,'copying')",
            &[
                Value::Text(seed.destination.clone()),
                Value::Text(seed.source.clone()),
                Value::Text(seed.nonce.clone()),
            ],
        )?;
        Ok(())
    }
    pub fn complete_volume_seed(&mut self, seed: &VolumeSeed) -> Result<(), Error> {
        let rows = self.transaction.query(
            "SELECT source,nonce FROM volume_seeds WHERE destination=?",
            &[Value::Text(seed.destination.clone())],
            QueryLimits {
                max_rows: 1,
                max_bytes: 4096,
            },
        )?;
        let row = rows
            .first()
            .ok_or(Error::BadRow("seed completion record missing"))?;
        if text(row, 0)? != seed.source || text(row, 1)? != seed.nonce {
            return Err(Error::BadRow("seed completion identity"));
        }
        let helper = self
            .helper_record(&seed.nonce)?
            .ok_or(Error::BadRow("seed helper missing"))?;
        if helper.state != CacheHelperState::Removed {
            return Err(Error::BadRow("seed helper not settled"));
        }
        self.transaction.execute("UPDATE volume_seeds SET state='complete' WHERE destination=? AND source=? AND nonce=? AND state='copying'", &[Value::Text(seed.destination.clone()),Value::Text(seed.source.clone()),Value::Text(seed.nonce.clone())])?;
        Ok(())
    }
    pub fn delete_volume_seed(&mut self, destination: &str, nonce: &str) -> Result<(), Error> {
        let rows = self.transaction.query(
            "SELECT state FROM volume_seeds WHERE destination=? AND nonce=?",
            &[Value::Text(destination.into()), Value::Text(nonce.into())],
            QueryLimits {
                max_rows: 1,
                max_bytes: 1024,
            },
        )?;
        if rows
            .first()
            .is_some_and(|row| text(row, 0).is_ok_and(|state| state == "copying"))
        {
            let helper = self
                .helper_record(nonce)?
                .ok_or(Error::BadRow("seed helper missing"))?;
            if helper.state != CacheHelperState::Removed {
                return Err(Error::BadRow("seed helper not settled"));
            }
        }
        self.transaction.execute(
            "DELETE FROM volume_seeds WHERE destination=? AND nonce=?",
            &[Value::Text(destination.into()), Value::Text(nonce.into())],
        )?;
        Ok(())
    }
}
#[cfg(test)]
mod tests;
