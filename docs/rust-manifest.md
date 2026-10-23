# Pure Rust manifest and policy scope

`bosn-core` now parses supplied TOML text into typed stack/task declarations and resolves
policy from caller-supplied values. It does not open files, inspect paths, read environment,
observe CPUs/clocks, download URLs, or invoke a process.

`ManifestRoots` deliberately keeps three distinct opaque inputs: document source/provenance,
build-asset/materialization root, and workspace bind root. Bind sources and Dockerfile paths
remain unresolved spellings for a future kernel resolver; lexical normalization here only
protects container destinations and is not represented as a symlink-safety guarantee.

The supported manifest subset is the existing `stack`/`task` TOML shape: image or Dockerfile,
family/default selection, volumes/scopes/retention, mounts/read-only, tmpfs, env/workdir, and
the macOS guest declaration including explicit license acknowledgement. Digesting Dockerfiles,
COPY context, remote assets, setup-document inline Dockerfiles/companions, cache/apply, and all
filesystem/process work are intentionally deferred.

### Task-container keys (#666)

These keys are parsed into typed values; slice 1 of #666 carries them into
the plan, and later slices act on them.

| Key | Values | Default | Notes |
| --- | --- | --- | --- |
| `[stack.X.mounts.NAME] mode` | `"rw"`, `"ro"`, `"overlay"` (`MountMode`) | `"rw"` | `readonly = true` is an alias for `"ro"`; both given and disagreeing is an error. |
| `[stack.X.mounts.NAME] upper` | `"tmpfs"` or the name of a volume of this stack (`OverlayUpper`) | `"tmpfs"` for an overlay | Valid only with `mode = "overlay"`; a named upper must be a declared `[stack.X.volumes]` entry. |
| `[stack.X.volumes.NAME] mode` | `"exclusive"`, `"shared"` (`VolumeMode`) | `exclusive` for `scope = "spec"`/`"stack"`, `shared` for `"machine"` | Whether runs of different names may mount the volume at once. |
| `[stack.X] default_target_volume` | boolean | `true` | `false` suppresses the synthesized `target` build-output volume. |
| `[task.X] keep` | boolean | `false` | `true` keeps today's long-lived container instead of an ephemeral one per run. |

There is no machine-policy file. A `MachinePolicy`/`AppPolicy` resolver with a `[policy]` TOML
surface once lived in `bosn_core::config`, but no production code ever called it, so every key
it validated was inert; it was retired (#527). Retention's one configuration surface is the
state directory's `retention.toml` (`auto_retention = false` opts out); its thresholds are the
documented constants of the retention planner (AGENTS.md, "Automatic retention").

## Test evidence

The original implementation did not retain a pre-implementation RED test command; its only
recorded intermediate failure was Rust `E0382` (a moved `kind` value), which is not evidence of
missing behavior. The review corrections were RED first: the newly added separate manifest and
policy tests made `soldr cargo test --workspace --locked` fail because the old API had only two
manifest roots and lacked `resolve_machine_policy`/`resolve_app_policy`. After implementing the
three-root and split-policy APIs, the same command is GREEN. The final gate is also
`soldr cargo fmt --all -- --check`, `soldr cargo clippy --workspace --all-targets -- -D warnings`,
and `git diff --check`.

Final local result: 19 tests passed (13 domain, 4 manifest, 2 config), with no failures.
Formatting, warnings-denied Clippy, and the diff check also passed. The Python reference
manifest/config tests passed 64 cases, with one Windows-only drive-path test skipped on Linux.
