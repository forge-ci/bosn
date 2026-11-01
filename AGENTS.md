# Agent notes for Bosn

Settled decisions, recorded so they are not re-litigated. Change them only with an
explicit new decision, not by inference.

## CI and release gate (staged rollout)

Ordinary PRs and `main` pushes run minimal CI; the literal `ci-test` PR label
adds Linux lint, unit, and Docker tests. The literal `ci-full` PR label runs the
full platform matrix, including hosted Intel and Apple Silicon macOS smoke
checks. One path rule sits beside the labels: a PR or `main` push that changes
`crates/bosn-widget/**` (its own Cargo workspace, which no other lane builds)
runs the required `Desktop widget (Linux, WebKitGTK)` job, selected by
`ci/select_ci_tier.py`; `ci-widget` forces it otherwise. Before **any** tag or release, full CI must pass
on the exact candidate commit SHA. See [CI tiers and queue cost](docs/ci-queue-slo.md) and the
[fleet rollout issue](https://github.com/zackees/soldr/issues/3345).

**One required check: `CI summary` (decided 2026-11-01, #803).** Classic branch
protection on `main` requires only `CI summary` (`ci/ci_summary.py`); the merge-queue
ruleset carries no `required_status_checks` rule, so the check is declared in exactly
one place. The job reads its run attempt's full job list and fails on any failed,
cancelled, timed-out, startup-failed, action-required or unfinished job outside its
exact-name `EXEMPT` list (bookkeeping, timing, janitors, build-only cache writers).
A new lane is enforced by existing in `ci.yml` and in `CI summary`'s `needs:`; never
add it to branch protection, and never add a validating job to `EXEMPT`.

Releases go only through the **pretag release gate** (see "Releasing" below):
an explicit request for an exact candidate SHA on `main`, refused unless full
CI already passed on that SHA. Nothing tags or publishes on a version bump or a
pushed tag; never create a release tag by hand.

## act2 dependency release authorization

Act2 releases are always authorized when necessary to implement or ship Bosn
work. Proceed without asking for additional release approval. Before tagging,
require full act2 CI to pass on the exact candidate commit SHA, use a clean
checkout of act2's default branch, and verify that the tag and release do not
already exist. Publish through act2's existing tag-triggered release workflow.
Verify the resulting binaries and checksums before updating Bosn's pinned
version and artifact digests. Record release evidence and rollout status in the
living implementation spec.

## Code rules (hard gates in `./lint`)

- **Single responsibility and DRY.** Parse inputs eagerly into typed structs
  and enums at the boundary; never probe untyped JSON for fields.
- **Every Rust, Python and shell file stays under 1,000 lines**
  (`ci/lint_file_length.py`). Split by responsibility; never raise the limit.
- **Write a path once.** Within a file, a directory appears in at most one
  `include_str!`/`include_bytes!` path. Several files from one directory go
  through one base macro, e.g.
  `macro_rules! asset { ($n:literal) => { include_str!(concat!("assets/", $n)) }; }`,
  and a file used more than once is bound to one `const`
  (`ci/lint_include_base.py`). A `concat!` base must stay inside the crate's
  `src/`, because `ci/publish_amalgamate.py` only relocates literal includes.

## Python packaging

- **The extension is `abi3-py310`. Always. Only.** `crates/bosn-python/Cargo.toml`
  declares `pyo3/abi3-py310`; every platform wheel is tagged `cp310-abi3-*` and ships
  `bosn/_native.abi3.so` (`.pyd` on Windows). `requires-python = ">=3.10"`.
  - Do **not** build interpreter-specific wheels (`cp311-cp311`, etc.). One abi3 wheel
    per platform covers CPython 3.10+. This is the fleet-wide policy (soldr, fbuild,
    zccache, running-process all use `abi3-py310`); it is also what lets the macOS
    wheels cross-compile on Linux with no target-side CPython.
  - `ci/verify_installed_wheel.py` and `ci/verify_cross_wheel.py` assert the
    `cp310-abi3` tag and the `_native.abi3.so` / `.pyd` extension name. Keep them in
    lockstep with any change here.

- **Build backend: `bosn_build_backend.py` stays; the bin cannot ride on maturin.**
  Bosn's wheel needs **both** a PyO3 abi3 extension (`bosn._native`, used by
  `import bosn`) **and** the `bosn-native` CLI. maturin builds one artifact kind per
  wheel — a pyo3 extension **or** a `bindings="bin"` binary — and does **not** ship a
  bin alongside a pyo3 extension (verified by building it: bin un-disabled → wheel
  `scripts: []`). soldr is **not** a counterexample: soldr's wheel is `py3-none-*`,
  binary-only, with no extension, so it can be pure `build-backend = "soldr"`. Bosn
  cannot. So a custom PEP 517 backend that builds the CLI and stages it, then lets
  maturin build the extension, is the correct shape — do not "simplify" it away.
  - **The `bosn` command IS the native binary** (#262, done): the backend stages the
    CLI into the wheel's `.data/scripts/` (installed on PATH as `bosn`) — there is no
    `native_cli.py` launcher and no `[project.scripts]`. The Cargo bin stays
    `bosn-native` (bosn-service already owns a `bosn` bin; a second would collide on
    `target/<profile>/bosn`); the backend renames it to `bosn` while staging.
  - **One OpenSSL, statically linked (#265, decided).** Linux TLS comes from
    kernal-api's `http-client` (reqwest `default-tls`, i.e. OpenSSL; macOS uses
    Security.framework, Windows SChannel). `crates/bosn-python` enables
    `openssl-sys/vendored` on Linux, so the extension and the CLI each link OpenSSL
    statically: no `bosn.libs/` OpenSSL, no `.data/scripts/` sidecars, no `$ORIGIN`
    rpath. Both wheel verifiers reject any bundled `libssl`/`libcrypto`. An rpath into
    `bosn.libs/` was rejected (auditwheel hash-renames the sonames and does not repair
    scripts). rustls is the fleet's choice (soldr, zccache, fbuild) and the eventual
    target, but needs a TLS-backend feature in kernal-api first; when it lands, drop the
    vendored dependency (kernal-api#427). Dropping `bosn_build_backend.py` entirely still waits on
    **soldr#3239** (native aux-bin staging).

- **Test install profile (#600, decided 2026-10-09): `./install` builds a `dev`-profile
  wheel.** It exports `BOSN_WHEEL_PROFILE=dev` (unless already set) from
  `ci/dev_profile.sh`, which `./lint` and `./test` source too: the variable is a uv
  cache key, so a bare `uv run` without it rebuilt the wheel in release (#503). Both the
  `bosn-native` CLI and the extension build with Cargo's `dev` profile; the backend
  accepts only `release` (the default) or `dev`. This is the wheel the local gate and
  the `linux` lane test. The gate must check what ci.yml checks, and it still does: the
  `linux` job runs the same `./install` remotely. Release-profile wheels are built
  **only** by the `native-wheel`/Darwin lanes (full CI) and `auto-release.yml`, which run
  `uv build` without the variable and verify the result; that is where release
  optimisation is proven. Fleet precedent: soldr's own backend defaults to
  `--profile dev` for dev installs and builds release only for `soldr wheel --release`.
  Never set `BOSN_WHEEL_PROFILE` in a release or native-wheel lane.

## Releasing (the pretag release gate)

- **The version is written once**: `[workspace.package].version` in the root `Cargo.toml`
  (the zccache/soldr pattern). `bosn`, `bosn-python` and `bosn-service` inherit it, the wheel
  reads it through maturin (`dynamic = ["version"]`), `bosn.__version__` comes from the loaded
  extension, and `uv.lock` records the project as dynamic. `ci/verify_release.py` refuses any
  second copy. `bosn-service` builds the shipped `bosn` binary and daemon, so it must inherit:
  a pinned `0.1.0` there made every released CLI answer `bosn 0.1.0`. The other internal
  crates are never published, ship no artifacts, and keep their own versions.
- **Prepare the version by PR**: `./bump patch` (or `minor`, `major`, `X.Y.Z`) rewrites that
  one line and refreshes `Cargo.lock`'s two workspace entries. Merging it releases nothing.
- **Request the release for the exact merged SHA** (`<SHA>` = full 40-hex commit on `main`):
  1. Full CI on that SHA: `gh workflow run ci.yml -f tier=full -f commit_sha=<SHA>`. ci.yml
     names the run `CI full <SHA>`; wait for it to go green and note its run ID.
  2. Optional release-request issue (the control record): body lines
     `candidate_sha: <SHA>` and `tag: vX.Y.Z`. The release comments and closes it.
  3. Dry run, then publish:
     `gh workflow run auto-release.yml -f candidate_sha=<SHA> -f full_ci_run_id=<ID> [-f issue=<N>]`
     (dry by default: builds and verifies all four wheels, publishes nothing), then the same
     with `-f dry_run=false`.
- **What the gate enforces** (`ci/release_gate.py`, the guard job every other job needs):
  the SHA is full, lowercase, checked out, and reachable from `origin/main`; the CI run is a
  `workflow_dispatch` of `ci.yml` from `main` named `CI full <SHA>`, completed `success`, with
  every full-tier cell (`ci/verify_full_coverage.py`'s `REQUIRED`, hosted macOS smokes
  included) and `Full CI coverage` successful; tag `v<version>` is new or already names that
  SHA; a named issue is open and names the same SHA and tag. The tag is created only by the
  final `GitHub release` job (`gh release create --target <SHA>`), after PyPI.
- **Retry by dispatching the same SHA again**, never by bumping the version: every
  publishing step is idempotent (PyPI and GitHub assets are byte-compared, only missing
  files are uploaded). `auto-release.yml` has no push or tag trigger.
  `tests/test_auto_release_guard.py` holds the gate and that wiring.
- **A release is exactly four `cp310-abi3` wheels**: Linux x86_64 (`manylinux_2_39`, so glibc
  ≥ 2.39), Windows x86_64, and both Darwin targets. No sdist: building Bosn from source
  needs the Rust toolchain and the staging backend, so an unsupported platform should get
  "no matching distribution" rather than a failed compile.
- **PyPI uses trusted publishing** (OIDC), like kernal-api's crates.io release: no PyPI
  token is stored. It needs `vars.PUBLISH_PYPI == 'true'` and the pypi.org trusted
  publisher for `bosn`: repository `forge-ci/bosn`, workflow `auto-release.yml`,
  environment `pypi`. PyPI matches both names exactly: renaming the workflow file or the
  job's environment breaks publishing. Without the variable a tag still produces the
  GitHub release, and the run summary says PyPI was skipped.
- The build steps are **copies** of `ci.yml`'s wheel lanes, not a shared `workflow_call`:
  branch protection requires those job names verbatim. Change both together.

### crates.io: one amalgamated `bosn` crate

- **Only `bosn` is published.** `crates/bosn` is a facade that, in the workspace,
  re-exports the internal crates (`bosn::core`, `engine`, `generation`, `registry`,
  `setup`, `service`). `ci/publish_amalgamate.py` (zackees/zccache's approach) rewrites it
  into one self-contained crate: each internal crate's `src/` becomes a module, paths are
  rewritten, the `bosn` CLI binary is carried across, test fixtures reached by
  `include_str!` are relocated, and internal dependencies are stripped. Every internal crate
  and `bosn-python` (the PyO3 extension, PyPI only) is `publish = false`.
- **The script rewrites in place, so never run it on the working copy.** Publish by hand
  from a disposable worktree:

  ```bash
  git worktree add ../bosn-extern/bosn-publish vX.Y.Z
  cd ../bosn-extern/bosn-publish
  python ci/publish_amalgamate.py --root .
  cargo package -p bosn --allow-dirty          # builds the .crate in isolation
  cargo publish -p bosn --allow-dirty          # needs `cargo login`
  ```

- **Keep the facade's `[dependencies]` in step** with the internal crates' external
  dependencies: the amalgamated crate builds from crates.io alone, and the script refuses
  a copy that still names an internal crate.

## macOS (issue #252)

- **Hosted macOS runners are reserved for `ci-full` and release smoke checks.**
  `ci/lint_no_macos_runners.py` rejects macOS runner labels outside those gated jobs.
  Both `x86_64-apple-darwin` and `aarch64-apple-darwin` wheels
  build on `ubuntu-latest` through `forge-ci/setup-soldr` (floating `@v0`) +
  `soldr prepare --target …`, using Soldr's LLVM 21.1.5 and managed Apple SDK 14.5 —
  never Xcode, zig, or osxcross.
- **Linux verifies macOS wheels statically; full CI and release execute them on
  matching hosted Macs.**
  `ci/verify_cross_wheel.py` checks the Mach-O extension and bundled CLI (arch,
  filetype, min-OS ≤ tag floor, allowed system dylibs, no ELF/OpenSSL contamination),
  tag, and version alignment. The floors are 10.12 (x86_64) and 11.0 (arm64).
- **The legacy Recovery smoke remains advisory in `macos-x64-execute.yml`** — a macOS
  Recovery guest (`zackees/docker-mac-x64`, OSX-KVM on `ubuntu-latest`, no macOS
  runner). It is nightly + manual, never a required check or release gate. Both
  architectures now have hosted full CI and release smoke checks. See
  `docs/macos-guest.md` "Executing the wheel".

## Rust lane shape (#503, decided 2026-10-10)

The `rust` job compiles the workspace once per edit. Restructure it; never drop a check.

- **Clippy runs inside the test build.** `cargo clippy` is cargo with clippy-driver as
  `RUSTC_WORKSPACE_WRAPPER` and its `-- <args>` in `CLIPPY_ARGS`, so the `Clippy and test
  Rust workspace` step wraps `cargo nextest run --workspace --all-targets` the same way
  (`-D warnings`). It lints every target `clippy --all-targets` did (plus bosn-python) and
  compiles each crate once instead of a check build then a full rebuild. Doctests run in
  the same step with the same wrapper and features, so they reuse those library units.
- **One integration-test binary per crate.** bosn-core, bosn-registry and bosn-service set
  `autotests = false` and declare `tests/integration.rs`, whose modules are the former
  `tests/<name>.rs` files: an edit relinks one binary per crate, not one per file. Add a
  new integration test as a module there. Run one with
  `cargo test -p <crate> --test integration -- <module>::`.
- **bosn-python is tested in the workspace build.** Its wheel-only features (`wheel` =
  `extension-module` + vendored OpenSSL) are not crate defaults; `[tool.maturin] features`
  and the backend's CLI build select `wheel`. The test build selects
  `bosn-python/embedded-python-tests` and links CPython and system OpenSSL. Do not make
  vendored OpenSSL a default: its C build would serialise ~40 s ahead of every test build.
- **No lane-input narrowing for the Rust lane yet.** Rust tests read `docs/` (`ci.schema.json`,
  `macos-guest.md`), `bosn.toml` and `tests/fixtures/`, so a docs or fixture exclusion is
  unsafe without the per-path input evidence GATE-007 requires.
- **The tests lane excludes Rust test sources (decided 2026-10-10).** `local-gate.toml`'s
  `tests` lane excludes `crates/**/tests/**/*.rs`, `crates/**/tests.rs` and
  `crates/**/*_tests.rs`, so a test-only Rust edit reuses the lane's GATE-007 pass and does
  not rebuild the dev wheel. It is safe because those files are declared only behind
  `#[cfg(test)]` (or are integration-test targets) and never reach the wheel, and because
  the `rust` job also runs every tests-lane check that reads them (`lint_file_length`,
  `lint_include_base`, `test_publish_amalgamate`). `tests/test_tests_lane_inputs.py`
  enforces both; extend that list when a tests-lane check starts reading Rust sources.
  The rust lane keeps the whole tree. A non-test Rust edit still rebuilds the wheel: it
  is a real wheel input. No other lane exclusion until it has the same evidence.

## Toolchain note (host hazard)

Never install an exact old Soldr on the host (`uv tool install soldr==<x>`,
`pip install soldr==<x>`, or a PEP 723 `soldr==<x>` script dependency).
`~/.local/bin/soldr` and the `~/.soldr` cache root are shared by every
repository and session on the machine: a pinned install downgrades all of
them, and an old Soldr daemon can take over `~/.soldr` and lock newer daemons
out until it is killed. Use the latest release (`uv tool install soldr`, or
`uv tool upgrade soldr`). CI pins its own Soldr through `forge-ci/setup-soldr`,
and `docker/test.Dockerfile` installs the latest one inside the container;
neither touches the host.

## Daemon version contract (#509, #324)

Decided 2026-10-09.

- **Every daemon-backed CLI command runs the version pre-flight** before it
  sends work (#324), and `bosn mcp` runs it once at startup against a live
  daemon: an incompatible daemon is refused with both releases, both
  protocols and the remedy, never a bare "request failed".
- **Clients accept by protocol (#509 phases 2 and 5).** A daemon whose
  protocol window holds this client's `DAEMON_PROTOCOL` is accepted whatever
  its release; otherwise it is refused. The release guard is retired to the legacy path: only a daemon
  reporting protocol 0 (it predates the handshake) is still compared by exact
  release (`legacy_release_matches`). `daemon_version_mismatch` is the one
  place this is decided.
- **The daemon serves a protocol window (#509 phase 3).** It advertises
  `[DAEMON_PROTOCOL_MIN, DAEMON_PROTOCOL]` on ping, and every request carries
  the client's protocol. A bump of `DAEMON_PROTOCOL` keeps the previous
  protocol's decoders: `DAEMON_PROTOCOL_MIN` trails it by exactly one (N-1). A
  client outside the window gets one typed refusal from the daemon (reply code
  5, `Error::ProtocolUnsupported`, `protocol_unsupported:` with both windows
  and the remedy). Ping and stop are always served so the remedy stays
  reachable; a request reporting protocol 0 predates the window and is served.
  Additive fields that every peer of the current protocol still reads are not
  a bump (phase 3 itself added its fields this way).
- **The daemon starts from one authoritative executable (#509 phase 4).**
  `ensure_daemon` launches `<state>/daemon/bosn-daemon`, never the client
  binary that happened to call it. A client installs itself there (the binary
  is self-contained since #265; by atomic rename, stamped in `bosn-daemon.release`)
  only when it is a newer release, or the same release from another build; an
  older client never downgrades it.
- **Idle handoff only (#509 phase 4).** A client newer than the running
  daemon sends operation 39; the daemon stops only if it has no unfinished
  jobs, no queued or running CI runs and no held leases, and otherwise answers
  busy (code 32) and keeps running. The daemon decides, never the client: a
  busy daemon is never stopped on anyone's behalf, and a daemon that predates
  operation 39 is left alone.
- **Every daemon contact warns on a daemon/client mismatch; only work fails.**
  The CLI connects through one helper, `daemon_client`, which prints
  `warning: bosn daemon/client mismatch: ...` on stderr, once per process,
  for a compatible daemon of another release and for one on another protocol,
  and never fails the command over it. Refusing an incompatible daemon stays
  the job of the pre-flight above, for commands that send it work. Commands
  that need no daemon (`--version`, `compose plan`, `act plan`) never contact
  one, so a dead or stale daemon can never break them.
- **A CLI-started daemon exits when idle (decided 2026-10-15).** After 10
  minutes (`BOSN_DAEMON_IDLE_EXIT_SECS`; `0` never exits) with no client
  connected and nothing busy (the operation-39 test: no unfinished job, CI run
  or held lease), it stops, so the next command starts a daemon of its own
  release instead of talking to a stale one. A persistent daemon is what
  left machines on old releases. Exiting tears nothing down: containers keep
  running, and the registry and its writer fence pass to the next daemon,
  which reopens the same SQLite file. `Service` itself defaults to no idle
  exit; the `bosn daemon serve` binary turns it on.
- **Compatibility moves from release equality to a wire protocol (#509).**
  Every daemon reports `DAEMON_PROTOCOL` on its ping reply (zero means it
  predates the handshake). Bump it on any change to the request/reply wire or
  the JSON documents it carries; `protocol_surface_is_pinned` fingerprints those
  files and fails until the bump decision and new fingerprint are recorded.
- **Not soldr's per-generation daemons.** soldr's broker can launch one daemon
  generation per client image because its daemons are caches. Bosn's daemon is
  the sole registry writer and Docker owner for a state directory, so it stays
  one daemon per state directory; skew is solved by the daemon serving a
  window of protocols, not by running several daemons (phased plan on #509).
- **Docker access into the engine (2026-10-10, #547 step 4).** Each run reaches
  the engine's `dockerd` only through act2 serve's per-run Docker proxy
  (`/var/lib/docker/bosn-ci/sock/<key>.sock`, opened at admission). Engines bind
  no host socket directory; the daemon serves no proxy for CI runs. An engine of
  an earlier release whose creation profile froze `docker_socket` is still
  observed and retired with its directory, never made again.
- **One shared engine, every platform (2026-10-10, #547 step 4).** Every run on
  every daemon leases a slot (ports `40000+2·slot`) in the machine's one
  long-lived engine, in its own run scope; the engine retires after
  `[engine] idle_retire_secs` (default 600) with no run. There is no per-run
  engine and no prepared spare: `[engine] shared` and `spares` are accepted and
  ignored so existing configs keep working. A run whose wanted engine differs
  from the machine's engine waits for it to drain (#544), never gets its own.

## Async daemon waits (decided 2026-10-17)

Daemon IPC, startup readiness, handoff, job following and CI following wait on
the async runtime. A CLI entry point may use `Runtime::run` as its synchronous
boundary, but its wait loop uses `async_engine::sleep` or an async signal, not
`std::thread::sleep`. Keep blocking filesystem and process setup at the CLI
boundary or move it to a blocking worker when it must run inside an async task.
As adjacent paths change, move them to this shape rather than adding another
blocking poll loop.

Daemon-originated diagnostics use `daemon_log!` so every line carries Unix
milliseconds in `daemon.log`. This includes startup errors from `bosn
daemon serve`. Keep the log useful for matching an IPC loss to the daemon's
last activity and restart; do not add plain `eprintln!` in daemon execution
paths.

## One daemon supervisor (decided 2026-10-17, #669)

An installed user systemd or launchd entry for a state directory owns that
daemon's startup. The CLI installs the authoritative
`<state>/daemon/bosn-daemon`, asks the service manager to start the enabled
entry, and waits asynchronously for the socket; it never forks a competing
child after selecting the entry. An old generated entry is repaired to the
fixed executable before starting. A unit for another state directory is not
selected. Generated entries encode executable and state paths as single
arguments and are replaced atomically before the manager reloads them. With
no entry, the CLI starts the fixed executable on demand in a
new Unix session. `WriterAlreadyHeld` means another daemon already owns the
registry; `daemon serve` exits successfully so `Restart=on-failure` never
burns the unit's start limit. Idle exit and an explicit stop are also clean
exits. Service-manager commands are one-time process setup at the CLI
boundary; readiness and handoff remain async waits.

## Work dies with its client unless detached (decided 2026-10-16)

- **A following client owns its work.** `bosn run` (ensure and task jobs),
  `bosn ci run`/`retry` (which now wait by default) and the MCP/Python
  `bosn_ci_run`/`bosn_ci_retry` hand the daemon their PID and the host's
  creation key for it (request tags 30-31, `ClientOwner`). `--detach`
  (`detach: true` over MCP) sends no owner and the work outlives the client.
  Submit-only commands (`bosn setup ...`, `bosn manifest ...`) stay detached.
- **The client is the first line of defense:** Ctrl-C or a termination request
  cancels its job or releases its CI run (operation `release`: cancelled only
  when no other owner holds it). A second request exits at once.
- **The daemon is the second line:** it looks every owner up twice a second
  (one process-table read each; a reused PID or an unreaped zombie counts as
  gone, an unreadable one as alive) and cancels work whose owners are all gone.
  Cancelling stops the container work (CI scope close, the #357 task stop).
- An identical submission that joins work adds its owner; a detached joiner
  pins it; work created unowned never gains an owner. The 30 s follow lease of
  `bosn run` (#357) stays as a backstop.

**CI cancellation display (#675).** Accepting cancellation of a running run
records `cancel_requested_at` before signalling the engine. The wire lifecycle
stays `running` while teardown owns its scheduler slot; new clients render that
timestamped state as `cancelling`. The optional field is additive for older
protocol clients. Only completed teardown changes the lifecycle to
`done/cancelled`. A queued cancel still finishes immediately. Repeated requests
keep the first timestamp and do not publish duplicate status transitions.

## CI capacity accounting (#672, decided 2026-10-17)

CI admission defaults to `max(1, physical_cores / 4)`, counting package/core
pairs among the process's allowed CPUs on Linux or `hw.physicalcpu` on macOS,
capped by available logical CPUs. When topology is unavailable or a CPU quota
is smaller than the allowed set, use half the logical count (at least one).
Read topology off the async daemon lane. The task lane keeps its separate
`logical_cpus * 2` slot default and per-slot CPU quota. The shared CI engine
keeps the 8-CPU default ceiling; `[engine] cpus` overrides it. `bosn jobs`
and `bosn ci doctor` name both lanes, the CI admission limit, and the
engine's effective CPUs, source and memory. A running engine reports its
frozen limits from the machine claim even before this daemon adopts it. The
claim freezes the CPU source; older claims without that label report the
source as unknown. Before an engine exists, report the limits a new engine
would use.

## Docker context entry limit (#636, #641)

Count only entries selected by the Dockerfile and its effective `.dockerignore`
against the context collector's `max_entries` limit. Prune fully ignored
directories during the walk; where a negation requires walking an ignored
directory, retain selected entries plus one witness for each Dockerfile source
so missing-source checks stay accurate. A refusal reports both the raw number
walked before stopping and the selected count at the limit. Git ignores apply
when a source snapshot materializer uses `.gitignore`; the Docker context
collector itself follows Docker's `.dockerignore` policy. A separate
one-million-entry raw walk budget bounds traversal of a partially ignored
tree; it never changes the 100,000 selected-entry cap.

## Widget release handoff (#652, #678)

The widget hello reports its release. A missing release is a protocol-1 widget;
the daemon accepts it, keeps it alive while a matching artifact is installed,
then explicitly restarts the user unit. If the old process is still connected
after systemd accepts the restart, its next poll gets Quit. A replacement quit
is addressed to the old PID even if the new widget registers first. Dismiss
messages carry a PID, so the old widget cannot clear the new one's presence;
protocol-1 dismissals without a PID are handled as legacy handoffs. A
replacement quit does not persist the user's dismissal choice. Installation failure keeps the
old widget; a restart failure is logged and leaves the unit available for a
later start. The user unit launches the
authoritative `<state>/daemon/bosn-daemon widget --autostart` path; that command
selects the artifact for the daemon executable's release, so the unit no
longer pins a widget version. The daemon's repair runs in an async child
process and does not hold the daemon executor during download or systemd
operations. Protocol 2 carries the hello release; the daemon still serves
protocol 1 for the previous decoder.

## Low CPU priority (#643, #645)

Every container Bosn creates runs at `bosn_engine::LOW_CPU_PRIORITY_ARG`
(`--cpu-shares=256`, cgroup v2 `cpu.weight` about 10) so builds yield to the
desktop under contention. CPU caps (`--cpus`) bound how much; the weight decides
who wins. A new `docker create`/`run` site must pass the constant. When an
operator provisions a low-weight `bosn.slice`, `DockerEngine::with_args` adds
`--cgroup-parent=bosn.slice` at execution, never in a creation profile, so the
slice never renames a setup container. See [CPU priority](docs/cpu-priority.md).

## CI engine architecture (#544, #547)

Decided 2026-10-08. This replaces "one private nested engine per run."

- **One long-lived act engine per machine.** Concurrent runs execute inside it,
  each isolated by its own parent cgroup (`--cgroup-parent=/run-<id>` with
  memory/CPU/pids limits), network, work directory, artifact path and a
  run-scoped Docker proxy. Per-run cleanup inside the engine is keyed by the run
  label; the engine has one storage budget, in-engine GC and idle retirement.
- **No per-run engines with an external cache-sharing protocol.** The cache has
  one owner inside the engine, not leases coordinated across engines (the
  approach in #546, rejected).
- **Mixed-release memory safety (2026-10-17, #652).** When sizing a new shared
  engine, reserve the Docker memory limits of every other running Bosn act
  engine, including pre-claim engines from older daemons; an unlimited limit
  reserves the whole host. Keep at least 2 GiB outside those limits, refuse an
  unsafe pinned limit, and recheck the running-engine inventory immediately
  before creating the shared engine. A claim records its Bosn release; another
  release's claim, including an older unlabelled one, is drained rather than
  adopted even when its act pins match. Do not stop a busy older engine.
- **act2 owns CI; Bosn stays a generic Docker lifecycle manager.** An act2
  serve mode inside the engine owns run admission, concurrency, isolation, the
  action/tool/image cache and in-engine cleanup. Bosn manages the engine as an
  ordinary long-lived owned container and forwards `bosn ci` unchanged. Move
  CI-specific code out of `bosn-service` behind that boundary; do not add new
  act-specific machinery to Bosn. No third composite app unless something
  other than act2 needs the cache.
- Trust model is unchanged: the engine is privileged, so it runs trusted
  workflows only.
- **Singleton enforcement is a Docker name claim (2026-10-10, #544).** The host
  Docker engine decides which daemon makes the machine's engine: a created, never
  started container named `bosn-ci-engine-claim` whose labels name the engine and
  the holding daemon (registry, boot ID, pid, pid start time). Docker's name
  uniqueness makes it atomic across daemons, state directories, users and
  restarts, and it is scoped to the actual Docker engine, not the host's
  filesystem. A machine-level file lock was rejected: it is scoped to a
  filesystem, not the Docker engine, and does not survive into the engine. Other
  daemons adopt the claimed engine when its identity (pinned act/engine/runner
  images and init command) matches, else request retirement and wait; a dead
  holder's claim is taken over once no live run is in it.
- **Slots live in the engine (#544).** Runs from every daemon lease slots in
  `/run/bosn-slots` inside the engine under one `flock`, so idleness and ports
  are the engine's. Only the engine's maker retires it (registry proof); others
  only retire one whose maker died. Legacy engines (any act engine but the
  claimed one) are drained: busy ones finish, idle ones older than 10 minutes
  and seen idle twice are removed with their storage volumes, proven absent.
- **How Bosn forwards (decided 2026-10-10, act2#62).** Bosn reaches `act serve`
  only through `docker exec <engine> act serve <admit|exec|close>` (the same
  act binary is the client), so the transport works wherever `docker exec`
  does and needs no host-side socket. Bosn starts serve once per engine under
  a lock. The engine's slot lease still picks the slot and Bosn passes it to
  `admit`, which serve honours or refuses; Bosn refuses any admitted scope
  other than the one it expected.
  Serve's admissions can outlive their lease (a failed `close` still frees
  the slot), so a `409 slot N is taken` makes Bosn close every admission no
  lease backs, reading serve's runs before the slot table, and admit once more
  (`ci/engine/admission.rs`).
- **What serve owns (decided 2026-10-10, act2#67, #547 step 4).** Besides the
  run scope: each run's Docker proxy; `act serve prepare`, which loads and
  proves the pinned runner image (Bosn passes the publisher manifest and
  config bytes) and prepares the tool cache once per engine (seed, or a frozen
  generation's overlay whose recipe digest Bosn pins as
  `OVERLAY_RECIPE_SHA256`); the tool-cache save after a run whose act exited
  on its own (reported in the `close` reply); and the shared cache budget.
  Bosn keeps only the runner's stock tools (once per engine, after prepare).
  `bosn ci doctor` is `act doctor --json` run in the engine, typed and printed
  as act prints it.

## Automatic retention (#545)

Managed garbage collection runs by default while the daemon is running.
Operators opt out with `auto_retention = false` in the state directory's
`retention.toml`. Keep the ownership, age, liveness and pinning gates on every
removal.

**No machine-policy file (#527, decided 2026-10-23).** The `bosn_core::config`
`MachinePolicy`/`AppPolicy` resolver and its `[policy]` TOML were never called by production
code, so they were retired rather than wired. `retention.toml` is retention's one
configuration surface; its thresholds are planner constants. A new operator knob gets a real
reader in the same change, never a parse-only surface.

**Objects from registries that predate the machine catalog** (decided
2026-10-10). A label naming a registry the catalog never saw is not proof that
the registry is gone, so those objects stay `foreign-registry`: never reclaimed
automatically, never adopted by guessing. The operator decides once per
registry. `bosn gc release-registry` lists the registry ids Docker objects
carry; `bosn gc release-registry <UUID> --yes` records that registry as
abandoned in the machine catalog, and the machine daemon then reclaims its
objects through the ordinary pass under every gate. It refuses this machine's
own registry and any cataloged registry whose database still exists. No bulk
"adopt everything foreign" command, and no re-labelling of foreign objects into
the current registry.

**Host decision, 2026-10-10 (v0.1.20).** The only pre-catalog registry left on
the development host is `b3718269-079a-4b01-a761-b3c94b987a8e`, and the only
object naming it is `bosn-ci-cache-v1`: the machine-scoped, pinned, shared CI
cache that every run mounts. It is **not released**. Its state directory can't
be proved gone (none of the 83 registry databases on the host names it, but
that is absence of evidence), and releasing it could reclaim nothing anyway,
because the volume is pinned and in use. Release a registry only when its
state directory is provably gone and its objects are not shared machine
resources.

**Diagnosing a pass.** A daemon started on demand appends its stderr to
`<state>/daemon.log` (rotated to `daemon.log.1` past 8 MiB at start). Every
retention pass ends with a timestamped `bosn retention: pass finished at unix
…` line. `bosn gc owned --json` waits up to 600 s for its fresh read; the 3 s
control deadline made it fail on any busy host.

**Idle owned-volume byte budget** (decided 2026-10-10, #545). The 14-day volume
gate is not a disk bound on its own: setup content changes mint fresh `bosn-v-*`
volumes (about 110 GB/day measured on the development host), and they waited
out the full gate while the disk filled to 92%. So idle owned volumes also have
a byte budget (`DEFAULT_IDLE_VOLUME_BUDGET`, 64 GiB). Past it, the oldest
volumes that are proven ours, unmounted, unpinned and idle for at least
`DEFAULT_IDLE_VOLUME_FLOOR` (1 h) are reclaimed until the rest fit. The budget
only shortens the age gate. Ownership, liveness and pin gates still apply, and
the pre-removal re-check repeats them.

**Disk pressure zeroes the budget** (decided 2026-10-16, #658). An absolute budget
still held 64 GiB of idle volumes on a 96%-full host. When the filesystem under
Docker's data root (`docker info` `DockerRootDir`, measured by statvfs) has less
than 15% (`PRESSURE_FREE_PERCENT`) or less than 100 GiB (`PRESSURE_FREE_BYTES`)
free, the maintenance pass uses `RetentionPolicy::under_disk_pressure`: idle
volume budget 0 and a 1 h stopped-container gate (`PRESSURE_CONTAINER_TTL`,
also used for idle keepalives). The 1 h volume floor and every ownership,
liveness and pin gate, and the re-check, are unchanged. The probe lives in the
service (`managed_retention/pressure.rs`); the planner only gets the policy. A
root the daemon cannot stat is no pressure, logged once. The pass logs
`bosn retention: disk pressure (X GiB free, Y%); idle volume budget 0`.

**Eager reclamation by workspace and family** (decided 2026-10-16, implemented
2026-10-23, #659). A stack or spec volume's identity hashes its workspace path, so a worktree
or temporary checkout mints volumes no later ensure can reuse. Past the 1 h idle floor, an idle
volume is reclaimed before its age gate when its workspace path is proven absent
(`workspace-gone`; an unreadable path proves nothing), or when it belongs to a repository
family (the canonical git common directory, read from `.git` at or above the workspace, so
worktrees share their clone's) and stack whose 3 most recently used workspaces
(`FAMILY_KEEP`) exclude it (`family-cap`). A workspace in no repository is never grouped by
guessing. The family is read at pass time from the live workspace rather than recorded in
the registry: only a present workspace can be in a family, a gone one falls to
`workspace-gone`, and a registry schema bump would lock older daemons out of the registry.
Every ownership, liveness and pin gate and the re-check (now at the idle floor whatever the
budget) are unchanged; each early removal logs its reason, and `bosn gc owned` reports
`eager` counts. Merged-branch purging is #732.

**Owned footprint ceiling** (decided 2026-10-22, #508). Free disk is not owned
allocation. Every pass sums the Bosn-labelled containers, volumes and images it
observed (`OwnedFootprint`), logs the total by kind with the unmeasured count, and
past `DEFAULT_OWNED_CEILING` (256 GiB) uses the same `under_disk_pressure` policy.
Every ownership, liveness and pin gate, the volume floor and the re-check are
unchanged. Sizes come from Docker's own reports; an unmeasured object is counted,
never guessed.

## CI late-create cleanup (#676)

An unacknowledged Docker create keeps its durable helper or engine intent while
the request may still land. The retry loop accepts a fresh, explicit Docker
absence observation only after the 30 s control deadline plus 10 min grace
(630 s). Helper intents use their immutable creation time; engine intents use
the last durable update, which can only extend the wait. The engine records the
create request at the moment it is sent, after preparation. Startup migrates a
legacy engine without a create marker once into a durable uncertain marker
with a fresh timestamp; later retries use that timestamp. An absent helper is
finished without inventing a container ID. Any observed object still goes
through exact ownership and removal checks. Repeated deferrals log once per
intent and reason until that intent retires; the log state is process-local and
does not affect cleanup decisions.

## Shared CI cache budget (#544)

Decided 2026-10-10, moved behind act2 the same day (#547 step 4). While no
cohort policy is enrolled (#508), act2 serve in the machine's engine applies a
default budget to the legacy `actcache/` namespaces of `bosn-ci-cache-v1` when
it starts and hourly: at most 8 GiB of completed archives per repository
namespace, expiry after 7 days unused or 30 days old (`--cache-budget-*`, set in
`run_scope.rs`). It runs act2's own `cache prune --apply` under the machine
maintenance lock (`.bosn-maintenance-v1.lock`) that cohort maintenance holds.
act2's exclusive transfer and metadata locks are the liveness gate: a namespace
a live server holds is left alone. A namespace without `transfers.bolt` is
skipped, never deleted. The volume itself is never removed. The last pass is
reported by `bosn ci doctor`. An enrolled cohort policy replaces this budget.
Don't raise the budget to avoid a cold cache; see `docs/ci.md` "Shared cache
budget". There is no Bosn-side pass.

## Design principles (fleet directives, zackees/ci.yml#163)

Agent and review guidance from zackees/ci.yml `docs/policy-rust.md` ("Design principles, narrowed"). Not enforced by ci-lint; no rule ID.

1. **Search before you add.** Before adding a resolver, detector, predicate, guard or path helper, `rg` for an existing one. Reuse it, or say in the PR why it does not fit.
2. **Conflicting implementations get a research issue, not an inline winner.** If the same concept is implemented more than once with observably different behaviour and fixing it needs a design decision, file `research: N conflicting implementations of X -- which is canonical?` listing every copy (`file:line`) and what the difference does to real input; cross-reference it and carry on with your task. Copies that behave identically are a refactor, not an issue.
3. **Keep behaviour changes and refactors apart.** Separate commits, preferably separate PRs, never one mixed diff.
4. **Refactor first only when you would grow a giant function.** If a fix would add more than 50 lines to a function already over 200 lines, extract first in a separate behaviour-preserving change with characterization tests written first (RED -> GREEN). Otherwise fix it where it belongs; no unrequested restructures.
5. **Function and file size follow the complexity ratchet (`RUST-018`).** New functions stay under clippy's `too_many_lines` (100) and `cognitive_complexity` (25). A function carrying an `#[expect]` must not grow; delete the `expect` once it is back under. Never split a file with `include!` fragments to dodge a length ceiling. Before splitting a hot file, check `git branch -a --contains` for in-flight branches -- a split is a rename and conflicts destructively.
6. **Fail fast, and delete before you abstract.** Reject invalid input at the boundary with a typed error; never fall back silently to a degraded path. Prefer deleting code to adding a layer. No new trait until there are two real implementations (a test double does not count).
7. **Existing boundaries beat generic DRY.** Platform modules, Dylint-enforced layering, crate boundaries and documented deliberate copies win over removing duplication. Do not copy code across a crate boundary to get around it -- say the boundary hurts. An undocumented copy is a finding under 2; a documented one is not.
