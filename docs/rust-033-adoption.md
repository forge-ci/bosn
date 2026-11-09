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

## Unresolved dependencies

- Shared ci-lint has not implemented RUST-033's special lint enforcement in
  shadow mode (forge-ci/ci.yml#511). The current verifier still returns success
  for missing proof in shadow mode. Bosn retains its existing enforce mode;
  changing to shadow cannot yet meet #799. Runtime lanes retain their existing
  behavior until the shared tool supports the requested split.
- Bosn currently has no `[workspace.metadata.dylint]` libraries. Selecting an
  arbitrary fleet library would introduce a new code policy, so this adoption
  does not invent one or certify an empty pass. A real configured Dylint engine
  is needed before this lane can produce valid Dylint evidence.
- The initial Soldr 0.9.51 host-only diagnostic passed Clippy, then failed with
  `rust-nightly-versions.v1.json has no nightly mapping for Rust 1.95`
  (58.62 seconds). Soldr's related resolver issue is forge-ci/soldr#3999.

Until lint succeeds and the final committed tree passes the shared local gate,
this change must not be merged or described as fixing #799. Use `Refs #799`.
