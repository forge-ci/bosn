//! Operator-owned console targets. Network destinations never come from a page.
use std::{collections::BTreeSet, io::Read, path::Path};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub certificate_sha256: String,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Catalog {
    #[serde(default)]
    servers: Vec<Target>,
}

/// Only these display fields leave the daemon. No credential or trust material.
#[derive(Serialize)]
pub struct Summary<'a> {
    pub id: &'a str,
    pub name: &'a str,
}

impl Target {
    fn validate(mut self) -> Result<Self, String> {
        if !valid_id(&self.id) || self.name.trim().is_empty() || self.name.len() > 128 {
            return Err("server needs an id (letters, digits, '-' or '_') and a name".into());
        }
        if self.port == 0
            || self.host.is_empty()
            || self.host.len() > 253
            || !self
                .host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".-:".contains(&b))
        {
            return Err(format!("server {} has an invalid host or port", self.id));
        }
        if self.certificate_sha256.len() != 64
            || !self
                .certificate_sha256
                .bytes()
                .all(|b| b.is_ascii_hexdigit())
        {
            return Err(format!(
                "server {} needs a 64-digit certificate SHA-256",
                self.id
            ));
        }
        self.certificate_sha256.make_ascii_lowercase();
        Ok(self)
    }

    pub fn summary(&self) -> Summary<'_> {
        Summary {
            id: &self.id,
            name: &self.name,
        }
    }
}

pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}

/// Read on the blocking lane; cap allocation before deserializing local input.
pub fn load(state: &Path) -> Result<Vec<Target>, String> {
    let path = state.join("servers.toml");
    let file = match std::fs::File::open(&path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let mut text = String::new();
    file.take(65537)
        .read_to_string(&mut text)
        .map_err(|e| e.to_string())?;
    if text.len() > 65536 {
        return Err("servers.toml exceeds 64 KiB".into());
    }
    let catalog: Catalog = toml::from_str(&text).map_err(|e| format!("servers.toml: {e}"))?;
    let mut ids = BTreeSet::new();
    catalog
        .servers
        .into_iter()
        .map(|target| {
            let target = target.validate()?;
            if !ids.insert(target.id.clone()) {
                return Err(format!("duplicate server id: {}", target.id));
            }
            Ok(target)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_destinations_that_could_be_urls_options_or_paths() {
        let base = Target {
            id: "rack_1".into(),
            name: "Rack 1".into(),
            host: "192.0.2.1".into(),
            port: 5901,
            certificate_sha256: "AB".repeat(32),
        };
        assert_eq!(
            base.clone().validate().unwrap().certificate_sha256,
            "ab".repeat(32)
        );
        for host in [
            "https://example.com",
            "x/y",
            "x\n--privileged",
            "",
            "host name",
        ] {
            assert!(
                Target {
                    host: host.into(),
                    ..base.clone()
                }
                .validate()
                .is_err()
            );
        }
        for id in ["../rack", "rack/name", "", "rack%2fother"] {
            assert!(!valid_id(id));
        }
        assert!(
            Target {
                certificate_sha256: "bad".into(),
                ..base
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn missing_configuration_is_empty_but_duplicate_ids_are_errors() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load(dir.path()).unwrap().is_empty());
        let entry = format!(
            "[[servers]]\nid='rack'\nname='Rack'\nhost='192.0.2.1'\nport=5901\ncertificate_sha256='{}'\n",
            "ab".repeat(32)
        );
        std::fs::write(dir.path().join("servers.toml"), entry.repeat(2)).unwrap();
        assert!(load(dir.path()).unwrap_err().contains("duplicate"));
    }
}
