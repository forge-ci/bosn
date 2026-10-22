//! Typed read-only act2 footprint receipts; none of these fields authorize removal.
use super::{CLASSES, CacheClass, CacheComponent, CacheUsage, MAX_NAMESPACES};
use serde::Deserialize;
use std::collections::BTreeSet;

const MAX_ENTRIES: u64 = 1_000_000;
const MAX_REPORT: usize = 256 << 10;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Usage {
    filesystem_device: Option<u64>,
    path_entries: u64,
    unique_inodes: u64,
    apparent_bytes: Option<u64>,
    allocated_bytes: Option<u64>,
    unique_file_bytes: Option<u64>,
    referenced_file_bytes: Option<u64>,
}

impl Usage {
    fn validate(&self, partial: bool) -> Result<(), String> {
        if self.path_entries > MAX_ENTRIES
            || self.unique_inodes > self.path_entries
            || (self.path_entries > 0 && self.filesystem_device.is_none())
        {
            return Err("cache footprint inode counts are invalid".into());
        }
        match (
            self.apparent_bytes,
            self.allocated_bytes,
            self.unique_file_bytes,
            self.referenced_file_bytes,
        ) {
            (None, None, None, None) if partial => Ok(()),
            (Some(apparent), Some(allocated), Some(unique), Some(references))
                if !partial
                    && apparent >= unique
                    && references >= unique
                    && allocated % 512 == 0 =>
            {
                Ok(())
            }
            _ => Err("cache footprint byte totals contradict completeness".into()),
        }
    }
}

#[derive(Clone, Copy, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
enum Class {
    Tools,
    Images,
    Actions,
    Toolcache,
    Actcache,
    Other,
}

impl Class {
    fn known(self) -> Option<CacheClass> {
        match self {
            Self::Tools => Some(CacheClass::Tools),
            Self::Images => Some(CacheClass::Images),
            Self::Actions => Some(CacheClass::Actions),
            Self::Toolcache => Some(CacheClass::Toolcache),
            Self::Actcache => Some(CacheClass::Actcache),
            Self::Other => None,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Component {
    class: Class,
    usage: Usage,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Namespace {
    namespace: String,
    store_path: String,
    usage: Usage,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Report {
    schema_version: u32,
    root: String,
    observed_at: String,
    total: Usage,
    components: Vec<Component>,
    namespaces: Option<Vec<Namespace>>,
    omitted_namespaces: u64,
    partial: bool,
    #[serde(default)]
    error: String,
}

pub(super) fn parse(volume: &str, text: &str) -> Result<CacheUsage, String> {
    if text.len() > MAX_REPORT {
        return Err("cache footprint report exceeds bound".into());
    }
    let report: Report =
        serde_json::from_str(text).map_err(|error| format!("cache footprint receipt: {error}"))?;
    if report.schema_version != 1
        || report.root != "/cache"
        || report.observed_at.len() > 64
        || time::OffsetDateTime::parse(
            &report.observed_at,
            &time::format_description::well_known::Rfc3339,
        )
        .is_err()
        || report.components.len() != CLASSES.len() + 1
        || report.omitted_namespaces > MAX_ENTRIES
        || report.error.chars().count() > 256
        || report.partial == report.error.is_empty()
    {
        return Err("cache footprint identity or completeness is invalid".into());
    }
    report.total.validate(report.partial)?;
    let mut result = CacheUsage {
        volume: volume.into(),
        bytes: report.total.apparent_bytes,
        allocated_bytes: report.total.allocated_bytes,
        partial: report.partial,
        ..CacheUsage::default()
    };
    let mut classes = BTreeSet::new();
    for component in report.components {
        if !classes.insert(component.class) {
            return Err("cache footprint class is duplicated".into());
        }
        component.usage.validate(report.partial)?;
        if let Some(class) = component.class.known() {
            result.components.push(CacheComponent {
                class,
                namespace: None,
                store_path: None,
                bytes: component.usage.apparent_bytes,
                allocated_bytes: component.usage.allocated_bytes,
            });
        }
    }
    let namespaces = report.namespaces.unwrap_or_default();
    if namespaces.len() > MAX_NAMESPACES || (report.partial && !namespaces.is_empty()) {
        return Err("cache footprint namespace details exceed bound or are incomplete".into());
    }
    let mut paths = BTreeSet::new();
    for namespace in namespaces {
        namespace.usage.validate(false)?;
        let component = namespace_component(namespace)?;
        if !paths.insert(component.store_path.clone()) {
            return Err("cache footprint namespace is duplicated".into());
        }
        result.components.push(component);
    }
    if report.partial {
        result.errors.push(report.error);
    }
    if report.omitted_namespaces > 0 {
        result.partial = true;
        result.errors.push(format!(
            "{} namespace details omitted; total includes every scanned entry",
            report.omitted_namespaces
        ));
    }
    Ok(result)
}

fn namespace_component(namespace: Namespace) -> Result<CacheComponent, String> {
    let path = namespace.store_path;
    if namespace.namespace.len() != 16
        || !namespace
            .namespace
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        || ![
            format!("actcache/{}", namespace.namespace),
            format!("actcache/cohort-v1/{}", namespace.namespace),
        ]
        .contains(&path)
    {
        return Err("cache footprint namespace identity is invalid".into());
    }
    Ok(CacheComponent {
        class: CacheClass::Actcache,
        namespace: Some(namespace.namespace),
        store_path: Some(path),
        bytes: namespace.usage.apparent_bytes,
        allocated_bytes: namespace.usage.allocated_bytes,
    })
}
