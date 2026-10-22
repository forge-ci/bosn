//! Offline extraction into the helper's private tmpfs; the cache stays read-only.
use super::super::{ACT_VERSION, ActArtifact};

pub(super) fn program(act: ActArtifact) -> String {
    format!(
        r#"set -eu
bin=/var/lib/docker/bosn-cache-reader
mkdir -p "$bin"
exec 9< '/cache/tools/act-{version}-{archive}.tgz'
echo '{archive}  /proc/self/fd/9' | sha256sum -c - >/dev/null
tar -xzf /proc/self/fd/9 -C "$bin" act
test -f "$bin/act" && test ! -L "$bin/act"
echo '{binary}  /var/lib/docker/bosn-cache-reader/act' | sha256sum -c - >/dev/null
exec 9<&-
status=0
"$bin/act" cache usage --cache-server-path /cache --max-entries 1000000 || status=$?
# A typed partial receipt is still useful; the Rust boundary validates it.
test "$status" -eq 0 || test "$status" -eq 1
"#,
        version = ACT_VERSION,
        archive = act.sha256,
        binary = act.binary_sha256,
    )
}
