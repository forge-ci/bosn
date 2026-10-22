//! Frozen helper process and mount profiles used during cleanup/adoption.
use super::{Config, Helper, HostConfig, Mount};
use bosn_registry::cache_helper::{CacheHelperIntent, CacheHelperRole};
use std::collections::BTreeMap;

pub(super) enum Profile {
    Measurement,
    Maintenance,
    Recovery {
        cache: String,
        source: String,
        source_mountpoint: Option<String>,
        command: String,
    },
    Release {
        cache: String,
        command: String,
    },
    Disposal {
        cache: String,
        command: String,
    },
}

pub(in crate::ci::engine) fn command_sha256(
    entrypoint: &[String],
    arguments: &[String],
) -> Result<String, String> {
    let encoded =
        serde_json::to_vec(&(entrypoint, arguments)).map_err(|error| error.to_string())?;
    Ok(kernal_api::hash::sha256_bytes(&encoded).to_hex())
}

impl Profile {
    pub(super) fn from_intent(intent: &CacheHelperIntent) -> Result<Self, String> {
        match &intent.role {
            None => Ok(Self::Measurement),
            Some(CacheHelperRole::MaintenanceV1) => Ok(Self::Maintenance),
            Some(CacheHelperRole::ToolRecoveryV1 {
                intent: frozen,
                command_sha256,
                source_mountpoint,
                ..
            }) => Ok(Self::Recovery {
                cache: intent.volume.clone(),
                source: frozen.source_volume.clone(),
                source_mountpoint: source_mountpoint.clone(),
                command: command_sha256
                    .clone()
                    .ok_or("recovery helper command binding missing")?,
            }),
            Some(CacheHelperRole::ToolRecoveryReleaseV1 { command_sha256, .. }) => {
                Ok(Self::Release {
                    cache: intent.volume.clone(),
                    command: command_sha256.clone(),
                })
            }
            Some(CacheHelperRole::ToolRecoveryDisposalV1 { command_sha256, .. }) => {
                Ok(Self::Disposal {
                    cache: intent.volume.clone(),
                    command: command_sha256.clone(),
                })
            }
        }
    }
    pub(super) fn label_key(&self) -> &str {
        match self {
            Self::Measurement => "io.bosn.cache.measurement",
            Self::Maintenance => "io.bosn.cache.maintenance",
            Self::Recovery { .. } => "io.bosn.cache.recovery",
            Self::Release { .. } => "io.bosn.cache.recovery-release",
            Self::Disposal { .. } => "io.bosn.cache.recovery-disposal",
        }
    }
    pub(super) fn component(&self) -> &str {
        match self {
            Self::Measurement => "ci-cache-measurement",
            Self::Maintenance => "ci-cache-maintenance",
            Self::Recovery { .. } => "ci-cache-recovery",
            Self::Release { .. } => "ci-cache-recovery-release",
            Self::Disposal { .. } => "ci-cache-recovery-disposal",
        }
    }
    pub(super) fn matches(&self, row: &Helper, volume: &str) -> bool {
        match self {
            Self::Measurement => {
                !row.host_config.privileged && one_mount(&row.mounts, volume, "/cache", false)
            }
            Self::Maintenance => {
                !row.host_config.privileged && one_mount(&row.mounts, volume, "/bosn/cache", true)
            }
            Self::Recovery {
                cache,
                source,
                source_mountpoint,
                command,
            } => {
                cache == volume
                    && row.mounts.len() == 2
                    && exact_mount(&row.mounts, cache, "/bosn/cache")
                    && match source_mountpoint {
                        Some(path) => exact_source_bind(&row.mounts, path),
                        None => exact_mount(&row.mounts, source, "/var/lib/docker"),
                    }
                    && row.host_config.privileged
                    && recovery_isolation(&row.host_config, false)
                    && frozen_command(&row.config, command)
            }
            Self::Release { cache, command } | Self::Disposal { cache, command } => {
                cache == volume
                    && row.mounts.len() == 1
                    && exact_mount(&row.mounts, cache, "/bosn/cache")
                    && !row.host_config.privileged
                    && row.host_config.cap_drop.as_deref() == Some(&["ALL".into()])
                    && recovery_isolation(&row.host_config, true)
                    && frozen_command(&row.config, command)
            }
        }
    }
}
fn one_mount(mounts: &[Mount], volume: &str, target: &str, writable: bool) -> bool {
    let [mount] = mounts else {
        return false;
    };
    mount.kind == "volume"
        && mount.name == volume
        && mount.destination == target
        && mount.rw == writable
}
fn exact_mount(mounts: &[Mount], volume: &str, target: &str) -> bool {
    mounts
        .iter()
        .filter(|mount| {
            mount.kind == "volume"
                && mount.name == volume
                && mount.destination == target
                && mount.rw
                && mount.driver.as_deref() == Some("local")
        })
        .count()
        == 1
}
fn frozen_command(config: &Config, expected: &str) -> bool {
    let (Some(entrypoint), Some(arguments)) = (&config.entrypoint, &config.arguments) else {
        return false;
    };
    entrypoint.as_slice() == ["/bin/sh"]
        && arguments.len() == 2
        && arguments[0] == "-ec"
        && command_sha256(entrypoint, arguments).is_ok_and(|hash| hash == expected)
}

fn recovery_isolation(host: &HostConfig, release: bool) -> bool {
    let mut tmpfs = BTreeMap::from([("/tmp".into(), "exec,size=64m".into())]);
    if release {
        tmpfs.insert("/var/lib/docker".into(), "ro,noexec,size=1m".into());
    }
    host.memory == Some(512 << 20)
        && host.nano_cpus == Some(2_000_000_000)
        && host.pids_limit == Some(128)
        && host.cpu_shares.is_some_and(|shares| {
            format!("--cpu-shares={shares}") == bosn_engine::LOW_CPU_PRIORITY_ARG
        })
        && host.tmpfs.as_ref() == Some(&tmpfs)
        && host.init == Some(false)
        && host.pid_mode.as_deref() == Some("")
        && host.auto_remove == Some(true)
        && host
            .restart_policy
            .as_ref()
            .is_some_and(|policy| policy.name == "no" && policy.maximum_retry_count == 0)
}

// The kernel needs writable backing upper/work for mounting; the producer
// separately verifies its recovered overlay view is read-only.
fn exact_source_bind(mounts: &[Mount], source: &str) -> bool {
    mounts
        .iter()
        .filter(|mount| {
            mount.kind == "bind"
                && mount.name.is_empty()
                && mount.source.as_deref() == Some(source)
                && mount.destination == "/var/lib/docker"
                && mount.rw
                && mount.driver.is_none()
        })
        .count()
        == 1
}
