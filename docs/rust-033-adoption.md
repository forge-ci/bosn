# RUST-033 adoption (#799)

The local `lint` lane replays the complete remote `lint` job. Both use
`soldr lint rust --cross-dylint --check --locked --features
bosn-python/embedded-python-tests`. Soldr adds `--workspace --all-targets`
and `-D warnings` to Clippy and reads the platform set from
`[workspace.metadata.soldr].targets`. No replay step is excluded. The existing
Clippy-wrapped nextest and doctest build remains unchanged (#503), as do the
tests lane's Rust-test-source exclusions.

`ci-attestations.yml` declares full-set Clippy and Dylint gates. These are
declarations, not evidence: a failed lane must never stamp either gate.
The lint job runs on PRs and merge groups and is a dependency of `CI summary`;
branch protection still requires only `CI summary`.

The workflow lint step sets `SOLDR_DYLINT_TOOLCHAIN=nightly-2026-05-28`
as the workaround for [soldr#3999](https://github.com/forge-ci/soldr/issues/3999).
This must track the `channel` in both `_vender/kernal-api/dylints/*/rust-toolchain.toml`
files and must be removed when soldr#3999 is fixed. The local lint lane replays
this same step, so it receives the same environment as hosted CI.
The step provisions nightly rustfmt and Clippy through `soldr rustup` too:
the override selects this toolchain for the entire canonical lint command,
and Dylint's automatic provisioning alone omits the formatting component.
It also provisions the nightly standard libraries for every target read from
`[workspace.metadata.soldr].targets`: Soldr's automatic cross-target preparation
otherwise installs stable std while this overridden command invokes nightly.
The root Dylint metadata explicitly selects the two kernal-api lint libraries
at the clean vendored checkout's commit `fc634e507024d63ccaaf75fa564818b9dcfbff36`.
It uses a pinned Git source because `_vender/` is ignored and absent from clean
CI checkouts; an empty library discovery cannot supply coverage. This pin and
the nightly channel must be updated together when adopting newer lint libraries.

## Unresolved dependencies

- Shared ci-lint has not implemented RUST-033's special lint enforcement in
  shadow mode (forge-ci/ci.yml#511). The current verifier still returns success
  for missing proof in shadow mode. Bosn retains its existing enforce mode;
  changing to shadow cannot yet meet #799. Runtime lanes retain their existing
  behavior until the shared tool supports the requested split.
- The initial Soldr 0.9.51 host-only diagnostic passed Clippy, then failed with
  `rust-nightly-versions.v1.json has no nightly mapping for Rust 1.95`
  (58.62 seconds). Soldr's related resolver issue is forge-ci/soldr#3999.

Until lint succeeds and the final committed tree passes the shared local gate,
this change must not be merged or described as fixing #799. Use `Refs #799`.

## Validation evidence

`./lint` passed. The focused isolated suite (`test_local_gate_wiring`,
`test_tests_lane_inputs`, `test_bosn_gate_proof`) passed all 12 tests.

Soldr 0.9.52, unchanged tree `7b74ba466d2879fb5d95efc288fd431ed863ca02`:
cold attempt 413.11 seconds, warm attempt 133.18 seconds, limit 41.31 seconds.
Both stopped at the same Windows test compile error (the durability tests
imported a Unix-only fixture). These are failed-attempt measurements, not a
successful full-lint benchmark. The host was shared with other builds and the
cold attempt also overlapped `./lint`; the warm attempt ran after it finished.
The Windows fixture condition is corrected in a separate commit.
