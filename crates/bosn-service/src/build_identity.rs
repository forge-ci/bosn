//! The release + source size/mtime identity used by the daemon install stamp.
use std::path::Path;

/// A daemon executable build, using the existing install-stamp representation.
#[derive(Clone, PartialEq, Eq, prost::Message, serde::Serialize)]
pub struct BuildIdentity {
    /// Release followed by source size and modification time, as stamped by installation.
    #[prost(string, tag = "1")]
    pub stamp: String,
}

impl BuildIdentity {
    /// Identify an executable using the same metadata as its install stamp.
    pub fn from_executable(release: &str, source: &Path) -> std::io::Result<Self> {
        let meta = std::fs::metadata(source)?;
        let modified = meta
            .modified()?
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(std::io::Error::other)?
            .as_nanos();
        Ok(Self {
            stamp: format!("{release}\n{}:{modified}\n", meta.len()),
        })
    }

    /// Read an existing installation stamp. Legacy incomplete stamps have no identity.
    pub fn from_stamp(stamp: String) -> Option<Self> {
        let mut lines = stamp.lines();
        let release = lines.next()?;
        let (size, modified) = lines.next()?.split_once(':')?;
        if release.is_empty()
            || size.parse::<u64>().is_err()
            || modified.parse::<u128>().is_err()
            || lines.next().is_some()
        {
            return None;
        }
        Some(Self { stamp })
    }
}
