//! Independent legacy du oracle on small private test trees. Production never
//! executes this sampler; physical helper fixtures compare the new reader to it.
use super::*;
use std::collections::BTreeSet;
const MAX_ERRORS: usize = 16;
pub(super) const SCRIPT: &str = include_str!("reference.sh");

pub(super) fn parse(volume: &str, output: &str) -> CacheUsage {
    let mut report = CacheUsage {
        volume: volume.into(),
        ..CacheUsage::default()
    };
    let mut seen = BTreeSet::new();
    for line in output.lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() != 3 {
            measurement_error(&mut report, "invalid cache measurement".into());
            continue;
        }
        let identity = component_identity(fields[0]);
        if fields[0] != "total" && identity.is_none() {
            measurement_error(&mut report, "unrecognized cache component identity".into());
            continue;
        }
        if !seen.insert(fields[0].to_string()) {
            measurement_error(&mut report, "duplicate cache measurement".into());
            continue;
        }
        let (bytes, allocated_bytes) =
            (fields[1].parse::<u64>().ok(), fields[2].parse::<u64>().ok());
        if bytes.is_none() || allocated_bytes.is_none() {
            measurement_error(&mut report, format!("{}: size is unknown", fields[0]));
        }
        if fields[0] == "total" {
            report.bytes = bytes;
            report.allocated_bytes = allocated_bytes;
        } else if let Some((class, namespace, store_path)) = identity {
            report.components.push(CacheComponent {
                class,
                namespace,
                store_path,
                bytes,
                allocated_bytes,
            });
        }
    }
    for key in std::iter::once("total").chain(CLASSES.iter().map(|class| class.as_str())) {
        if !seen.contains(key) {
            measurement_error(&mut report, format!("{key}: measurement missing"));
        }
    }
    bound_components(&mut report);
    report.partial = !report.errors.is_empty();
    report
}

fn measurement_error(report: &mut CacheUsage, message: String) {
    if report.errors.len() < MAX_ERRORS {
        report.errors.push(message);
    } else {
        report.errors[MAX_ERRORS - 1] = "additional measurement errors omitted".into();
    }
}

fn bound_components(report: &mut CacheUsage) {
    let classes = report
        .components
        .iter()
        .filter(|row| row.namespace.is_none())
        .count();
    let namespaces = report.components.len() - classes;
    if namespaces <= MAX_NAMESPACES {
        return;
    }
    report.components.sort_by(|a, b| {
        a.namespace
            .is_some()
            .cmp(&b.namespace.is_some())
            .then_with(|| {
                // Unknown entries stay visible instead of masquerading as small stores.
                b.allocated_bytes
                    .or(b.bytes)
                    .unwrap_or(u64::MAX)
                    .cmp(&a.allocated_bytes.or(a.bytes).unwrap_or(u64::MAX))
            })
            .then_with(|| a.store_path.cmp(&b.store_path))
    });
    report.components.truncate(classes + MAX_NAMESPACES);
    measurement_error(
        report,
        format!(
            "{} namespace details omitted; retained the {MAX_NAMESPACES} largest or unknown stores",
            namespaces - MAX_NAMESPACES
        ),
    );
}

fn component_identity(key: &str) -> Option<(CacheClass, Option<String>, Option<String>)> {
    let store = key
        .strip_prefix("namespace:")
        .map(|namespace| (namespace, "actcache"))
        .or_else(|| {
            key.strip_prefix("cohort-v1:")
                .map(|namespace| (namespace, "actcache/cohort-v1"))
        });
    if let Some((namespace, root)) = store {
        return (namespace.len() == 16
            && namespace
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()))
        .then(|| {
            (
                CacheClass::Actcache,
                Some(namespace.into()),
                Some(format!("{root}/{namespace}")),
            )
        });
    }
    CLASSES
        .iter()
        .find(|class| class.as_str() == key)
        .map(|class| (*class, None, None))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_cache_explains_components_and_repository_stores() {
        let report = parse(
            "test",
            "total 10000 8192\ntools 100 4096\nimages 500 4096\nactions 100 4096\ntoolcache 100 4096\nactcache 8000 4096\nnamespace:0123456789abcdef 7000 4096\n",
        );
        assert_eq!(report.bytes, Some(10000));
        assert_eq!(report.allocated_bytes, Some(8192));
        assert_eq!(report.components.len(), 6);
        assert_eq!(
            report.components[5].namespace.as_deref(),
            Some("0123456789abcdef")
        );
        assert!(!report.partial);
    }

    #[test]
    fn machine_cache_breakdown_stays_within_the_tool_reply_limit() {
        let mut sample = String::from(
            "total 10000 8192\ntools 0 0\nimages 0 0\nactions 0 0\ntoolcache 0 0\nactcache 10000 8192\n",
        );
        for index in 0..1000 {
            sample.push_str(&format!("namespace:{index:016x} {index} {index}\n"));
        }
        for _ in 0..1000 {
            sample.push_str("invalid duplicate record\n");
        }
        let report = parse("test", &sample);
        assert!(serde_json::to_vec(&report).unwrap().len() < 60 * 1024);
        assert!(report.partial);
        assert_eq!(report.allocated_bytes, Some(8192));
        assert!(
            report
                .components
                .iter()
                .any(|component| component.namespace.as_deref() == Some("00000000000003e7"))
        );
    }

    #[test]
    fn incomplete_measurement_is_never_an_empty_cache() {
        let report = parse("test", "total unknown unknown\ntools 100 4096\n");
        assert!(report.partial);
        assert_eq!(report.bytes, None);
        assert!(!report.errors.is_empty());
    }

    #[test]
    fn docker_outages_are_not_volume_absence() {
        assert_eq!(
            volume_present(false, b"Error response from daemon: no such volume: test"),
            Ok(false)
        );
        assert!(volume_present(false, b"Cannot connect to the Docker daemon").is_err());
        assert!(volume_present(false, b"permission denied").is_err());
        assert!(volume_present(false, b"").is_err());
    }

    #[test]
    fn malformed_and_duplicate_component_samples_stay_partial() {
        for output in [
            "total 1 2\ntotal 3 4",
            "namespace:../../foreign 1 2",
            "cohort-v1:../../foreign 1 2",
            "cohort-v1:0123456789abcdeF 1 2",
            "tools 1 2 3",
            "tools overflow 2",
        ] {
            assert!(parse("test", output).partial, "{output}");
        }
    }

    #[test]
    fn real_measurement_distinguishes_sparse_lengths_from_disk_blocks() {
        let root = kernal_api::platform::fs::TemporaryDirectory::new().unwrap();
        let tools = root.path().join("tools");
        std::fs::create_dir(&tools).unwrap();
        std::fs::File::create(tools.join("sparse"))
            .unwrap()
            .set_len(1 << 20)
            .unwrap();
        let namespace = root.path().join("actcache/0123456789abcdef");
        std::fs::create_dir_all(&namespace).unwrap();
        std::fs::write(namespace.join("archive"), "cached").unwrap();
        let imported = root.path().join("actcache/cohort-v1/0123456789abcdef");
        std::fs::create_dir_all(&imported).unwrap();
        std::fs::write(imported.join("archive"), "imported").unwrap();
        #[cfg(unix)]
        {
            let foreign = root.path().join("outside");
            std::fs::create_dir_all(foreign.join("fedcba9876543210")).unwrap();
            std::os::unix::fs::symlink(&foreign, root.path().join("actcache/1111111111111111"))
                .unwrap();
        }
        let script = SCRIPT.replace("root=/cache", &format!("root='{}'", root.path().display()));
        let output = kernal_api::run_bounded_command(
            kernal_api::SpawnSpec::new("sh")
                .arg("-c")
                .arg(script)
                .stdin(kernal_api::StreamMode::Null)
                .stdout(kernal_api::StreamMode::Piped)
                .stderr(kernal_api::StreamMode::Piped),
            std::time::Duration::from_secs(10),
            1 << 16,
        )
        .unwrap();
        assert_eq!(output.exit.raw_code(), 0);
        let report = parse("test", &String::from_utf8_lossy(&output.stdout));
        assert!(!report.partial, "{report:?}");
        assert_eq!(
            report
                .components
                .iter()
                .filter(|c| c.namespace.is_some())
                .count(),
            2,
            "retained legacy and imported stores must remain separately visible"
        );
        let stores: BTreeSet<_> = report
            .components
            .iter()
            .filter_map(|c| c.store_path.as_deref())
            .collect();
        assert_eq!(
            stores,
            BTreeSet::from([
                "actcache/0123456789abcdef",
                "actcache/cohort-v1/0123456789abcdef"
            ])
        );
        assert!(report.bytes.unwrap() > report.allocated_bytes.unwrap());
        assert!(
            report.components.iter().any(
                |c| c.namespace.as_deref() == Some("0123456789abcdef") && c.bytes.unwrap() >= 6
            )
        );
        assert!(
            report
                .components
                .iter()
                .any(|c| c.class == CacheClass::Images && c.bytes == Some(0))
        );
    }
}
