# CI cache and Docker footprint: living spec

This document records the verified behavior of Bosn's act2 engine and the
remaining work to make repeated CI runs fast without unbounded disk growth.
Update the survey, implementation table and verification evidence in the same
change as each implementation step. The desired behavior below is a contract,
not a claim that it is already implemented. Related issue: #456.

## Current implementation status (2026-10-04)

This status is reconciled against main at
`d0b313fdf80c121d3fe213720779702fcde1ca92`. Historical evidence below records
intermediate states; descriptions of an unmerged candidate there are not
current rollout status. No new Bosn release is claimed.

| Requirement | Implemented on main | Verification and remaining gap |
|---|---|---|
| Warm caches across jobs and fresh engines | Shared machine volume for act archives, runner-image tar, actions, tool installs and repository cache archives; unique tool publication stages and hidden-stage exclusion | Real private-Docker proofs establish later-job and fresh-engine hits. Shared class expiry and sustained physical ceilings remain open. |
| Machine cache accounting | Allocated and apparent bytes for five classes, bounded per-repository detail, partial/unknown results and independent volume total | Sparse files, malformed samples, failed reader cleanup and actual cache measurement are covered. Samples are non-atomic; nested engine bytes are separate. |
| Private containers/images/build cache | Exact owned engine/storage journal and retirement, with bounded recovery after failures/restarts | Removing a verified private engine retires its nested Docker storage. This is not host owned-image or shared builder expiry. |
| Archive retention | Verified act2.7, typed age/byte policy, bounded maintenance reports and journaled finite-lived helpers | Actual private cohort maintenance and warm-cohort reuse pass. Default legacy planning remains outside configured retention. |
| Idle maintenance | Normal daemon discovers an existing immutable shared policy and runs periodic maintenance without jobs; bounded latest outcome persists for diagnostics | Actual idle daemon and restart proofs pass. Absence never bootstraps defaults; policy bootstrap and production repository enrollment remain open. |
| Warm migration | Typed legacy/cohort routes, participating lifetime leases, exclusive import, historical publication journal and sampled current inventory | Callable imports preserve warmth and recover historical publication. They do not authorize production enrollment or physical-footprint claims. |
| Routing admission | Participating legacy wrapper rechecks routing after acquiring its shared migration lease | Real lock race proves published routing refuses a stale legacy plan. Routing publication/selection and exclusion of older nonparticipating writers remain open. |
| Other shared classes and host pressure | Accounting and warm restore exist; unmanaged census reports host build-cache bytes | Tools, actions, shared image tar and tool-install expiry are unfinished. Host build cache is report-only in unmanaged GC; owned image and scoped builder pressure control are unfinished. |

Merged implementation evidence:

- Foundation [PR #473](https://github.com/zackees/bosn/pull/473):
  `b23398ae`, owned footprint and private-storage reconciliation.
- Retention/startup [PR #483](https://github.com/zackees/bosn/pull/483):
  `0f9c2302bd172e5492015c18b196bac6775f6cde`, including the act2.7 pin, tool
  publication corrections, policy discovery, current inventory and startup
  maintenance. These changes are on main, rather than only a candidate.
- Routing admission [PR #485](https://github.com/zackees/bosn/pull/485):
  `d24c25d676b94910ece3141788fa340ea33a8b10`; required remote checks passed.

Act2 v0.2.89-act2.7 was released from
`1a6782d5bbabb715f425cad5601b1f706fc23fa8` (merged PR #28). Exact-commit full
CI 37190653388 and release run 37191477610 passed. The Linux x86_64 archive
SHA-256 is `61d640112af87278075c70cd67412ec9c44ceffd1e269e88632d50d4a83ebd27`;
the extracted binary SHA-256 is
`328147f59cc101aa86cc826ecb5e7b2493f74765d520d4809997b2a9c6ba56bd`. Published
checksums and the reported version match the main pin. This release includes
empty-cutover correction and historical publication receipts; historical receipt
parsing alone cannot authorize enrollment.

The cache lifecycle follow-up [PR #486](https://github.com/zackees/bosn/pull/486)
is merged at `d0b313fdf80c121d3fe213720779702fcde1ca92`. Its exact-source gate
passed (Rust397s, Linux305s, total702s; tree
`3580afa77edd7db51100ed9623265fa14a11d562`) and required remote checks passed.
Main now avoids duplicate snapshots for identical helper registration and uses
cancellation-safe bounded process controls for normal CI commands and cache
maintenance/inventory/import/receipt reports. These preserve journals and
nonzero/partial outcomes; local client cancellation does not establish remote
Docker rollback. Distinct-nonce history and physical registry convergence remain
open. No Bosn release is claimed.

The new survey and isolated copy-on-write seed primitive are documented below.
They are evidence for the next toolcache implementation, rather than activated
immutable generation publication or production mounts. Production warm
enrollment, older-writer exclusion, shared class expiry and host image/build-cache
retention remain open.

## Survey and evidence (2026-10-03)

The read-only host audit in #456 measured ~859 GiB under `/var/lib/docker`.
`bosn-ci-cache-v1` was ~49 GiB, of which `actcache/` was ~41 GiB. Seventy-six
unattached `bosn-v-stack-*` or `bosn-v-machine-*` volumes were present; their
attachment state alone says nothing about safe removal. An ~18 GiB anonymous
volume sampled during the audit was attached to an active CI engine. The audit
did not establish an anonymous-volume leak.

The implementation survey was refreshed against `main` at `146515cf` (act2 `0.2.89-act2.3`). The session checkout was on the older `fix/359-workspace-identity` branch, so implementation work is in `../bosn-extern/bosn-456` on `fix/456-ci-cache`. The current code and tests give the following narrower evidence:

| Resource | Reuse between runs now | Accounting now | Expiration now |
|---|---|---|---|
| act release | Verified tar in shared `bosn-ci-cache-v1/tools/` | Class bytes and allocated blocks in `runners cache` | Whole-cache clear only |
| Runner image | Saved tar in shared `images/`, loaded into each fresh engine | Class bytes/blocks; each active engine also holds loaded layers outside this total | Engine removal deletes nested copy; shared tar lasts until whole-cache clear |
| Action checkouts | Shared `actions/` path passed to act | Class bytes and allocated blocks | Whole-cache clear only |
| `actions/cache` archives | Shared `actcache/<repository hash>/` path passed to act | Class bytes/blocks and per-repository namespace breakdown | act2 has fixed age GC while its server runs; no configurable size ceiling |
| `/opt/hostedtoolcache` | Seeded from shared `toolcache/`; completed installs copied back before teardown | Class bytes and allocated blocks | Whole-cache clear only |
| Job container/image layers and job build cache | Private nested engine per run, optionally prepared as a spare; runner tar is loaded, other layers rebuilt or pulled | Nested bytes are not attributed per run in CI cache usage | Owned engine retirement removes the engine and disk-backed anonymous storage; tmpfs storage disappears with the engine |
| CI run records/source snapshots | Retained in daemon state, separate from Docker cache | `runners prune-cache --max-bytes` measures these files | Count, age and size pruning |
| Host Docker build cache and non-CI Bosn volumes | Outside the CI cache volume | Unmanaged census reports only unowned artifacts; owned resources are protected there | Separate conservative lifecycle commands; no aggregate owned-volume quota |

Evidence: `crates/bosn-service/src/ci/engine.rs` mounts the named volume,
constructs act cache paths, seeds and saves tool installs, saves the runner
tar, and delegates exact engine retirement to `crates/bosn-service/src/act_engine/`. The current engine is verified before retirement and uses disk or tmpfs storage according to its frozen creation profile. `crates/bosn-service/src/ci/runtime/runners.rs`
measures the whole volume and prunes run records separately.
`crates/bosn-service/tests/ci_live.rs` contains an opt-in second-run
`actions/cache` restore proof; its module also describes leak checks.
`crates/bosn-service/src/ci/lifecycle.rs` has fault-path and restart
reconciliation tests, but those are not proof of a host-wide storage ceiling.
`docs/rust-unmanaged.md` documents the existing machine-wide *unmanaged*
census and its deliberate exclusion of resources owned by this registry.

### act2 already performs age GC

The pinned act2 cache server inherits a metadata-aware age GC: unused entries
expire after seven days, entries older than thirty days expire even if used,
incomplete uploads expire after five minutes, and superseded entries have a
five-minute grace period. GC runs when the server starts and at most hourly
while it handles requests. It has no byte ceiling. Therefore an idle namespace
receives no maintenance and heavy weekly use can still grow very large.
Evidence: [the pinned act2 handler](https://github.com/zackees/act2/blob/v0.2.89-act2.3/pkg/artifactcache/handler.go).
Bosn must coordinate with this metadata rather than delete archive files by
creation time and call that LRU. A live archive transfer also needs explicit
protection when multiple servers share a namespace.

### Current live-host survey

A fresh host probe found `/` at 93% full with 134 GiB free, and the shared CI
volume retains its exact Bosn machine/pinned labels. Docker container and
volume inspections respond, but `docker system df` stalled in the previous
survey. Component accounting should use bounded direct volume measurements
rather than depend on that whole-engine operation.

## Accounting implementation in this branch

`bosn ci runners cache` now returns `bytes` (apparent file lengths),
`allocated_bytes` (filesystem blocks), `components`, `partial`, and `errors`.
Components contain class totals and valid repository namespace details. The CLI
shows the twenty largest entries; JSON carries up to 256 repository namespaces
plus the five class totals. Larger stores retain their largest or unknown
namespace details and explicitly mark the report partial. Errors are bounded,
so an arbitrarily large machine census cannot overflow the 64 KiB tool reply.
The widget/dashboard summary uses allocated bytes and identifies archive bytes.
Older JSON readers retain the existing `volume` and `bytes` fields.

Measurements are bounded, read-only, and non-atomic: active writes can change
sizes between samples. Class totals contain their namespace entries; do not
sum both, or sum component samples into the independent volume total. Unknown
files are included in the volume total. No layer sizes are added to it. A
Docker outage, permission error, malformed sample, or failed measurement is
reported as partial/unknown. Only Docker's explicit `no such volume` verdict
reports absence. The temporary reader uses Docker auto-removal and explicit
cleanup by its immutable ID after every read result, including a measurement command deadline.
Cleanup errors identify the container for recovery. Each create now carries a
unique UUID name and nonce label. A failed or invalid acknowledgement triggers
typed discovery; identity and isolation must match before removing an immutable
ID. Successful removal requires Docker's explicit absence verdict. Discovery
that is absent, unreadable, malformed or mismatched remains pending, with the
helper name in diagnostics. The service now commits a durable helper intent
before create and retries uncertain cleanup through the existing maintenance
worker; the verification scope and remaining crash-replay gap are below.
Its read-only cache mount and masked engine data volume create no build cache.

Verification: the two initial focused tests failed before the implementation
(missing allocated bytes/components, missing partial flag), then passed.
Real sparse-file measurement, malformed/duplicate records, Docker outage,
public runner error reporting and failed-helper cleanup are covered by tests.
Eight helper transport tests pass, including a replay where real Docker creates
the helper but a wrapper discards the acknowledgement. Recovery removes that
verified helper; a subsequent real measurement succeeds with nonzero allocated
bytes and leaves the shared cache intact. Tests also reject successful remove
responses without observed absence and preserve mismatched helper identities.
The replay uses the isolated private Docker engine, not the host Docker data.
The generated CI schema was refreshed from the Rust types in an isolated
container with a writable docs mount.

A third RED repro created 1,000 namespaces and malformed samples: the JSON
report exceeded the tool limit. Bounded namespace/error details turn it GREEN
without discarding the independently measured volume total.

A read-only invocation of the actual BusyBox measurement script took 4.38 s
on the shared volume and reported 53,272,137,728 allocated bytes (~49.6 GiB):

| Component | Allocated bytes | Approximate GiB |
|---|---:|---:|
| act archives | 44,340,707,328 | 41.3 |
| tool installs | 5,726,400,512 | 5.3 |
| runner image tar | 2,226,663,424 | 2.1 |
| action repositories | 945,635,328 | 0.9 |
| act release archives | 32,727,040 | 0.03 |

There were 58 repository namespace measurements. The largest three occupied
13.95, 10.53 and 7.16 GiB. This verifies the need for a byte policy even with
act2's existing age GC. The sample is evidence of usage, not permission to
delete those stores. The reader container was removed after the sample.

Cache quotas, eviction coordination, warm-run benchmarks and aggregate owned
engine/image/build-cache attribution remain open. This slice is local branch
implementation, not evidence of a released runtime change.

## act2 retention implementation in progress

The sister checkout `../bosn-extern/act2-cache`, branch `feat/cache-budget`,
starts from act2 `master` at `5af1c44`; the tested retention slice is committed
locally as `f702202`. Its focused byte-limit repro failed
against existing GC: two 80-byte completed archives remained with a 100-byte
budget. The candidate metadata-aware LRU pass turns that repro green and
keeps the newer archive warm. It measures actual archive lengths rather than
trusting the declared upload size, refuses unmeasurable data, and preserves
metadata when removal fails so future accounting can still find the data.

The next focused repro confirmed a cross-server race: GC could unlink an
archive while another server was downloading it. The candidate now uses a
separate bbolt file for an OS shared lock over each complete HTTP request and
an exclusive lock for GC. A two-process test proves protection during a
transfer and expiration after it finishes. Lookup refreshes use metadata and
absolute-age expiry honors the five-minute lookup-to-download grace.

Act2 exposes typed byte, maximum-age, unused-age and maintenance-interval
settings. A periodic timer maintains an idle **running server**, preserving
warm LRU entries. The artifactcache suite passes with the Go race detector;
focused tests cover policy parsing, invalid settings before store creation,
actual archive lengths and failed-removal metadata preservation. The full CLI
suite passes against a separate private Docker 29.7.2 test daemon with
RAM-backed storage (49.66 s); no host Docker socket or host cache was used.
The changed act2 packages also pass pinned golangci-lint v2.11.4. The CLI suite
caught direct test-input fixtures missing the new policy defaults, and those
fixtures were corrected to match the actual CLI boundary.

This candidate is not enabled in Bosn. Offline maintenance of namespaces with
no server, a machine aggregate budget, bounded repository audit/export and
end-to-end concurrent restore tests remain open. A shared namespace must use
the same coordination protocol in every server; legacy peers cannot be
assumed to hold the new transfer lock. The Bosn pin continues to name the
existing released act2 binary. The dependent branch has its own detailed
contract in `act2-cache/docs/cache-retention.md`.

## Docker-backed verification

The complete opt-in `ci_live` suite passed: six scenarios, zero failures,
473.76 s, using a private Docker 29.7.2 daemon with 20 GiB RAM-backed storage.
The host socket and host CI cache were not mounted into this daemon. The
outer storage must be executable (`tmpfs ...:rw,exec,size=20g`); the initial
harness omitted `exec` and act installation failed with permission denied.
Correcting the harness mount, without changing Bosn's engine profile or
verification rules, allowed the same restore test to pass.

The proof covers:

- A later job restores the actual file saved by an earlier job.
- A second run restores actual file contents in a different engine.
- Different local repositories do not restore each other's archives, even
  when using the same key.
- A run claims a prepared spare, another spare is prepared, and daemon stop
  removes its owned engines/storage.
- Host-resource snapshots return to baseline after success, failure, timeout,
  client interruption and daemon interruption/restart.
- Recorded matrix/needs/failure execution produces the expected tree and
  bounded failure report.

These live scenarios use Bosn's **existing released act2 pin**, not the new
quota candidate. Act2 transfer/quota behavior is separately proven by its
package/process/race tests. The RAM-backed harness does not reproduce the
real host's disk-pressure cleanup failures in #445 or the 0.1.10 partial
create receipt in #452. It is also not a sustained footprint benchmark under
an integrated machine budget.

The isolated test-container IDs and socket-volume identity are recorded in
`/tmp/bosn-456-docker-harness.json` for exact cleanup/reuse. These are temporary
verification resources; none is a new production runner or cache contract.

## Required behavior

1. **Warm jobs.** Jobs in the same repository can reuse safe `actions/cache`
   entries, action checkouts and completed tool installs across runs and
   across fresh nested engines. Different repositories cannot read each
   other's `actions/cache` entries. Concurrent jobs cannot observe partially
   written cache entries. A newly created engine may be cold for its private
   image layers, but its reusable inputs must be hydrated from the shared
   store. Keep the private Docker socket boundary.
2. **Bounded disposable cache.** A configurable policy bounds `actcache/`
   by size and age. Eviction protects entries in use by any active engine,
   including engines of another Bosn daemon using the named volume. A pass
   removes exact selected entries and reports skipped protected bytes. It
   never clears the entire shared volume as a quota mechanism. Tools, runner
   tar, action checkouts and tool installs need explicit retention decisions
   and accounting so they cannot become an invisible remainder.
3. **Owned accounting.** Report measured bytes and counts for each shared
   cache component and repository namespace, active nested-engine storage,
   detached owned volumes, and host-side Bosn images and build cache where
   ownership can be established. Show largest contributors, retention reason,
   exact safe action, and whether measurement is partial. Never add shared
   layers and volume sizes into a false physical total. Continue reporting
   unowned artifacts through the existing `scan` path.
4. **Lifecycle and expiry.** Completed, failed, cancelled and interrupted runs
   eventually remove their exact nested engine and private storage. Cleanup
   failure remains visible and retryable. Old Bosn-owned images and
   containers are expired only after exact ownership, reachability and use
   checks. Build cache needs a separate policy: Docker's build-cache records
   lack safe per-record deletion in the current census, so any prune must be
   scoped and independently verified before it is automated. Retained
   stack/machine and pinned volumes keep the explicit-release contract.
5. **Pressure response.** A threshold warning identifies the owned class
   causing pressure and distinguishes reclaimable from intentionally retained
   bytes. Unknown sizes remain unknown; an incomplete read cannot authorize
   deletion. The warning points to a supported Bosn command, not a broad
   Docker prune.

## Implementation sequence and verification

| Step | Work | Evidence required | State |
|---|---|---|---|
| 1 | Inventory current caches, accounting and cleanup; record a host sample | Code paths, current tests, read-only audit | Complete (survey above) |
| 2 | Expose a typed breakdown for the shared cache and owned engine volumes | Focused RED to GREEN tests with accurate partial/unknown behavior | Shared-cache apparent/allocated breakdown and machine-wide labelled volume attribution implemented/tested locally; bounded private-volume lifecycle correlation verified; per-private-volume allocated-block samples and legacy anonymous attribution open |
| 3 | Add age/size policy for disposable act cache data and active-use coordination | Concurrent live runs retain hits; over-limit idle data shrinks; no cross-repo reads | act2 byte limit and cross-process transfer RED to GREEN; policy settings and idle-server maintenance tested; offline and aggregate completed-archive maintenance implemented locally; Bosn integration and automatic warm cutover open |
| 4 | Account for and expire eligible old CI engines, host images and build cache | Fault/restart live Docker tests, exact ownership checks, repeated-run footprint trend | Named private storage requires independent absence proof; six live lifecycle scenarios and focused mount identity pass, expiring nested image/build-cache storage; online retry verified locally; host base/shared artifacts, legacy anonymous survivors, real partial-create/lost-ack replay and sustained growth checks open |
| 5 | Wire pressure diagnostics and verify sustained warm workloads | Repeated cold/warm benchmark plus disk growth under the configured ceiling | Open |

### Decisions to preserve

- Keep one named shared volume for cache data that must survive fresh engines;
  the host does not have a portable bind path into Docker Desktop's VM.
- Keep a private nested engine per run until a measured alternative preserves
  the socket/isolation and cleanup contract. Reusing a whole engine without
  a robust ownership boundary would let jobs inherit arbitrary state.
- Treat Docker's `reclaimable` estimate and `du` as different measurements.
  Report their provenance and avoid a misleading combined physical total.
- The existing `runners prune-cache` controls run records. Its name must not
  be presented as a shared cache eviction command.

### Still unverified

- A strengthened second-run test now verifies restored file contents in a
  different engine and passed in the isolated Docker harness (81.45 s for the
  cold/warm pair). The sequential-job restore test also passes. Concurrent
  cross-server eviction is covered by the act2 process test; end-to-end
  concurrent Bosn runs under a configured ceiling remain unverified.
- The isolated live suite proves normal lifecycle cleanup and interruption
  recovery on Docker 29.7.2. Delayed/failed control recovery and partial-create
  replay on the disk-backed host remain unverified; the active sample in #456
  is not a leak verdict.
- Which detached stack/machine volumes remain useful and which registry owns
  them; do not infer from names or attachment alone.

### Offline act2 maintenance progress (local candidate)

The act2 checkout now offers typed paginated `cache audit` and explicit
`cache prune --apply` for a namespace with no running server. A RED repro left
160 bytes above a 100-byte ceiling; GREEN leaves the newest 80-byte archive.
The audit creates nothing for absent stores and reports missing, busy and partial
states separately. It refuses unknown/untracked inventories before deletion.
Pages are capped at 12 entries and expose metadata/file-size fingerprints;
these are consistency checks, not archive-content hashes. Metadata, temporary
files and allocated-block accounting remain separate from completed-archive quota.

Artifactcache race tests and focused offline CLI tests pass. Updated artifactcache/CLI packages also pass pinned golangci-lint v2.11.4
with zero issues; the full CLI Docker result predates this extension. The candidate also reports bounded successful eviction receipts with typed
reasons, reclaimed completed-archive lengths, remaining/protected budget bytes
and an explicit budget-met flag. A protected over-budget namespace is not
reported as meeting its ceiling. Read-only mount proof passes on a disposable Docker volume: EROFS write probe,
accurate 160-byte inventory and byte-identical metadata. Sustained aggregate-budget
tests remain open. Legacy
servers do not honor the new transfer lock: Bosn still pins the released act2,
and this candidate must not evict mixed-version shared stores.

### Aggregate archive retention progress (local candidate)

Act2 now provides a cohort root lease: enrolled new namespaces share a root lock
for each server lifetime; aggregate maintenance holds it exclusively before
namespace locks/metadata. Live servers defer the aggregate pass, and new namespace
creation cannot cross a running pass. Existing legacy namespaces cannot silently
enroll. A new directory cohort is needed so released act2 peers never write it;
completed-archive import is now implemented locally, while automatic Bosn
quiescence/cutover remain open.

The aggregate pass validates all namespaces before deletion and refuses busy,
legacy, unknown, partial or oversized inventories. Up to 64 namespaces are
supported per pass, with bounded directory reads. Global UsedAt eviction preserves
recent-use protection and reports aggregate-budget receipts and protected overflow.
Two individually compliant 160-byte namespaces demonstrate the old aggregate gap
(320 bytes); the candidate global pass retains only the warmer repository's 160
bytes. Partial inventory in the last namespace preserves the first namespace.
The full artifactcache race suite, focused offline CLI/policy tests and pinned
lint pass for this slice. Namespace report ordering is deterministic.

This is an archive-length budget, not a physical machine footprint ceiling. Other
cache classes, metadata and temporary data remain accounted separately. Scheduled
Bosn integration/retry, automatic warm cutover, host image/build-cache expiry and sustained
concurrent workloads remain open; the production pin and cache path are unchanged.

### Warm migration progress (local candidate)

Act2 provides bounded completed-archive import from a quiescent legacy namespace
into a new cohort. The source metadata is held read-only; files and retention
timestamps remain unchanged. Recent completed archives are selected within an
explicit positive byte bound. The importer checks lengths, source/destination
SHA-256 and a final source inventory fingerprint, then publishes a synced staged
namespace by atomic rename under the exclusive cohort lease. Existing targets
are refused. Failed staging cleans only the exact generated directory or reports
its pending path; unpublished work has no successful import receipts.

A fixture with no transfer-coordination file demonstrates a real HTTP lookup and
download hit from a fresh candidate server after import. Source metadata stays
byte-identical and new reservations do not collide with imported IDs. Byte-bound,
overwrite/cancellation/corruption and changed-source staging cleanup cases are
covered. Import receipts are bounded to twelve details with an omitted count.
The full artifactcache race suite, focused offline CLI/policy tests and final
migration-specific race check pass; pinned lint reports zero issues.

This migration requires legacy servers to be stopped: holding their metadata
lock can fail active requests, and it cannot coordinate arbitrary file writers
outside that protocol. The CLI requires explicit quiescence acknowledgement;
Bosn must verify quiescence rather than treating that flag as evidence. Import
copies rather than removes legacy archives, so retained legacy bytes and temporary
duplication must be included in headroom. Bosn integration, automatic safe cutover,
retry, sustained workload and host image/build-cache expiry remain open.

### Online engine cleanup progress (local candidate)

Survey of current main found `CLEANUP_BUDGET = 180s`, so the older 60-second
failure in #445 is not the current literal budget. However, durable failed
retirements were retried only at startup; a running daemon could retain an engine
until restart. The candidate daemon now runs a separate cancellable retry worker.
Every pass scans at most 512 pending records and attempts at most one eligible
retirement, with a 200-second whole-pass deadline, then waits 60 seconds. A cursor carries across passes and wraps at
the end so early failures/active records cannot starve later cleanup.

Only registry `cleanup_required` records are eligible, and a tracked CI run must
already be done. Active claims are neither interrupted nor cleared. The existing
backend still verifies registry ownership and exact immutable container identity,
authorizes removal, requests `container rm --force --volumes`, and proves container
name/ID absence before recording a terminal receipt. Retrying retirement expires
that engine's private nested image/build-cache store. New v2 profiles require
independent named-storage absence proof, as described below; legacy anonymous
profiles retain their earlier removal contract. The named warm cache remains
protected. Successful retry updates the
run's cleanup field but preserves its original execution verdict and history.

All three isolated synthetic-host tests pass: recovery without daemon restart,
preservation of an active claim, and cursor fairness across failed retirement.
Strict Clippy for all bosn-service targets passes, as do source-length/include
checks. The timed daemon worker and disk-backed fault paths still need live proof. No disk-backed timeout or partial-create replay
has yet proved #445/#452 resolved on current main. Legacy anonymous-volume absence
is not independently recorded; the v2 named-storage slice below adds the required
volume reconciliation proof for new engines. Host base
images and shared cached image/tool artifacts also need separate expiry policy.

### Independent private-storage retirement (local candidate)

Evidence: anonymous disk storage names were observed only inside container
inspect responses. They were neither committed to the intent nor required in
terminal receipts. If `container rm --volumes` removed the container but left its
volume, a later container-absence probe could finalize cleanup without identifying
that storage. This loses the nested image/build-cache retirement evidence.

New disk profiles use `named_disk_storage_run_tmp_noexec_v2`: a local, labelled
`bosn-act-storage-<immutable intent UUID>` volume is created after the durable
intent commits and before container creation. Its scope is spec, retention warm,
with exact registry/run identity. The pinned shared cache remains separate.
Container declarations and observed mounts must bind the exact storage name.
Existing anonymous v1 profiles remain observable for recovery; new creation uses
v2 only. Older executables do not understand v2 profiles, so downgrade into a
registry containing them is unsupported.

Cleanup proves the container absent, verifies any remaining private volume's
exact labels/local driver/empty options, removes only that named volume without
force, and then proves its absence by a successful list. The registry requires
the exact storage identity in v2 terminal receipts, even when partial creation
never produced a container ID. An attached, foreign or pinned volume is refused.
A failed/unavailable probe is not absence; failed or uncertain removal stays
pending. Storage control commands each have a ten-second budget; retirement
reserves an additional forty seconds for them. Startup's pass budget is now
180 seconds so the required 135-second named-storage removal reservation can
fit; the former 120-second startup bound would always defer a present v2 engine.
The online retry retains its 180-second retirement and 200-second whole-pass bounds.

Transport tests cover unreadable inspect, attached volumes, lost acknowledgement,
reported success with surviving volume and later successful absence reconciliation.
The durable registry test rejects container-only/wrong-storage receipts and
verifies the correct identity survives reopen. The full isolated service library
suite passed 397 tests (four ignored), and all 18 registry lifecycle tests passed.
The focused bound-spare test passes, and strict Clippy passes for all service
and registry targets. Online retry now follows a spare's run binding for its active-run guard
and cleanup update, rather than looking up the original spare UUID as a CI run.

This provides a stronger expiry contract for new private nested image/build-cache
storage. Legacy anonymous storage still lacks independent survivor identity, and
host base images/shared cache artifacts need separate policy. Live Docker
fault/restart/partial-create replay is still required: these checks do not prove
late Docker-create side effects resolved or claim #445/#452 closed. No existing
host data was converted or removed; cache cohort integration remains open.

Live v2 verification passed all six scenarios in a fresh, isolated Docker
29.7.2 engine (457.26 seconds): a later job restored `actions/cache` from an earlier
job, a second fresh engine restored prior-run cache contents, repository
namespaces remained isolated, and spare reuse/daemon-stop cleanup succeeded,
all with v2 private storage enabled. Success, failure, timeout, client kill,
daemon kill/restart and the recorded matrix/failure report also passed. A final
volume listing contained only the retained `bosn-ci-cache-v1`, with no private
storage volumes. This proves those tested lifecycle paths, not partial-create
or lost-acknowledgement recovery against real Docker. The live isolation check now
also asserts that `/var/lib/docker` uses the intent-derived named volume; that
additional assertion passed in a focused lifecycle rerun (215.86 seconds).
Host snapshots already compare all owned
volume identities before and after retirement.

Shared-cache and private-storage inspection now deserialize the same typed
Docker volume response before ownership checks. The shared cache still accepts
a valid different registry's machine cache; local driver/scope, empty options
and identity labels remain mandatory. Ten focused accounting tests and the
new malformed-label/foreign-driver/bind-option boundary test pass in isolation.
Strict Clippy for all service targets and source-length/include checks also pass.

### Machine-wide owned volume breakdown (local candidate)

`bosn scan --json` now adds `owned_storage`, derived from the same read-only
Docker census. Valid label sets from every registry are included and grouped
as shared CI cache, private CI storage, or other Bosn volumes, with attached and
detached object counts. Malformed Bosn labels make the summary partial. Names
alone never establish ownership. Missing sizes keep the class byte total null,
rather than reporting a known subtotal as the complete footprint.

These are Docker's rounded approximate sizes, not allocated-block samples. They
must not be added to the shared-cache `du` result, because that would count the
same volume twice. This summary grants no deletion authority. The bounded
correlation slice below adds registry cleanup state. Legacy anonymous volumes remain
outside this labelled breakdown. The initial four aggregation tests pass.
Before the correlation slice, the CLI built and a read-only live scan succeeded
in JSON and readable modes: one detached retained shared-cache volume,
805,100,000 approximate bytes, zero private CI volumes, and no partial reads.
Final strict Clippy verification passes for all service targets, including the
readable-output change. Source-length/include and diff checks pass.

The next local slice adds up to 64 private-volume detail rows (largest/unknown
first, with an explicit omitted count). `scan` opens only its selected registry
read-only and correlates each matching owner/intent-derived volume with the
durable engine state and any spare run binding. Foreign registry, unavailable
registry, absent record, unreadable record and identity mismatch are separate
typed outcomes. Correlation is diagnostic and cannot authorize deletion. This
slice passed six focused isolated accounting tests, including persisted
cleanup-required state, unavailable/foreign/missing history, and bounded details
without losing object totals or hiding unknown sizes. Strict Clippy passes for
all service and registry targets after extracting volume classification into
its own function. All 18 registry lifecycle regression tests pass. The final
CLI built and a live read-only scan of a synthetic labelled private volume in
the isolated engine reported its exact intent/registry identity, detached status,
zero bytes and unavailable lifecycle history (not matched cleanup authority).
The synthetic volume was then removed; the retained cache was preserved.
The registry read-only handle now exposes the same bounded engine-record decoder
as the writer handle (one row, at most 32 KiB), including registry-owner checks.
The persisted cleanup-required correlation test verifies this read-only API.

### Concurrent shared-input staging (local candidate)

The act tarball and runner-image tar previously staged as `<path>.$$`. Shell
PIDs are container-local, so distinct engines can collide on the same shared
staging file. Both paths now use `mktemp` in the destination directory, followed
by validation/save and atomic rename. A normal shell exit cleans the unique
stage. Abrupt engine death can still leave a stage; stale-stage expiry and
coordination to avoid redundant concurrent downloads remain open. The focused
cache-script test passes. A live primitive probe ran two separate network-disabled
containers against one synthetic volume: both had shell PID 1 and the identical
legacy `/stage/input.1` path, while `mktemp` returned distinct shared-volume paths.
The synthetic containers and volume were removed afterward. This proves the
cross-container naming fix, not concurrent full artifact-download behavior or
crash-stage expiry. Strict Clippy for all service targets and source-length/diff
checks pass.

The next shared-input slice adds a per-artifact `flock` on a persistent
shared-volume lock file. The pinned Docker image provides BusyBox `flock`.
New engines take an exclusive lock before checking/downloading the act tarball.
Runner archives load under shared locks, allowing concurrent warm restores;
a missing/failed restore upgrades to exclusive and rechecks before pulling or
publishing. Refresh takes the exclusive lock before
removing an invalid tar. File descriptors release on process/engine death;
lock files must not be unlinked, which could split coordination across inodes.
Legacy executables ignore these locks. A live two-container probe verified that
an exclusive holder excludes another process, and killing the holder releases
the lock without deleting its file. The synthetic resources were removed. Full
artifact-level concurrency/failure verification and final checks remain pending.
An isolated test now executes the production runner-cache shell with a
deterministic Docker transport: two cold callers issue exactly one pull/save;
two warm callers overlap their loads and issue no pull/save. It passes in
0.88 seconds. This verifies shell coordination with mocked Docker operations;
live artifact correctness and failure/stale-stage cleanup remain open.
The concurrency test now uses a warm-reader barrier instead of timing sleeps.
A failed-save injection reproduced a false success: the shell reached image
inspection after publication failed. Runner and act publication chains now
explicitly exit on failure, before consuming or reporting the cached input.
The failure test also checks stage cleanup and subsequent lock reuse; it went
from RED to GREEN and passed in 1.32 seconds after the explicit failure exits.
Final strict Clippy for all service targets and source-length/include/diff checks
pass. The real fresh-engine cache-restore rerun passed in 47.25 seconds with
the new lock/publication shell against the pinned Docker image. It verifies
cache contents restored across distinct engines; concurrent cold live hydration
and interrupted-stage expiry remain open.

### Repeated act2 warm/retention workload (local candidate)

Eight cycles now exercise imported warm data, fresh HTTP servers, cold namespace
growth and aggregate maintenance. Each adds 160 cold bytes; active servers defer
GC, then idle maintenance stays within a 160-byte completed-archive ceiling while
preserving the imported 80-byte warm archive. A final fresh server restores it.
The race-enabled workload/import tests pass (1.176 seconds), and pinned Go lint
reports zero issues. This verifies act2 handler/file behavior, not Bosn's automatic
maintenance loop or a physical machine ceiling. Namespace metadata and retained
legacy-source bytes remain outside this archive cap; automatic cutover remains open.

### act2 review: bounded inventory and interrupted deletion (local candidate)

Review found that `filepath.WalkDir` materialized whole directories before the
advertised entry/time checks. Inventory now reads 256 entries per page, with one
shared 100,000-entry ceiling and a 64-directory depth ceiling. Focused tests cover
oversized directories, cancellation and excessive depth.

Review also found that interruption between archive removal and metadata deletion
could permanently block offline maintenance. A typed deletion intent is now
committed before file removal; cache metadata and intent are deleted in one final
transaction. Offline preflight accepts missing archives only with a matching intent.
Ordinary audit remains partial until recovery; unknown missing files still refuse
maintenance. Cohort recovery starts only after every namespace passes preflight.
Recovery receipts count zero newly reclaimed archive bytes when the file is already
absent, and a second pass emits no duplicate deletion. Lookup preserves missing-file metadata so it cannot orphan the intent before
recovery. Interruption, intervening lookup and cohort deferral tests pass; the full
cache package passes race testing (17.315 seconds),
and Go lint reports zero issues. These changes remain unshipped and do not change
Bosn's released act2 pin or close the automatic maintenance/cutover gaps above.

### Automatic act2 shutdown retention (local candidate)

An explicit `--cache-server-cohort-max-bytes` policy now invokes aggregate
maintenance after the cache server stops and releases its cohort lease. Active
peers/transfers defer collection; the last normal shutdown retries under the
existing five-second operation limit and full namespace preflight. The typed
`RetentionOnClose()` outcome and `cache_retention` log field distinguish partial
or deferred work from completed collection and protected overage. Repeat close
does not repeat maintenance. The CLI closes on function exit, including watch
exit and early errors, and keeps cleanup independent of workflow cancellation.

RED: shutdown left 240 completed archive bytes under a 160-byte ceiling. GREEN:
eight cycles add cold data, restore the warm 80-byte archive through fresh
servers, and each shutdown leaves at most 160 bytes without an explicit prune.
The cache package passes race testing (16.969 seconds); active-peer deferral,
last-peer retry, protected overage, cancellation and lease release are covered.
CLI policy/offline checks and lint cover the new flag and context wiring.

This closes the normal act2 shutdown-to-maintenance handoff, not the host retry
or Bosn integration gap. Killed servers and deferred/protected work require host
maintenance. Bosn still pins the released build without this flag. The policy
remains opt-in; physical machine bytes, metadata and retained legacy inputs are
outside the completed-archive ceiling. Warm cutover and automatic Bosn retries
remain open.


### Durable accounting-helper recovery (candidate, not released)

Accounting requests now commit a typed helper intent through the sole registry
writer before Docker create. The existing event ledger stores Pending, Created
and Removed states; no second database or schema migration is introduced.
Created records retain the immutable container ID before start or removal.
Identity freezes the image digest, cache volume, nonce and ownership labels.
Recovery verifies these plus the read-only mount and container isolation before
removing that exact ID. Successful receipts require observed absence. Missing
name lookup after uncertain create remains Pending because create may arrive
later; known-ID absence can finish a Created record.

An in-process active guard is installed before intent visibility and released
on return or cancellation. The maintenance worker skips active measurements,
pages at most eight batches of 64 records, and attempts one inactive helper per
pass. Its helper cursor is independent of engine retirement and advances past
deferred records. Engine and helper passes each have a 200-second outer budget;
the worker sleeps 60 seconds after both passes, so this is not a fixed
60-second cleanup deadline. Output paging is bounded; internal SQL scan cost
and accumulated completed ledger history are not a physical storage ceiling.
Shared volumes and foreign or mismatched containers are never removed.

Verification in the isolated Rust container: registry library tests passed
(5), service library tests passed (414; 6 ignored), and the focused accounting
and helper suite passed (20, including its real-Docker replay). Registry tests
close and reopen the writer between transitions and verify immutable IDs,
owner checks, nonce reuse refusal and paging past completed history. The real
private-engine replay discards a successful create acknowledgement and makes
recovery inspection unavailable, leaving a persisted Pending intent and an
actual orphan. A fresh backend recovers the verified ID from that ledger,
removes it and records Removed. A subsequent tracked measurement returns
positive allocated bytes with the shared cache intact.

This proves durable state and backend replacement recovery. Actual daemon
SIGKILL during helper creation and a timed background-worker recovery replay
remain unverified. Engine partial-create failures in #452 are separate; helper
recovery does not establish their resolution. Bosn still pins the released
act2 without the candidate aggregate retention policy, so this change does not
yet establish automatic machine-wide byte control.

### Supervised act2 aggregate retry (local candidate)

The candidate now adds `cache prune-cohort --apply --watch 1m`, emitting a JSON
report for each bounded pass and retrying busy or incomplete outcomes after
the interval. This provides a maintenance process independent of cache-server
shutdown, including stores left behind by crashed servers. It uses existing
exclusive cohort leases and refuses incomplete or legacy inventories before
deletion. Single-pass failures retain their error exit; invalid intervals and
policies fail before maintenance. Cancellation stops future passes.

Focused CLI race tests turn the missing --watch repro GREEN and verify repeated
partial reports plus recovery after an actual running handler releases its
root lease. The retry lease test uses an empty store; byte convergence is still
supported by the separate eight-cycle archive workload. Bosn does not yet
launch or supervise this command, and its released act2 pin lacks it. Safe
machine-wide warm migration, old-peer exclusion, headroom and physical storage
admission remain open. This is a tested maintenance mechanism, not evidence of
automatic Bosn byte-budget enforcement.

The watcher now also has a retained-data workload proof: real metadata/storage
fixtures contain 240 completed archive bytes with shutdown retention disabled.
An age-only pass leaves them intact; the watched CLI's 80-byte aggregate budget
removes the eligible cold entries and leaves the recently used warm entry.
A fresh server retrieves that entry and its exact bytes through loopback HTTP.
The focused watcher race tests and pinned lint pass. This closes the earlier
empty-store-only verification limitation for watcher byte collection; it still
does not prove actual SIGKILL handling or Bosn supervision.

### Candidate handoff and graceful cancellation

The act2 implementation is published for review as draft
[act2 PR #22](https://github.com/zackees/act2/pull/22), head `8a2e84f`.
The complete candidate diff and the graceful-interrupt follow-up passed the
single-reviewer local gate. A fresh full artifactcache race run passed in
16.778 seconds; focused CLI policy/offline/watcher race tests and pinned lint
also passed. A candidate CLI was built inside the isolated Go container.
Sending its maintenance watcher the first real SIGINT after its initial JSON
report produced a successful exit. The watcher now combines the CLI's graceful
job context and force context through the existing EarlyCancelContext helper;
the focused repro previously required force cancellation and now passes.

Latest published act2 remains `v0.2.89-act2.3` as checked during this handoff.
The draft introduces no version bump or release. It is not merged, not a
verified release artifact, and not a Bosn pin update. Bosn policy arguments,
independent helper supervision, verified warm migration and physical pressure
control remain required integration work.

### Owned-volume warnings in doctor (local Bosn candidate)

`bosn doctor` now renders owned-volume footprint warnings from its existing
machine census, before unmanaged warning/acknowledgement handling. No second
Docker scan is added. It uses the existing default thresholds (5 GiB or 25
objects) across the nonoverlapping owned volume classes. Partial accounting or
unknown sizes also warn when owned or invalid-labeled volumes are observed.
An unavailable empty census does not claim an owned footprint.

Warnings show each observed class's Docker approximate bytes and attachment
counts, link users to `bosn scan --json`, and direct shared-cache owners to
`bosn ci runners cache` for actual block accounting. They preserve retained
volume release contracts and convey no removal authority. They remain on
stderr, so `doctor --json` stdout keeps its existing report shape. An unmanaged
acknowledgement does not suppress the independent owned warning.

Two focused RED warning signals became GREEN; all eight owned-accounting tests
pass in the isolated Rust container. Strict Clippy for all service targets,
source-length/include gates and slice review pass. This is a visibility
improvement, not a free-space pressure controller or automatic owned GC. No
new live host measurement was required for this warning change.

### CI coverage scope for act2 PR #22

The existing PR workflow's Windows and macOS lanes select only
`TestRunEventHostEnvironment`; their success is host-environment coverage,
not execution evidence for the newly added retention, transfer-lock or import
tests on those operating systems. The snapshot lane builds the release target
matrix without publishing. Linux runs `./...` with coverage and a 20-minute
per-package timeout, then CLI smoke steps. Its test step was confirmed live
on job `111376046334` in run `37181922413`; no replacement run was started.
Lint, spelling, snapshot and both host-environment lanes had completed
successfully at that observation. Linux completion and end-to-end Bosn quota
integration remain unproven until their corresponding evidence exists.

### Verified CI snapshot available for integration experiments

Downloaded Linux amd64 artifact `11295491838` from act2 run `37181922413`.
The extracted binary SHA-256 is
`ae923c1fdba91a95b358218e767b3f8e82085574de6e127704be34aabfd9af45`.
Its build information identifies clean revision
`52a1ce794a4cdbcd0983b1fdf92e1bf0a628f0cc`, Linux amd64, CGO disabled,
and version `0.0.0-SNAPSHOT-52a1ce7`. GitHub's commit record confirms that this
is the PR merge snapshot with parents `60c84bbf` (current master) and `8a2e84f`
(the reviewed candidate). It is not the PR branch SHA itself.

The actual downloaded binary was executed only in the isolated Go container.
It exposes the quota/cohort/watch flags, emits a typed partial report for a
missing maintenance root without creating that root, and exits successfully
on its first real SIGINT. This provides a provenance-checked executable for
local integration experiments while Linux CI continues. The artifact is a
CI snapshot, not an approved release or permanent download pin; Bosn's
released artifact/version checks remain unchanged. Warm workflow execution
with this binary and automatic Bosn policy/supervision are still unverified.

### Warm migration headroom admission and native publication

Act2 PR #22 now includes commit `0ae797d`. Import measures destination
caller-available free bytes before archive copying and refuses unknown or
insufficient space. The additional-space estimate is selected apparent
archive lengths plus 64 KiB per archive and 64 MiB metadata headroom. Reports
expose retained source archive bytes separately: when both stores use one
filesystem, its current available space already accounts for retained data.
This estimate is not a reservation against concurrent writes or an exact
allocated-block budget, and does not implement Bosn's machine pressure floor.

Focused RED refusal/accounting cases now pass. Failure-path tests leave source
metadata and archives unchanged, remove the generated stage, and publish
nothing. Native probes use Linux/macOS filesystem available blocks and Windows
GetDiskFreeSpaceEx; other platforms explicitly refuse unknown space. Linux
import race tests pass, the artifactcache race suite passed in 16.874 seconds,
pinned lint reports zero issues, and Windows amd64/macOS arm64 cross-builds
pass. Local review passed after fixing an exposed Windows publication bug.

Windows now uses same-volume MoveFileEx with WRITE_THROUGH without replacement,
after file/metadata close. Unix retains rename plus parent-directory sync and
truthfully preserves Published after rename even on a later sync failure.
Power-loss durability on arbitrary Windows filesystems is not established.
Existing host CI jobs now include TestImport* to execute these native paths;
no workflow files or runners were added. All CI checks passed for the previous
8a2e84f candidate; the new head's native and full CI remain pending. Bosn warm
cutover, old-peer exclusion and maintenance supervision remain open.

## Native Windows coordination correction (candidate)

CI run 37182945345 at act2 `0ae797d` passed full Linux tests, macOS
import tests, lint and snapshot builds. Windows import tests failed before
publication: bbolt attempted to truncate an intentionally read-only
coordination descriptor while acquiring an exclusive lease.

Windows now validates the existing bounded coordination database read-only,
then takes a native exclusive LockFileEx lease over exactly bbolt's lock byte.
Shared leases retain bbolt's read-only locking protocol. No writable descriptor
or creation is permitted by inspection. Close releases the lock and handle
idempotently; acquisition failure returns a nil lease. Unix retains its
existing bbolt protocol.

Read-only file preservation, shared/exclusive exclusion, separate-process
transfer protection and import tests pass with the race detector in isolated
Linux Docker (2.124 seconds); pinned lint reports zero issues. The previous
full artifactcache race suite passed in 16.844 seconds after correcting a
typed-nil lease regression. Windows package cross-build passed. Existing host
CI now selects the lease and separate-process transfer tests as well as import
tests. Review found no concrete protocol defect; native Windows execution of
this correction remains pending. This does not complete Bosn policy wiring,
maintenance supervision, warm cutover or machine-wide physical budgeting.

## Warm-cutover store accounting (candidate)

The integration destination layout is `actcache/cohort-v1/<repository hash>`,
separate from retained legacy `actcache/<repository hash>`. The class and volume
totals already contain both. Namespace diagnostics now additionally enumerate
the cohort layout and expose optional `store_path`, relative to the shared
volume, so equal repository hashes cannot hide the retained source or import.
The CLI shows that path and remains compatible with older replies lacking it.
Top-256 namespace limits apply across both layouts; these separate non-atomic
samples must not be added to the inclusive class/volume total. This path is
accounting provenance, not deletion authority or proof of cohort enrollment.

An actual shell measurement in isolated Docker was RED (only one namespace
reported for two same-repository stores), then GREEN. The fixture checks both
paths, distinct apparent/allocated measurements, and exclusion of a symlinked
namespace. Malformed/uppercase cohort hashes remain partial. Targeted accounting
tests pass: 17 passed, two live-Docker cases intentionally excluded from this
slice. Strict all-target bosn-service Clippy also passes. Source length/include gates
pass and the single reviewer passed this
slice; full Bosn branch review remains unclaimed.

Act2 `578ce2b` native Windows and macOS jobs in run 37183643367 passed,
including imports, read-only lease exclusion and separate-process transfer
protection; lint also passed. Full Linux and snapshot jobs were still running
when sampled. This supersedes the pending native correction result above.
Bosn still pins the older released act2: no automatic migration, new-root
server routing, retention flags or maintenance supervisor is enabled yet.

## Command-level warm-cutover verification (candidate)

The integration test `TestCacheCutoverCommandsPreserveFreshServerHitsAndRepositoryIsolation`
uses actual loopback HTTP reservation, upload, commit, lookup and download. Two
stopped source servers have the same cache key/version but different 80-byte
payloads, under separate 16-digit repository identities. Cobra import commands
copy each store into `actcache/cohort-v1/<repository hash>` with an 80-byte import
bound; source metadata remains byte-identical. Three successive fresh servers
per repository download the correct distinct payloads after import.

A command-level aggregate pass with an 80-byte ceiling correctly reports
160 remaining bytes, 160 protected bytes and BudgetMet=false: both archives
were recently transferred. This verifies truthful protected overflow, not age
expiry or sustained convergence. Linux Docker race testing and pinned command
lint pass. Servers have immediate failure-path cleanup plus explicit close
before import. The existing native host CI selection includes this test; no
workflow/job/runner was added.

Production integration still requires a verified released binary pin, typed
machine policy, durable routing/cutover state, source quiescence across daemons,
and supervised maintenance independent of run teardown. Proposed command sequence:
`act cache import --apply --source-quiescent --from SOURCE --namespace HASH
--max-bytes IMPORT_BOUND --cache-server-path COHORT`; start servers with
`--cache-server-path COHORT/HASH --cache-server-cohort-root COHORT`, explicit
namespace byte/age policy and optional close-time aggregate policy; supervise
`act cache prune-cohort --apply --watch 1m --max-bytes AGGREGATE_BOUND
--cache-server-path COHORT` with the same namespace byte/age policy.
The source-quiescent flag is caller responsibility, not a detector of old
peers. This test does not authorize automatic source deletion or prove those
Bosn daemon paths are implemented.

## Other owned-volume contributors (candidate)

`bosn scan --json` now exposes `other_volumes` and `other_volumes_omitted`
within its existing owned-storage report. Each contributor identifies the volume,
registry, workspace, stack, generation, scope, retention, attachment state and
approximate Docker bytes. The 64 largest or unknown contributors remain visible;
class counts and size semantics still cover all observed objects. These are
not allocated-block measurements and cannot be added to shared-cache `du` totals.

A typed advisory inspection hint distinguishes registry-history investigation
from the existing read-only manifest durable-release preview for stack/machine
or pinned data. Preview must use the owning registry/workspace and is itself
responsible for establishing any eligible token. Labels, names, detached state
and this contributor list establish no deletion authority; foreign/untracked
and pinned data are preserved. No cleanup command was added or executed.

A RED test showed retained bytes lacked any contributor detail. After the fix,
ten owned-accounting tests pass in genuine isolated Docker. Strict all-target
Clippy and source length/include gates also pass. Tests verify
identity/retention visibility, unknown-first ranking, a 71-object/64-detail bound,
unchanged totals and foreign-registry handling. The single reviewer passed this
slice; full branch review remains unclaimed.

CI run 37183643367 for act2 `578ce2b` ultimately ended cancelled: Linux was
automatically superseded by the later test commit, not proven passing. Its
Windows/macOS/lint/snapshot jobs passed. At current `7902785`, run 37184103814
passed native Windows/macOS (including command cutover tests) and lint; Linux
and snapshot were still running at the latest sample.

## Full candidate review and schema correction

The single reviewer covered the full 45-file Bosn range through `224990a3`
against `146515cf`; no blocking ownership/replay/concurrency defects were found.
Full unit verification then caught a stale published schema for optional
`CacheComponent.store_path`: 417 service cases passed, one schema case failed,
and six cases were ignored. The earlier review assertion of schema parity was
incorrect. An actual generated schema fixes that mismatch; its comparison test
now passes. Registry library tests (five), workspace formatting and strict
all-target service Clippy pass.

A tiny stdout exporter, `soldr cargo run -q -p bosn-service --example
export_ci_schema`, generates the contract inside an isolated container without
writing its read-only source mount. The host can validate/copy its JSON output
into `docs/ci.schema.json`. No weakening of the schema parity gate was made.
These results precede reconciliation with newer main changes and do not prove
the merged candidate; current main is `5a5040fa`. Full verification after that
reconciliation remains required before pushing a candidate.

## Main reconciliation and exact-source gate

Merge `1527ee9` incorporates main `5a5040fa`, including unresolved-create
quarantine and authoritative source/run receipts. Startup recovery first
requires main's container absence proof, then independently removes/verifies
private storage before finalization. Both old and new test modules are retained.
The single reviewer passed the merge follow-up. The merged service library
suite passes: 429 cases, six ignored, no failures.

Main updates the production act2 pin to `0.2.89-act2.4`; that release still
does not contain this retention candidate. Act2 candidate `7902785` now has
all checks successful in run 37184103814: full Linux, native Windows/macOS,
lint and snapshot build. The earlier cancelled run is not substituted for it.

The current source-bound pre-push gate requires released Bosn >= 0.1.12 and
actual Rust/Linux workflow receipts. Host global Bosn is 0.1.10; an isolated
`uvx --from bosn==0.1.12` environment resolves and reports 0.1.12 without
replacing the user's global executable. Running the full gate/stamping the
candidate is the next required step before pushing this Bosn branch.

## Actual gate cleanup failure on released Bosn 0.1.12

The clean-source gate at `7179b6e` used isolated released Bosn 0.1.12
(act2 0.2.89-act2.3) and its own registry/state. Rust run
`e9701619-7ebb-4d51-8a91-c1001ce38b6e` passed with executed-step proof
in 489 seconds and reported cleanup removed. Linux run
`d58306d1-8da5-4af1-bff8-90046524e57a` completed all required workflow
steps successfully (act exit 0): Install, Lint, and Test. Python results
were 257 passed / 10 skipped; live Docker acceptance requires explicit opt-in.
The overall run nevertheless ended error: engine cleanup failed with
`Docker CLI exceeded its deadline`. The gate correctly refused attestation
and exited 1 after 1114 seconds; this branch was not pushed.

A subsequent exact-ID Docker inspection reported the Linux engine absent.
That late observation does not rewrite the failed run or prove independent
private-storage absence. No broad prune or user-daemon restart was performed.
This is current live evidence extending #445 beyond its previously observed
0.1.10 scope. Candidate online retry/named-storage behavior still needs to be
validated against this workload before calling the failure resolved.

Both successive fresh engines restored cached inputs. The Linux run restored
a roughly 693 MB archive; the Rust run saved tools in 0.3 seconds. Linux
native install prepared packages in 3m27s and uv reported a cross-filesystem
hardlink fallback to full copying. This demonstrates reuse plus a remaining
copy cost; it is not sustained convergence or a machine-wide physical cap.
Bounded raw observations and gate receipts remain in task-owned Git metadata
and logs. A draft PR body records scope and gaps, but push awaits a valid gate.

### Cleanup deadline diagnosis

Release tag v0.1.12 resolves to `08c7c8cdb262b980d56fd9fccbe9b6156fc6ec82`.
Its exact engine deletion uses the generic 30-second `docker_control` helper.
The candidate retains that per-command bound despite the 180-second outer
lifecycle budget: increasing the outer budget alone cannot extend deletion.
Candidate named-volume deletion likewise uses a 10-second storage-control
bound. The successful workflow followed by cleanup timeout and later engine
absence therefore exposes a concrete remaining deletion-budget gap, including
possible delayed completion after a caller timeout.

Next correction must distinguish short observation/create control operations
from potentially slower exact authorized deletion, account for all reserved
commands in startup and online budgets, and test a delayed removal that exceeds
the old bound. Timeout still leaves durable pending state; only exact container
and storage absence may finalize retirement. The existing named-storage/online
retry changes do not by themselves prove this deadline failure resolved.

### Coordinated deletion budgets (candidate; verification in progress)

The isolated process/registry regression now includes a 31-second authorized
container deletion. Against the old helper it failed (0 passed / 1 failed),
confirming the per-command bound independently of the outer lifecycle timeout.
The candidate separates exact deletion (90 seconds) from container observation
(30 seconds) and storage observation (10 seconds). Startup reserves 155 seconds
for legacy retirement or 275 seconds for named-storage retirement, including
independent absence probes and persistence. Lifecycle/startup/online retirement
allow 360 seconds; the online pass wrapper allows 380 seconds. Accounting helper
budgets remain independent. All values share one budget module.

Timeout continues to leave pending cleanup; exact absence remains mandatory.
These bounds do not guarantee disk-pressure convergence or resolve an arbitrarily
slow Docker removal. The container regression turned GREEN (1 passed); the private-volume regression
passed with an 11-second removal, exceeding its previous 10-second bound. Strict
service all-target Clippy passed. The existing reviewer found no blocking issues.
The full isolated service suite passed: 429 tests, 6 ignored, 0 failed.
A new exact-source workflow gate remains required before shipping this correction.

### Retention integration boundary (in progress)

Act2 PR #22 merged as `7e6f010fa7cc7171083329b2cf3d18f1566e3468`.
Full checks on that exact merge SHA are running in run 37186796787; no new
tag/release or Bosn pin update has been made. Typed `[cache]` configuration now
requires explicit positive repository/aggregate archive byte ceilings and
positive maximum-age, unused-age, and maintenance intervals in seconds. It
rejects unknown fields, inconsistent ceilings and act2 duration overflow.
These settings govern completed archive lengths, not a physical machine cap.

Production planning currently refuses configured retention with an explicit
rollout error. Activation remains dependent on a verified release pin, safe
warm import into the coordinated root, and maintenance supervision. Existing
legacy shared-cache execution remains the default. This preparatory boundary
does not claim that configured quotas are yet enforced.

### Live lifecycle verification at b0860ff

The candidate CLI built in the genuine isolated Rust harness. Against the
private Docker engine, `the_host_engine_is_unchanged_after_every_way_a_run_can_end`
passed (1 test, 282.13 seconds). After the warm-up baseline, exact owned-container,
network, volume and engine-image inventories returned to baseline after success,
failure, timeout, client SIGKILL and daemon SIGKILL/restart. The timeout scenario
verified intent-derived named storage, volume-only mounts and a nested Docker
engine distinct from its parent. Shared cache inputs persisted across the fresh
engines. This validates candidate lifecycle cleanup under these workloads, not
archive retention, sustained physical convergence or cleanup under full-disk
conditions. The separate exact-source gate still uses released Bosn 0.1.12.

The policy boundary test passed; both config tests passed, and strict service
library Clippy passed in the isolated container. Review caught a spare-planning
bypass; run planning, spare kickoff and spare planning now use the same
`load_engine` guard. The config test proves parsed policy is refused at that
boundary; it does not directly instrument backend creation counts. The existing
reviewer passed the corrected slice. Retention activation remains incomplete.

### Coordinated command mapping (candidate, not activated)

Bosn now constructs act2 server and independent watcher arguments from the
typed policy. Both use `actcache/cohort-v1`; repository namespace parsing only
accepts a 16-character lowercase hexadecimal direct child. Server and watcher
share the same byte/age settings. The watcher passes an explicit aggregate
ceiling and interval, rather than relying on server shutdown for idle cleanup.
These command builders are not yet invoked by production admission. Warm import,
legacy-peer exclusion, receipts, supervision and verified pinning are still
required before removing the planning guard. CLI contract validation is pending.

The two existing live Bosn cache tests passed at b0860ff (2 tests, 100.65
seconds): a later job restores an earlier job's archive, and a subsequent run
with a different engine ID restores and verifies the saved bytes. This proves
existing shared-cache reuse, not yet quota enforcement. Full act2 checks run
37186796787 passed on merge SHA 7e6f010, including Linux, Windows, macOS, lint
and snapshot. Dependency release approval is being prepared; no tag exists yet.
The namespace test and strict service library Clippy passed for the command
builders, and review verified flag mapping. Execution of those builders remains
unverified and production activation stays guarded.

### Dependency release and source-gate checkpoint

The exact-source Bosn gate at b0860ff passed Rust (477 seconds) and Linux
(485 seconds), reusing Python-static and passing guards. It exited successfully
and stamped the candidate as `445265cfa97e` after 962 seconds. This gate covers
the accounting/lifecycle candidate; it does not cover the later retention wiring.

The user approved act2 v0.2.89-act2.5 and established standing authorization
for necessary act2 releases in AGENTS.md. The annotated tag points to
`7e6f010fa7cc7171083329b2cf3d18f1566e3468`, whose full CI passed. Existing
release workflow run 37187501368 is running. Binary/checksum verification and
Bosn pin updates remain pending; no production retention activation is claimed.

### Verified act2.5 dependency pin (candidate)

Release run 37187501368 passed and published v0.2.89-act2.5 from the exact
full-CI-verified merge SHA 7e6f010. The Linux x86_64 archive matches published
checksums (`d116cc9e5ca040f4807763f562476275015e9d9bdf5c3a2ac598c182c0be0a9e`).
The extracted binary digest is
`ea682586fbf32aef140cfd2dc620d883be29f14650daadd6d1cd29ed34292ea1`;
its actual isolated invocation reports `act version 0.2.89-act2.5`. Bosn's
version, archive URL and both digests now pin these verified bytes.

The command contract test passed against the exact-merge snapshot: namespace
audit reports missing with null counts/bytes; missing-root watcher reports
partial/unknown budget outcome and stops successfully on its first SIGINT.
The initial fixture mistakenly expected missing audit to be partial/nonzero;
actual execution and review corrected the test, not the production contract.
Strict service-library Clippy passed. The published-binary contract test passed (1 test, 0.05 seconds). Production policy activation remains guarded pending warm migration
and supervision. Accounting/lifecycle PR #473 merged as
`b23398aef9f9c12eb314e0835ca7a508391a07fd` after required checks passed;
full hosted macOS/release checks were not requested or claimed.

### Warm migration refuses a budget-induced cold cutover (act2 follow-up)

Inspection and a focused RED test exposed an act2.5 import gap: two completed
80-byte source archives with a 79-byte import ceiling published an empty
cohort destination. The retained source survived, but destination existence
then prevented a warmer retry. This conflicts with warm migration.

The act2 candidate now refuses before staging/publication if completed
archives were skipped for budget and none fit. Tests prove destination absence,
unchanged source metadata, a successful 80-byte retry and actual HTTP restore
of the retained archive. Genuinely empty-source initialization remains allowed.
Import race tests passed (1.177 seconds), and pinned lint reported 0 issues.
This follow-up is not yet released: Bosn's act2.5 pin still has the old edge
case, and production enrollment remains guarded. A corrected dependency release
and verified pin are needed before automatic migration can be enabled.

### Typed warm-import boundary (candidate, not activated)

Bosn now builds source-quiescent import arguments using the validated repository
hash, legacy source path, fixed cohort root and repository byte budget. Calling
this builder requires prior legacy-writer exclusion; it does not establish that
exclusion. Production planning remains guarded.

Import JSON is eagerly decoded into typed reports and receipts with a 64 KiB
input bound. Parsing checks schema, exact source/destination identity, policy
ceiling, at most twelve receipts, unique positive archive IDs, lowercase SHA-256
digests, count/omission consistency and checked byte totals. A separate warm
publication check requires complete publication, no pending stage/error, known
adequate headroom, retained-source evidence and a nonempty import when the source
is populated. Published-but-partial output remains visible for reconciliation;
it is not converted into absence or permission to blindly repeat import.

Three isolated boundary tests passed (0.01 seconds), strict service-library
Clippy passed, and the existing reviewer found no blocking issues. These tests
use typed receipt fixtures: they do not prove actual automatic migration,
durable migration receipts, peer exclusion or supervision. Act2 cold-cutover
correction is tracked in PR #27; full CI is still running.

### Participating legacy cache lifetime lease (candidate)

Workflow and act-list invocations now acquire a shared FD 8 lock on
`/bosn/cache/actcache/.legacy-migration.lock` before executing act. The descriptor
survives exec and spans the server lifetime; OS process death releases it.
Independent readers can run concurrently. New cached engine profiles freeze
`SharedLegacyLeaseV1` and advertise the corresponding coordination label.
Historical profiles omit the optional field, preserving their identity digest.

The isolated process test passed (0.11 seconds): two readers coexist, exclusive
migration is refused while either lives, killing the last reader releases the
lease, and literal arguments survive the shell boundary. Engine tests passed
(19), registry lifecycle tests passed (18), and spare tests passed (5). The
existing historical digest assertion passed unchanged. Strict service
all-target Clippy passed.

This coordinates participating writers only. Older or unknown peers have no
proven exclusion; instant inventory absence cannot prevent their future
admission. Automatic migration remains guarded. Real workflow execution with
the new wrapper, durable migration receipt reconciliation and watcher
supervision remain unverified. Act2 PR #27 merged as
`d874c3b6e9518e11c3137da8dd810f1f3f361db1`; exact-merge full CI run
37188971746 is in progress, so the correction is not yet released.

The private-Docker `ci_live` warm-cache tests subsequently passed with the
new lifetime wrapper and frozen profiles (2 tests, 61.07 seconds): a later job
restored an earlier job's archive, and a successive run restored it through a
distinct fresh private engine. This verifies continued warm legacy sharing;
it does not verify cohort migration or retention-policy activation.

### Verified act2.6 pin and historical receipt boundary

The published act2.6 CLI contract test passed (1 test, 0.16 seconds), as did
four pin/proof consistency tests. The dependency is verified; production cache
policy remains guarded. Four typed import/receipt boundary tests passed and
strict library Clippy passed. Historical receipt validation shares the same
checked archive count/digest/byte logic as import reports, binds both source
and destination to the repository namespace, and requires a valid source
fingerprint, original import ceiling, current repository ceiling and known
retained-source warmth. It refuses populated-source empty evidence.

The record is creation-time evidence only: current inventory, durable routing,
publication reconciliation and old-writer exclusion remain caller obligations.
This parser is prepared for act2 PR #28 and is not invoked against act2.6,
which does not include the receipt command. Independent review found no blockers.
Issue #456 now records merged accounting work and exact dependency release
evidence while leaving the full acceptance criteria open.

The actual private-Docker workflow tests also passed with the verified act2.6
pin (2 tests, 140.91 seconds): a later job restored an earlier job's archive,
and a successive run restored saved bytes through a distinct fresh private
engine. These exercise artifact verification and the legacy lease wrapper in
real runs. They establish continued warm reuse, not policy activation or
sustained image/container/build-cache footprint convergence.

### Exclusive import lifetime and truthful command transport (candidate)

Bosn's import executor now uses a nonblocking exclusive FD 8 legacy-session
lease across the actual act process. Participating live sessions refuse import
immediately; an importer excludes participating readers until process death.
The OS releases the lease on termination. Older peers still need independent
exclusion, and the caller must persist intent before invoking this method.
Production planning remains guarded.

Import execution has a 90-second command deadline and a 64 KiB output bound.
It parses stdout even on nonzero exit, preserving published-but-partial evidence
instead of discarding it through a generic checked-command helper. Command exit
and publication report are separate typed facts: warm acceptance requires exit
zero plus complete valid warm publication. Transport/parsing failure leaves
publication unresolved; it is never converted into destination absence or
permission to blindly repeat import.

Two isolated synthetic transport tests passed (0.09 seconds), including a
nonzero exit with published/partial output and contradictory success evidence.
The actual process lease test passed (0.55 seconds): readers coexist, import
refuses contention, importer excludes readers, and termination releases both
lease modes. Strict service all-target Clippy passed. Independent review found
no blockers. These tests do not prove actual automatic migration or supervision.

Act2 receipt PR #28 merged as `1a6782d5bbabb715f425cad5601b1f706fc23fa8`
after full PR CI passed. Exact-merge full CI run 37190653388 is in progress;
the receipt feature remains unreleased and absent from the act2.6 candidate pin.

### Tool-cache save isolation and efficient fresh-engine seeding (candidate)

A focused RED test found that the old whole-root `cp -a` seeding copied hidden
`.saving-*` directories into every fresh engine. A second RED test replayed
equal PIDs from separate private engines with a controlled copy barrier: both
publishers used the same `.saving-$$` path. One could remove or reuse another's
unfinished stage, despite their different install destinations.

Save now creates an exclusive `mktemp -d` stage for each install, with one
shared shell setup function. Cleanup targets that invocation's own stage. Seed
copies visible published top-level entries and skips hidden stages/control
files. Sibling completion markers, inner stamps and existing warm entries
retain their existing behavior. The collision fixture terminates only its
spawned process groups, including blocked copy children on failure.

Four actual shell/copy tests passed (0.52 seconds), including both corrected
RED signals and the two existing completion conventions. Strict service
all-target Clippy passed (71.37 seconds), and independent review found no
blockers. This proves deterministic equal-PID isolation and avoids private-disk
duplication of unfinished saves; it does not introduce tool-install TTL/quotas,
abandoned-stage pruning or improve every pre-existing copy-failure path.
Real engine workflow coverage of this new seed/save slice remains pending.

### Verified act2.7 artifacts and pinned-engine tool-cache shell

The act2.7 release workflow 37191477610 succeeded on exact merge commit
`1a6782d5bbabb715f425cad5601b1f706fc23fa8`, whose full CI run 37190653388
also succeeded. Published archive checksums and extracted binary identity were
verified before changing the candidate pin. Four pin tests passed (0.01 seconds),
and the published audit/watch CLI contract passed (0.12 seconds). This command
test does not yet verify Bosn parsing an actual historical receipt.

The actual pinned engine image's shell executed the generated tool-cache save
and seed scripts successfully (1 test, 0.23 seconds). The helper had a read-only
root, no network, dropped capabilities and only disposable tmpfs cache/work
paths. It restored both sibling and inner completion conventions while excluding
a hidden orphan stage. Its immutable container ID was confirmed absent afterward.
This verifies the BusyBox shell path; actual workflow tool-install activity,
expiry and quotas remain unverified. Strict service all-target Clippy passed
after the shell test and new pin (28.59 seconds). Independent review passed.

### Actual Bosn import transport and published historical receipt

The isolated private-Docker proof passed using published act2.7 through
`DockerActBackend::import_cache_for_quiescent_source` (1 test, 2.83 seconds).
A Go HTTP fixture reserved, uploaded and committed an 80-byte archive, then
closed its legacy server before exporting the source. The test imported that
source through the actual exclusive lease wrapper and required exit zero plus
complete warm-publication evidence. It then called the published historical
receipt command and parsed its output through Bosn's `PublicationReceipt`.
The receipt identified 80 imported bytes; source file names and SHA-256 hashes
matched before and after import. Exact helper-container absence was verified.

The helper mounted only disposable tmpfs paths, had a read-only root, no network
and dropped capabilities. File streaming uses Bosn's existing bounded transport;
failed operation checks return through exact cleanup. Docker auto-removal and
a finite helper lifetime also cover a lost acknowledgement or panic. Initial
fixture errors concerned container absence wording, directory enumeration and
the absolute act binary path; they are not production RED signals. Independent
review passed after fixing unbounded fixture streaming and panic-before-cleanup.

Reproduce with the opt-in test
`published_binary_import_preserves_source_and_has_typed_historical_receipt`,
`BOSN_ACT_RETENTION_TEST_BINARY` pointing at the verified release,
`BOSN_ACT_IMPORT_SOURCE_TAR` containing a closed HTTP-seeded legacy namespace
`0123456789abcdef`, and the isolated Docker transport. This verifies actual
command/schema interoperability and source preservation. It does not establish
automatic enrollment, old-peer exclusion, crash-safe routing, watcher supervision
or restored workflow hits from the new cohort. Those remain activation gates.

Strict service all-target Clippy passed after the final import fixture changes
(60.84 seconds). The proof and review are complete for this slice; the full
source gate is still required before pushing the retention candidate.

### Durable local import intent and historical publication recovery

The registry stores a bounded typed migration record under a namespace-specific
indexed `meta` key, without a schema migration or second database. An immediate
transaction commits the nonce, namespace, original import ceiling and creation
time before import. A second begin is refused even for the same nonce; an
unresolved import must be reconciled rather than repeated blindly. Publication
contains the historical source fingerprint, imported count/bytes and retained
source bytes. Conflicting publication never replaces the recorded evidence;
identical recovery is idempotent. Rollback leaves no intent.

Bosn's recovery method reads the actual act2 historical receipt with a 30-second
deadline and 64 KiB output bound. It requires an existing local intent, exact
legacy/cohort namespace paths, complete typed receipt checks and the original
import ceiling. Missing, malformed or failed reads leave the intent unresolved.
Successful evidence is committed in an immediate transaction. This call does
not rerun import or delete source data.

Two isolated registry tests passed (0.44 seconds), covering rollback, reopen,
duplicate refusal, conflicting nonce/fingerprint, invalid warm byte evidence,
invalid time and idempotent recovery. The actual private-Docker published-binary
proof passed (1 test, 4.28 seconds): missing receipt first remained unresolved,
then a closed HTTP-seeded 80-byte archive was imported, the command acknowledgement
was discarded before journaling publication, and the registry was reopened.
Recovery recorded the actual act2.7 receipt and repeated safely. Source hashes
remained unchanged and exact helper-container absence was confirmed. Independent
review passed.

This is local recovery evidence, not daemon-wide automatic startup reconciliation
or machine-wide routing. The act2 historical receipt does not carry Bosn's nonce,
so this journal does not prove correlation to a unique import invocation. Before
enrollment, callers still need current inventory, publication durability, shared
routing state and exclusion of older writers. The production configuration guard
remains in place. No automatic migration or physical storage ceiling is claimed.

Strict registry/service all-target Clippy passed (50.64 seconds) after correcting
a collapsible conditional flagged by the first lint attempt. The task Docker
engine also reported no remaining import-proof helper containers. The retention
candidate remains local pending the exact-source gate and broader activation work.

### Typed routes reach actual workflow execution

`ActInvocation` now carries a typed `CacheRoute` instead of an arbitrary namespace
string. Legacy routes use the validated direct-child repository path; cohort
routes pass the shared root, repository/aggregate byte ceilings and age/interval
flags through the existing policy builder. There is exactly one cache-server
path in the execution argv. The production planner validates its derived
namespace and selects `Legacy` explicitly; it cannot infer enrollment from a
local journal or historical receipt. Retention configuration remains guarded.

The published act2.7 CLI accepted the full workflow planning argv for both routes
(1 test, 1.12 seconds). Four engine invocation tests, eight parameter tests and
fourteen lifecycle tests passed. These preserve runner pinning, overlay selection,
secret handling, execution claims and cleanup behavior.

An isolated real owned-engine proof then passed across two fresh engines
(1 test, 42.43 seconds). The first run saved an `actions/cache` archive and
a later job restored its file contents. A successive run restored those bytes
through a different private engine using the same explicit cohort route. Both
lifecycle reports proved cleanup, and the task Docker engine had no remaining
run containers. This exercises actual execution through the new route, not only
argument generation. The final three-engine extension also passed (1 test,
73.01 seconds): a different namespace using the same cache key reported no
hit and had no restored file. All three engines had distinct immutable IDs
and successful lifecycle cleanup. Independent review passed.

This fixture creates new coordinated namespaces; it does not migrate a live
legacy repository or authorize production enrollment. Shared machine routing,
old-peer exclusion, independent watcher supervision, other cache classes and
sustained physical-footprint convergence remain required.

Strict service all-target Clippy passed after the final three-engine fixture
(7.71 seconds). This routing slice is verified locally; the exact-source gate
is still required before pushing the retention candidate.

### Bounded idle-cohort maintenance and truthful outcomes

Bosn now builds a single-pass maintenance command from the same typed policy
used by the watch command, with no workflow server required. The executor
captures at most 64 KiB with a 30-second transport deadline and parses stdout
even when the command exits nonzero. A valid partial report remains visible;
transport or invalid-output failure means unknown outcome, not zero reclamation
or proof that no deletion occurred. The caller must separately supervise and
retire the verified helper, including remote execution after transport failure.

The typed report binds the exact cohort root and aggregate ceiling, validates
namespace identities and repository ceilings, and bounds stores/pages/receipts
to 64/12/32. Receipt counts and bytes use checked arithmetic and unique IDs.
Complete reports need known totals and complete namespace evidence. Stable
remaining bytes must equal namespace retention totals. Protection is sampled
separately at root and namespace level, so each value is bounded independently;
these samples must not be summed or asserted equal across time. A complete pass
with `budget_met=false` is distinct from an incomplete/unknown pass. These are
logical completed-archive bytes, not allocated filesystem blocks.

Four isolated boundary tests passed, including a protection-boundary RED/GREEN
regression. The first parser rejected a legitimate complete report whose archive
crossed act2's five-minute protection boundary between root and namespace
samples; validation now accepts those independently bounded measurements. An
actual published-binary run also exposed a mistaken full-path namespace
assumption: act2 emits basenames within its root-bound report. That candidate
parser mismatch was corrected without changing the dependency protocol.

The final idle-expiry/replay proof passed against published act2.7 in genuine
private Docker (1 test, 1.20 seconds): no workflow/cache server was alive, an
old imported archive expired, exactly 80 bytes and one deletion were reported,
current archive bytes became zero, and a second pass reported no new reclamation.
The legacy source hashes stayed unchanged and exact helper absence was verified.
Independent review passed; strict service all-target Clippy passed (17.82 seconds).

This is a callable bounded pass, not an enabled periodic supervisor. Shared
machine policy/routing, durable helper creation/recovery, restart scheduling,
old-peer exclusion and production enrollment remain required. The configuration
guard stays. No expiry for other shared cache classes or sustained physical
footprint ceiling is claimed by this proof.

The actual nonzero transport path was then verified against published act2.7:
a missing cohort returned a nonzero partial report with unknown byte totals,
and Bosn retained that typed outcome. The expanded proof passed (0.96 seconds);
strict all-target Clippy passed again (6.48 seconds). This closes command-exit
handling for the tested missing-root case; it is not crash-supervisor coverage.

### Durable maintenance helper identity (2026-10-04 candidate)

The existing helper journal now accepts an explicit `maintenance_v1` role.
Historical measurement records omit the role and keep their previous names and
serialization. Maintenance records use a distinct name, nonce label and ownership
scope. Recovery requires their exact pinned image, immutable container ID, isolated
profile and single writable named-volume mount at `/bosn/cache`; accounting
helpers still require a read-only mount at `/cache`. Unknown roles are rejected.
The shared cache volume is not authorized for deletion by either role.

Isolated Docker verification passed all four registry helper tests and the new
service profile test. These cover historical JSON, durable role recovery after
registry reopen, unknown role rejection, cross-role mismatch, wrong destination,
read-only maintenance mounts and extra mounts. Independent review passed.

This establishes recovery identity only. Maintenance create/start orchestration,
offline verified act installation, finite helper lifetime and periodic/restart
scheduling remain unfinished. Production enrollment remains guarded.

Strict registry/service all-target Clippy passed (20.89 seconds), after moving
the new test module to the end of its file to satisfy the existing lint gate.

### Bounded durable maintenance helper (2026-10-04 candidate)

`maintain_cache_with_helper` now drives one independent pass through an owned
helper. It verifies the existing shared volume, commits a maintenance intent
before create and registers the immutable ID before start. It rechecks the
volume and helper profile before executing. The helper has no network, no
capabilities, a read-only root, 128 MiB memory and one CPU; its writable private
executable tmpfs is limited to 64 MiB. A finite 300-second lifetime with Docker
auto-removal bounds abandoned execution after start. A never-started create
still requires durable journal recovery; no auto-removal claim applies to it.

Act installs offline from the machine archive under its artifact lock, verifying
both archive and extracted binary digests and the version. There is no download
fallback in this helper. An absent or corrupt archive is a visible error rather
than permission to discard cached data. The normal workflow installer and this
helper share one archive-path builder. The callable pass does not bootstrap a
missing archive; supervised scheduling must address that explicitly.

The returned helper result separates maintenance outcome from cleanup outcome.
Cleanup re-verifies ownership before exact-ID removal and requires explicit
absence plus durable journal completion. An acknowledged ID that has already
auto-removed can finish after explicit absence. An unacknowledged create remains
pending when its outcome cannot be observed. Existing online/restart helper
recovery understands the new role. The named shared volume is preserved.

The actual published act2.7 pass succeeded in genuine private Docker (1.09
seconds): complete typed maintenance evidence, zero expired fixture bytes under
a long-age policy, exact helper absence, journal Removed state, and preserved
owned shared cache. Independent review passed. The real lost-create-acknowledgement and temporary inspection-outage proof
also passed (0.78 seconds): a fresh backend recovered the pending intent,
verified and removed its exact container, committed Removed and successfully
measured the preserved shared volume afterward.

This is bounded orchestration, not an enabled periodic/restart scheduler.
Production policy consensus, legacy-peer exclusion and repository enrollment
remain incomplete and guarded. Image/build-cache pressure and expiry for other
shared cache classes remain open; logical archive retention does not prove
physical machine footprint convergence.

Strict service all-target Clippy passed (18.52 seconds), after extracting the
Docker creation profile to satisfy the existing function-length gate.

### Participating machine policy agreement (2026-10-04 candidate)

Cohort workflow execution and independent maintenance now agree on a fixed
schema-1 record at `actcache/.bosn-cohort-policy-v1` in the shared machine volume.
All validated policy fields are compared in a fixed numeric representation.
A nonblocking exclusive lock serializes publication; a unique staging file and
hard-link publication prevent overwriting an existing agreement. Conflicts
return exit 78 and preserve the old record; contention returns 75. No policy
change API or automatic replacement exists. Legacy routes do not claim this
record. The agreement happens before cohort act execution or pruning.

The isolated shell proof passed: initial publication, agreement from a new
process, conflicting aggregate ceiling refused and original bytes unchanged,
with no staging-file leak. Actual private-Docker helper verification also passed
with a compatible policy and refused a conflicting policy while still retiring
both helpers and preserving the shared volume. Review passed.

This coordinates participating callers only. It does not enroll namespaces,
recover a deleted agreement, exclude old/nonparticipating writers, prove record
provenance against arbitrary cache-volume writes, or establish power-loss
persistence. Losing/replacing this record while existing servers run is not a
supported policy transition. Production activation remains guarded. A trusted
scheduler still needs policy discovery/bootstrap, duplicate-pass coordination,
restart/cancellation supervision and durable reporting before rollout.

Strict service all-target Clippy passed (24.65 seconds). The actual cohort
workflow proof then passed through the new agreement path (97.45 seconds):
a later job restored saved bytes, a distinct fresh engine restored them again,
and a third repository namespace using the same cache key stayed isolated.
All three owned engines retired and the private Docker daemon had no remaining
fixture containers. These participating workflow and maintenance tests use the
same machine policy (100 MiB repository, 200 MiB aggregate, 30-day maximum age,
seven-day unused age, 60-second maintenance interval). This is not a production
machine-policy migration or contention/retry throughput proof.

### Callable periodic maintenance supervisor (2026-10-04 candidate)

`supervise_cache_maintenance` runs independently of workflow engines. It starts
with an immediate bounded helper-recovery pass, then an independent maintenance
helper pass. Recovery has a 200-second budget and carries its fair cursor between
ticks. A maintenance attempt has a 600-second outer budget, including pinned-image
availability and the existing bounded helper operations. Started helpers retain
their 300-second finite lifetime. Recovery errors do not erase maintenance
outcomes or permanently suppress all subsequent passes.

Each tick reports recovery and maintenance separately, preserving maintenance
outcome versus cleanup status. A bounded report channel applies backpressure:
the supervisor cannot launch further passes while the consumer is stalled.
After reporting, it waits the configured interval before another pass. A fresh
supervisor has no stale in-memory next-run timestamp and attempts immediately.
Shutdown cancels recovery, execution, reporting or sleep; incomplete helper work
remains in the durable journal for the established reconciliation path.

This is a callable supervisor with an explicitly supplied trusted policy, not
production daemon wiring or an enrollment API. Reports are typed; a bounded latest snapshot is now persisted before delivery
(see the evidence below), rather than an unbounded maintenance history. Machine-wide duplicate-supervisor coordination,
policy discovery/bootstrap, cancellation during individual Docker operations,
old-peer exclusion and production admission still require work. The existing
configuration guard stays in place. The actual idle periodic/restart proof
passed in private Docker (73.75 seconds): two ticks with distinct helper IDs,
complete typed maintenance outcomes, exact helper absence, stop during the
interval and immediate restart pass within its 15-second observation budget.
The shared volume survived and no fixture containers remained. Independent
review passed. This does not test cancellation during create/exec or establish
production daemon wiring.

Strict service all-target Clippy passed for the supervisor (24.83 seconds).

### Participating maintenance exclusion (2026-10-04 candidate)

The actual maintenance command now opens
`actcache/.bosn-maintenance-v1.lock` in the shared machine volume and acquires
an exclusive nonblocking lease on FD 6. It then execs the verified act binary
with unchanged literal arguments. The descriptor spans the command lifetime;
process/container death releases the lease. A competing participating pass
returns exit 75 with `machine cache maintenance busy` before invoking act.
The bounded maintenance adapter preserves that diagnostic as an outcome error,
not as successful reclamation or a complete empty report. The helper still
retires and its durable journal finishes. The scheduler reports the contention
and tries again on a later tick.

The genuine private-Docker proof passed (2.24 seconds): two independent backend
instances shared the same volume; one owned helper held the lease, a competitor
was refused and cleaned up, killing/removing the exact holder released the
lease, and the competitor's next pass completed. The shared volume survived.
The fixture cleans its holder before asserting operation errors; existing helper
identity verification and explicit absence gates are used. Review passed.

This excludes participating maintenance commands, not helper creation itself,
nonparticipating older binaries or workflow cache servers. Act2's cache-server
root/store locks remain a separate boundary. It does not establish production
policy discovery/enrollment, durable maintenance outcome history, or cancellation
during an uncertain Docker operation. Those rollout requirements and broader
cache-class/image/build-cache expiry remain open.

Strict service all-target Clippy passed for maintenance exclusion (12.04 seconds).

### Durable latest maintenance accounting (2026-10-04 candidate)

The supervisor now commits a typed latest snapshot to its local registry before
delivering each tick. The fixed meta key has a 4 KiB read/write ceiling and
replaces the prior snapshot, so report retention cannot itself grow without
bound. Registry and read-only registry readers can retrieve it after restart.
This is the latest committed observation, not a machine-wide cumulative history.

Observed snapshots retain command exit status, partial status, logical archive
budget, nullable remaining/protected bytes and budget outcome. Total reclaimed
archive bytes are computed with checked arithmetic only for complete root
reports whose namespace retention data is all known. Partial or missing totals
remain unknown. Transport/protocol errors use an explicit unknown outcome;
cleanup and orphan-recovery errors remain separate. UTF-8 diagnostics are capped
at 128 characters (at most 512 bytes). A failure replaces old totals rather than
presenting a prior successful snapshot as current evidence.

Helper references must match the registry's existing maintenance-role nonce and
immutable container ID. Claimed cleanup success requires its durable journal
state to be Removed. The trusted actor performs the transaction; no new client
wire operation grants receipt or deletion authority. Snapshot persistence errors
are reported separately without discarding maintenance/cleanup outcomes.
Cancellation during an already-enqueued registry operation can leave its commit
acknowledgement unknown; readers still see only committed snapshots.

The isolated registry restart/conflict test passed (0.22 seconds): premature
cleanup success and a different container ID were refused; reopening preserved
the valid snapshot; a later unknown transport outcome replaced it and survived
read-only reopening. Independent review passed. The actual periodic/restart
proof with committed snapshot lookup passed in private Docker (64.33 seconds):
two idle ticks, stop/restart, persistence acknowledgement before each delivered
report, and a read-only latest snapshot matching the restarted helper ID.
All fixture containers retired and the shared cache volume survived.

This stores bounded local latest evidence. It does not aggregate snapshots from
multiple registries, preserve cumulative reclaimed-byte history, retain all
namespace receipts, expose a new user-facing maintenance-status command, or
prove physical block reclamation. Current helper journal reconciliation may
advance after a snapshot; its earlier cleanup error remains historical evidence.
Production startup policy discovery/enrollment and broader image/build-cache
and shared cache-class expiry remain open.

The registry boundary also rejects a partial root carrying a claimed total
reclamation value. The expanded registry test passed (4.61 seconds), accepting
an unknown reclamation total for that partial report. This closes the typed
snapshot invariant even for another future trusted caller.

Final registry/service all-target Clippy passed (31.42 seconds), including the
partial-reclamation guard and latest snapshot integration.

### Current-main integration and promotion gate (2026-10-04 candidate)

The retention branch was rebased onto main `219ee2b1` (Bosn 0.1.14 preparation,
verified desktop widget and raw task-stream persistence). The configuration
conflict was resolved by preserving both desktop opt-in UI installation and
retention admission checks. The existing desktop test now includes a cache
policy, checks that installation preserves it and verifies that installing the
UI cannot bypass the enrollment guard.

The accumulated candidate is being prepared for a PR through the repository's
clean exact-source local gate. The gate must replay both the Rust and Linux
workflow lanes through released Bosn, validate executed-step/source receipts,
and stamp tree-bound attestations before push. Focused private-Docker proofs
above establish their stated behavior; they do not substitute for this promotion
gate. No push, merge or release is asserted by this entry. A promoted candidate
still needs production policy discovery/bootstrap, repository enrollment and
old-peer exclusion; broader cache-class and host image/build-cache expiry remain
open. The existing production guard will stay until those requirements are met.

The first promotion gate correctly refused the candidate at Rust formatting
(run `d52a8b2a-e1cc-4367-ac5e-eaf4b3a35696`, engine cleanup Removed). It found
an extra blank line left when the helper test module was moved. Python static
and repository guards passed; Rust tests and Linux tests were not claimed.
The changed Rust files were formatted again before replaying the clean gate.

The second gate (`33f51278-d38e-4018-a2e4-24f590db7a37`, cleanup Removed)
passed format, Clippy and kernel boundary checks but failed the 50-submission,
10-key concurrency test: its roughly five-second observation window expired
with a run still Running before an engine ID was recorded. The private Docker
service suite reproduced the failure (457 passed, one failed, 15 ignored),
while that test alone passed in 2.14 seconds. Its burst-specific observation
budget is now bounded at 30 seconds, and it additionally requires every run's
conclusion to be Success; timeout/error completion cannot satisfy the test.
With this change the full private Docker service suite passed: 458 passed,
zero failed, 15 ignored, 35.70 seconds. Production run deadlines are unchanged.
This is evidence of load-dependent test observation, not a proof of production
latency. The exact-source promotion gate still needs a successful replay; its
Linux lane did not run in either failed attempt.

The third gate (`6dbc5fe7-c19e-472e-b7b6-ac1e32f48ac8`, cleanup Removed,
481 seconds) passed format, Clippy, boundary checks and the service suite,
including the corrected burst test. It then failed an embedded Python fixture:
two setup submissions exceeded its 250-millisecond timing assertion
(`crates/bosn-python/src/tests.rs:494`). The failure needs investigation before
the next replay. No successful promotion or Linux-lane coverage is claimed.

Private-Docker investigation found that fixture's timer included spawning its
Python thread and attaching to the interpreter before either submission call.
An instrumented isolated sample measured the two API calls at 10.40 milliseconds
and passed; this does not identify the precise timing of the failed gate sample.
The fixture now measures the API calls after attachment, retaining their
250-millisecond bound and the isolated process's ten-second outer deadline.
Coalescing, raw-log and cancellation assertions remain. This test adjustment
does not alter production behavior or establish end-to-end startup latency.
All seven embedded Python tests passed in private Docker (0.85 seconds), and
independent review passed for the measurement adjustment.
Python all-target Clippy with embedded-test features also passed (57.62 seconds).

The fourth exact-source gate (`5847ae1f-0d10-4e5d-b415-854a9b043cce`,
375 seconds, cleanup Removed) again passed the service suite but failed the
embedded Python submission assertion: the two calls themselves took 410.23 ms.
Excluding interpreter attachment did not resolve the gate failure. Submission
latency and the fixture's actual asynchronous-admission contract need further
investigation; no successful promotion or Linux-lane receipt is claimed.

The current-thread runtime experiment did not resolve the failure (private
fixture measured 254.86 ms) and was reverted. Code inspection shows setup-ensure
awaits a durable SQLite submission audit before acknowledgement, unlike the
other fake submission cases. Eight WAL/FULL commit samples in the genuine Docker
harness took 292.66–605.72 ms on `/tmp` disk storage and 0.014–0.040 ms on
`/dev/shm`. These samples demonstrate that storage latency alone can exceed the
fixture's 250 ms bound; they do not attribute every millisecond of the failed
gate sample. The Linux protocol timing fixture now owns a RAM-backed temporary
directory, retaining the SQLite audit, 250 ms API check, coalescing and subsequent
log/cancellation assertions. Other platforms retain their original temporary
storage. Disk-backed production acknowledgement latency remains unbounded by
this timing proof; ordinary durable registry and service tests are unchanged.
All seven embedded Python tests passed with this fixture (0.33 seconds), and
independent review passed. The exact-source promotion replay is still required.
Embedded-feature Python all-target Clippy passed (44.81 seconds), and Rust
format checks passed for the integrated status reporting and timing fixture.

### Cancellation at the Docker create acknowledgement boundary

A follow-up private-Docker proof cancels the actual maintenance helper future
after Docker creates its container but before the client returns the immutable
ID. The durable intent remains Pending without an acknowledged container ID,
and cancellation releases the backend's active-helper guard. A fresh backend
recovers the exact helper from its frozen labels/profile, commits Removed,
confirms the immutable ID absent, and measures the shared cache successfully.
The first version of this proof passed in 60.21 seconds. Its wrapper deliberately
withholds the acknowledgement for 60 seconds; that total includes runtime
shutdown waiting for the outstanding command. It does not establish prompt
command cancellation or prompt daemon shutdown. The expanded proof separately
bounds maintenance-future cancellation return to five seconds and checks the
unacknowledged journal shape and explicit immutable-ID absence. That expanded
proof passed (one test, 63.29 seconds): maintenance-future cancellation returned
in 72.197 microseconds. The deliberate client command still delayed fixture
shutdown, so prompt daemon shutdown during an outstanding Docker command remains
unverified. Production startup wiring remains open.

Final service all-target Clippy passed (61.42 seconds) after splitting the
proof's cancellation operation from its recovery fixture. Independent review
passed for the proof and that extraction. The verified follow-up is now included
in the next promotion candidate; the exact-source gate has not passed.

### Maintenance evidence in cache accounting (follow-up candidate)

`bosn runners cache` now requests the fixed latest-maintenance row through the
trusted registry actor's read path, without entering a write transaction. Its
typed reply distinguishes no recorded pass, unavailable registry evidence and
a recorded unknown or observed outcome. The CLI prints the last observation's
Unix timestamp, inventory completeness, command exit code, logical archive
budget result and reclaimed archive bytes; unknown totals stay unknown. Helper
recovery and cleanup failures remain separate. Raw helper diagnostics, nonce
and immutable container ID are omitted from this public summary.

This is the latest evidence in the queried daemon's registry, not a current
machine-wide inventory, cross-registry aggregate, active-supervisor indicator,
cumulative reclamation counter or physical disk convergence guarantee. An old
daemon's reply omitting this optional field still decodes. Default production
planning still uses warm legacy caches and rejects configured retention; this
read-only visibility change does not enroll a repository or start maintenance.

Focused private-Docker maintenance tests passed (seven tests, five explicitly
ignored live prerequisites, 4.43 seconds), including unknown diagnostic
redaction and partial/protected-over-budget evidence with unknown reclamation
and failed cleanup. The published typed JSON Schema was regenerated and its
equality test passed. All-target service Clippy passed (31.05 seconds), and
independent review passed for the Rust, JSON Schema and documentation changes.

### Read-only participating policy discovery (follow-up candidate)

The backend can now discover the canonical shared policy through the existing
journaled helper lifecycle. The helper mounts the owned cache volume read-only,
uses a shared nonblocking policy lock and never creates a record or lock. A
missing volume or record returns explicit absence; malformed, oversized,
unsupported, noncanonical or busy records return an error. Discovery does not
authorize defaults, bootstrap policy, enroll repositories or exclude older
daemons. Production routing and supervisor startup remain open.

Two focused tests passed, including absence without filesystem changes and
canonical policy validation. The real private-Docker discovery test passed in
0.42 seconds: it read the agreed 100 MiB repository / 200 MiB aggregate policy,
retired its helper and left no pending helper intent.

Promotion gate c0819217-e6e3-401a-b403-5d29030cb74f failed after 367
seconds; cleanup was Removed. Format, Clippy and boundary checks passed. The
service suite had 459 passes and one failure in the setup admission timing
fixture. That fixture also awaits the SQLite audit commit, so the follow-up
uses Linux memory-backed storage while retaining the 250 ms protocol assertion,
durable events, coalescing, cancellation and restart assertions. This does not
prove a disk-backed production acknowledgement SLO. The 20 focused setup
tests passed in 0.71 seconds. Accounting and helper recovery tests passed
(20 tests, four explicit live prerequisites ignored, 0.34 seconds), format
passed and final all-target service Clippy passed in 7.03 seconds. Independent
review passed. The integrated candidate still requires its exact-source gate.

### Current destination inventory before warm admission (follow-up candidate)

Bosn now reads act2 `cache audit` through a supplied verified engine, using a
30-second deadline and 64 KiB output bound. The initial page has at most 12
entries; act2's count, logical archive byte total and fingerprint describe its
whole-store scan. Continuation entries are not represented as a full exported
catalog. Typed validation rejects identity/schema mismatches, contradictory
page/count/byte evidence, invalid entries and unbounded diagnostics. Missing,
busy, partial, failed-command and unknown inventory cannot satisfy the current
inventory requirement. Nonzero commands retain valid partial evidence.

The published act2.7 private-Docker import/expiry proof passed in 1.66 seconds.
Current inventory first observed the imported archive (one entry, 80 bytes);
after independent idle maintenance it observed zero entries/bytes and a changed
fingerprint while the historical import receipt still described the original
publication. Source hashes remained unchanged and the exact helper was removed.
A focused parser test passed and service all-target Clippy passed (18.09 seconds)
before the live fixture extension. The initial final lint attempt exposed the
fixture's 106-line function; extracting its current-inventory assertion kept
the gate intact. The expanded live proof passed again, and final all-target
Clippy passed in 18.91 seconds. Independent review passed.

This supplies current sampled evidence, not an enrollment transaction, snapshot
lease across routing publication, proof of old-peer exclusion or physical block
accounting. Production admission and maintenance startup remain open.

Promotion gate c602bb62-eaf2-4906-9027-5aa290ad1d9c completed after 378
seconds with all Rust workspace tests and lint steps successful, but its Bosn
0.1.14 driver marked the overall run Incomplete: reusable-workflow execution
identity was not qualified. Cleanup was Removed. This is not a passing gate
receipt; the Linux lane was not run. Bosn 0.1.15 is now available as a released
driver and the next gate will use an isolated state directory with that version.

### Normal daemon startup maintains existing agreed cohorts (follow-up)

Normal `Service::serve` now starts the independent cache maintenance worker and
keeps the registry writer alive until worker cancellation finishes. It polls
for an existing canonical shared policy; no volume/record means no defaults,
bootstrap, migration or deletion. Discovery errors persist an Unknown latest
result without a helper reference and retry after 60 seconds. Once discovered,
the bounded periodic supervisor runs independently of job admission and saves
its latest outcome before the service consumer receives it. The existing FD6
nonblocking cross-daemon lock continues to exclude concurrent maintenance
commands, though duplicate helper creation is still possible.

Each maintenance helper additionally requires the existing canonical policy on
its actual mounted volume, rather than publishing policy itself. A replaced
volume or removed/conflicting record cannot inherit cached in-memory agreement.
Explicit participating cohort workflow execution still publishes agreement;
maintenance never opts legacy repositories into the cohort or deletes source
stores. Production route selection remains Legacy and configured cohort
retention stays refused pending warm enrollment and old-peer coordination.

The genuine private-Docker normal-daemon fixture passed in 1.45 seconds: startup
without submitted jobs discovered the existing policy, recorded a successful
complete budget outcome, stopped normally and left the exact helper journal
Removed. The no-volume/Docker-outage test issued only volume inspection, never
helper creation or maintenance; absence left no snapshot and outage persisted
Unknown. Focused safety tests and all-target Clippy passed (14.12 seconds).
The tightened per-helper existing-policy check and full service suite are being
verified; no shipped startup behavior or prompt uncertain-command shutdown
guarantee is claimed yet. Other shared cache classes and host image/build-cache
expiry remain open.

The tightened policy check passed the real idle-daemon proof again (2.66
seconds). Full-suite runs exposed setup-only fixtures using the real CI backend:
background discovery legitimately added helper intent events to their exact
setup event expectations. Setup and rollover fixtures now explicitly supply
the existing fake CI backend; their setup executors and exact event assertions
are retained. The corrected full suite and final lint are running.

The promotion candidate before this startup follow-up passed its Rust lane
under the released Bosn 0.1.15 driver (375 seconds), with exact-source executed
step evidence. Its Linux lane is running as
41f19684-93f1-4b0d-bb80-de4603894b89. This supersedes the earlier driver
0.1.14 Incomplete result but does not yet establish a complete promotion gate.

After both setup fixture groups used explicit fake CI backends, the full private
Docker service suite passed: 463 passed, zero failed, 18 explicit live/large
prerequisites ignored. Final service all-target Clippy passed in 19.67 seconds.
The normal-startup slice and fixture isolation received independent review.
The preceding promotion candidate now has a complete passing exact-source gate:
Rust 375 seconds, Linux 332 seconds, total 707 seconds, stamped commit
85bc9afcba5760b4c3c418479c240a6620bc5a01 and tree
d27b7f0616d9e4dfccc9118d35d759896af1dd53. The startup follow-up still
requires its own integrated exact-source gate before promotion.

### Stale Legacy planning cannot cross participating route publication

The fixed shared route-record location is now
`/bosn/cache/actcache/.bosn-cohort-routes-v1/<16-hex-namespace>`. This follow-up
defines its admission fence only; it does not publish or parse routing records.
Actual Legacy workflow invocation acquires the shared lifetime FD8 lease, then
checks the namespace's record. Any record, including malformed content or a
dangling symlink, refuses the stale invocation before act starts and requests
replanning. Invalid/unreadable routing directories also refuse. Listing and
explicit cohort invocations retain their existing lease behavior.

The check belongs after lease acquisition: migration can publish while a
previously planned Legacy job is waiting. The real shell/OS-lock proof marks
the attempted acquisition before publication, confirms the job stays blocked,
publishes while holding the exclusive migration lease, releases it, and requires
exit 78 with no binary execution. The original unfenced wrapper is run against
the same publication and reproduces the unsafe admission. Malformed/dangling
records and an invalid routing directory refuse; genuine absence runs normally.
Two focused lease tests passed in private Docker (0.14 seconds), preserving
process-death lease release. Final all-target service Clippy passed (17.89
seconds) and independent review passed. Integration still requires its own
exact-source promotion gate.

Inspection of the actual released act2.4 source confirms it has no cohort
marker/admission protocol (`pkg/artifactcache/cohort.go` does not exist at that
tag; its legacy handler directly opens the selected directory). Consequently
a new marker or this participating Bosn wrapper cannot fence older binaries.
Old-peer exclusion remains an enrollment prerequisite, alongside durable shared
routing publication, current inventory and snapshot coordination. Neither an
empty census nor this guard authorizes production migration. Default planning
still selects warm Legacy; configured retention remains guarded.

The preceding startup candidate passed its rebased exact-source gate: Rust
325 seconds and Linux 330 seconds, total 654 seconds, stamped commit
06deb89d233e with tree13207a267328092bda66ffaa54c55fad16b43ee7.
The outer gate tool used the older pin and warned that `gate.replay` was unknown;
the lanes themselves used the current source-bound checker and passed. Verification with the current repository's gate-tool pin passed:
GATE-003 accepts the attestation for the exact tree. PR #483 now carries the
verified rebased startup head and is mergeable; remote CI is running. No merge
or Bosn release is claimed. This stale-route follow-up requires its own gate.
### Cleanup retries preserve the ledger without duplicate snapshots

A new survey of helper accounting found that every successful re-registration
of an already known immutable ID appended another full snapshot. Uncertain
removal retries can repeat indefinitely. The private-Docker regression test
reproduced 102 journal entries for a single helper after intent, first
registration and 100 identical retries (RED, 16.76 seconds).

Registration now validates identity, state and time first, then treats the
same ID in Created state as a no-op. The original transition timestamp and
full intent remain unchanged. A first registration and terminal removal still
append durable events; Removed nonces remain reserved forever and conflicting
IDs, invalid times and registration after removal still fail. This preserves
the append-only ledger and nonce protection instead of deleting audit history.

The change bounds repeated registration of one unresolved helper to its real
transitions. It does not bound the number of distinct helper nonces or the
whole registry, compact SQLite pages, reclaim historical audit data, or
establish current liveness from a transition timestamp. Periodic policy
discovery without a record still creates fresh read-only helper intents;
that cadence and the overall accounting-history footprint remain open.
GREEN: five registry helper tests passed (2.39 seconds), including reopen and
terminal nonce protection; 20 service accounting/helper recovery tests passed
(0.59 seconds, four explicit live prerequisites ignored). Registry/service
all-target Clippy passed (29.70 seconds) and independent review passed. The
follow-up still requires its exact-source gate before promotion.


The helper idempotence candidate passed its exact-source gate13 before
integration: Rust 494 seconds, Linux 567 seconds, total 1062 seconds, stamped
`1edf29024f8b389627522fa7086663b3fd47293c` and tree
`742947b21eebd930f8b3af174c3147a84b2bc474`. It is now integrated with merged
routing guard main `d24c25d6`; the doc append conflict preserved both evidence
sections. The current status table has been reconciled with merged behavior.
This combined tree requires fresh exact-source proof before publication.


### Verified locally: normal daemon shutdown during stalled Docker discovery

A normal `Service::serve` daemon with no workflow jobs was tested against a
synthetic Docker transport that enters the real shared-volume discovery call
and stalls for 12 seconds. An explicit marker proves the command started before
the client requests daemon shutdown. The assertion includes kernel runtime
teardown, rather than only completion of the service future.

The baseline failed: shutdown took **12.001524079 seconds**. CI control commands
used `capture_async`, whose kernel implementation dispatches bounded capture to
the runtime blocking lane. Dropping the maintenance future left that blocking
command alive, delaying runtime teardown. Evidence:
`retention-maintenance-shutdown-red.log` (isolated Docker).

The correction uses the existing process-session streaming runner with a bounded
event channel drained concurrently. The session owns and kills the local Docker
client when its future is dropped, while retaining the existing deadline and
total output ceiling. The first regression passed with shutdown in
**113.567175 milliseconds**. The final regression passed in **113.687191 ms**,
including a check that the captured client PID no longer exists after runtime
teardown. The complete service suite passed **470 tests** (18 explicit live
prerequisites ignored), and all-target Clippy passed. An initial broad run found
a stack overflow from the larger nested session future; boxing that future fixed
it, with both the discovery regression and the complete suite rerun successfully.

This does not prove that killing a Docker client rolls back a remote create or
stops a remote exec. Durable helper intents, exact ownership checks and recovery
still handle uncertain remote effects. The real lost-create-ack recovery test was rerun against the private Docker
daemon: cancellation returned in **140.777 microseconds**, the exact helper was
recovered and removed, and the shared cache remained measurable. The whole test
completed in **1.08 seconds**, compared with the previous 63.29-second test whose
blocking wrapper slept for 60 seconds. This proves direct helper cancellation
and recovery plus normal daemon discovery shutdown; normal daemon shutdown
during a real uncertain create/exec and every other shutdown phase remain
separate checks. Evidence: `retention-maintenance-shutdown-boxed-checks.log`,
`retention-maintenance-shutdown-final-checks.log`, and
`retention-maintenance-shutdown-real-recovery.log`. Publication still requires
the exact-source local gate.


### Cache report commands share cancellation-safe bounded controls

A follow-up survey found four report-producing cache operations still bypassed
the normal CI control runner: cohort maintenance, current inventory, import and
publication-receipt recovery. Each used kernel blocking capture directly. Thus
the discovery cancellation correction alone did not establish cancellation of
a maintenance report command.

A regression waits for the actual `prune-cohort` Docker client invocation to
enter a 12-second stall, cancels its future, and includes runtime teardown in
the latency assertion. Baseline RED was **11.999834665 seconds**. The correction
uses one common bounded process-session control runner for all four calls,
retaining each original deadline (30 seconds, or 90 seconds for import), its
**64 KiB** total output ceiling, and typed nonzero/partial stdout handling.
Ordinary controls retain their separate 1 MiB ceiling. The runner is extracted
by responsibility from the nearly 1,000-line engine module.

Focused GREEN was **593.295 microseconds**, including runtime teardown and a
check that the exact local client PID was absent. The complete service suite passed **471 tests** (18 explicit live prerequisites
ignored; 87.42 seconds) against the isolated private Docker daemon. The actual
import/current-inventory/historical-receipt proof passed (11.11 seconds), and
maintenance contention/death-release proof passed (9.01 seconds). All-target Clippy passed (47.76 seconds). The slice is independently reviewed;
its final integrated tree still requires an exact-source gate before publication. Local client cancellation
does not establish remote command termination or undo a published import;
durable journals and ownership-verified recovery remain required. Production
enrollment and shared class/image/build-cache expiry remain unfinished.
Evidence: `retention-cache-command-cancellation-red.log` and
`retention-cache-command-cancellation-green.log`.


### Shared class survey and immutable tool seed experiment (2026-10-04)

A bounded read-only survey used this task's live gate engine
`bosn-act-4326b744-5536-4fe8-94ea-fd34b639d670` to inspect the existing machine
cache without creating a reader or changing its data:

- Five act archive variants occupy the shared `tools/` class, including .2, .3,
  .4 and diagnostic builds. Several historical archive paths have no existing
  lock file; this does not prove either an active reader or safe exclusion.
- The shared runner-image tar has apparent length **566,650,368 bytes**. Engine
  retirement removes a private loaded copy, rather than this shared tar.
- Shared `toolcache/` measured **8,088,304 KiB** allocated (about 7.71 GiB), and
  this fresh engine's `act-toolcache` measured **8,088,652 KiB**. These are
  separate, non-atomic samples; their small difference is not a leak verdict.
  The seed script explicitly uses BusyBox `cp -a` on every visible tool family,
  so cache hits still require copying the broad tool set into each fresh
  engine. The actual BusyBox cp has no reflink option.

The classes contain Python, Node, Go, uv, Soldr toolchains/syslibs/bundles,
runner tools and binfmt data. This is evidence that warm reuse alone does not
solve per-engine tool-storage amplification. No old shared artifact was deleted
and no current archive was declared unused. Survey artifact:
`retention-shared-class-age-survey.txt`.

A genuine private-Docker experiment then tested a closed read-only lower volume
with a completed-install marker, a 16 MiB payload and a small settings file.
Two successive fresh privileged containers each mounted an OverlayFS view with
the same immutable lower and a distinct private tmpfs upper/work directory.
Both read the identical payload SHA-256 and marker with **0 KiB** allocated
upper data initially. Mutating settings allocated **4 KiB** privately, preserved
the original lower settings, and did not copy the 16 MiB payload into either
upper. Container wall times were 0.790 and 0.519 seconds. Both containers were
auto-removed; the exact task-owned source volume was removed and cleanup was
confirmed. Artifacts: `retention-toolcache-cow-experiment.py` and its JSON result.

An additional private Docker experiment mounted that overlay at a nested
Docker named volume's data path. Two actual job containers in the first fresh
engine shared their private settings change; a job in the second fresh engine
read the original settings. All three read the same 16 MiB payload digest.
Each engine's upper grew from **0 KiB to 4 KiB**, without copying the payload.
Engine readiness took 16.37 and 16.60 seconds. Exact task-owned engines and
the fixture volume were removed and their cleanup confirmed. Artifacts:
`retention-toolcache-cow-nested-experiment.py` and its JSON result.
This used the Docker image's normal Dind entrypoint, not Bosn's production
engine profile or an act workflow. It verifies nested volume visibility and
within-engine sharing; production initialization, real installers, lifetime
protection and expiry remain unverified. The current
mutable tool cache cannot serve as a live overlay lower: the kernel documents
undefined behavior when underlying trees change while mounted. Immutable
generations are therefore required before applying the primitive to Bosn.
See [the kernel OverlayFS contract](https://docs.kernel.org/filesystems/overlayfs.html#changes-to-underlying-filesystems).

#### In-progress act2 immutable install publisher

The unpublished `feat/immutable-tool-cache` act2 worktree adds
`cache tool-publish --from ... --cache-server-path ... --max-bytes ...
--apply --source-quiescent`. It copies one completed install into a private
stage, validates typed metadata and file hashes, and atomically publishes a
manifest-identified object without replacing an existing destination.
Mutable source inodes are never hard-linked into the object. Identity includes
permissions, ownership and timestamps as well as file data. Source quiescence
is an explicit caller assertion; a completion marker alone cannot prove it.

The focused CLI test failed before implementation and then passed in an
isolated Go Docker container: unchanged installs reuse an object, changed
installs produce a new object, and the old payload remains intact. Additional
focused tests passed for executable modes, relative symlinks, sibling completion
markers, incomplete/over-budget/cancelled sources, corrupt existing objects and
unknown stores. Corrupt objects are preserved and reported as partial failures.
Initial execution found and fixed missing coordination-file initialization.
The complete artifact-cache package test run passed (14.27 seconds), `go vet`
passed for the cache and command packages, and their golangci-lint run reported
zero issues after scanner decomposition and explicit returns.
Review then found external symlinks could violate object closure and eager
directory enumeration could defeat the entry bound. Both were corrected:
links must be relative and resolve inside the object (dangling/cyclic links
are rejected), traversal reads 256 entries per page with a depth limit of 64,
and initial-store recognition reads at most two entries. Absolute, parent and
transitive symlink escape regressions failed before their fixes and passed
afterward; excessive-depth refusal and all existing focused tests also passed.
The corrected package/command lint run reported zero issues. The same reviewer
returned PASS for the corrected slice. Artifact-cache package tests passed
again (14.25 seconds), and `go vet` passed. The act2 candidate is committed at
`1941705`; it has not been pushed or released. Artifacts: `act2-toolcache-link-red.log`,
`act2-toolcache-transitive-link-red.log`, `act2-toolcache-review-fixes.log`.
Artifacts: `act2-toolcache-publish-red.log`,
`act2-toolcache-publish-green.log`, `act2-toolcache-boundaries.log`.

The publisher also has a full-operation fault test: inject a final directory
sync failure after the no-replace rename, observe `published=true` together
with `partial=true`, then verify a subsequent normal publication reuses that
same object and acknowledges durability. This tests truthful uncertain-state
reporting and replay; it does not simulate a power loss or prove crash survival
for every filesystem. Artifact: `act2-toolcache-durability-replay.log`.

This is local work, not released or enrolled by Bosn. Publication gates,
power-loss evidence, reader protection, accounting
and coordinated expiry remain open. The publisher alone does not solve
per-engine tool copying or authorize deletion of existing caches.

#### In-progress immutable generation assembly

The act2 candidate `cbbd365` adds `cache tool-generation --manifest ...
--cache-server-path ... --max-bytes ... --apply`. A bounded, typed schema-1
specification lists install paths and closed object IDs. Assembly rejects empty
or overlapping paths, path escapes, completion-marker collisions, missing or
corrupt objects, and an exceeded logical payload-byte bound. The specification
is limited to 64 KiB and 256 installs; the complete tree remains subject to the
100,000-entry and depth bounds.

Under the same participating writer lock as object publication, it validates
every object, creates new directories and relative symlinks, and hard-links
file data only from closed objects. It preserves object file metadata and
materializes sibling completion markers. A sorted typed manifest identifies
the complete generation. Stage re-audit, file/directory sync, atomic no-replace
rename and final directory sync precede success. Reuse requires a complete
manifest/payload audit; unexpected existing data is preserved and rejected.
The resulting tree must be exposed as a read-only lower. Writable use would
mutate the shared file inodes and is not an approved enrollment path.

The CLI regression failed before implementation (`unknown flag: --manifest`)
and passed afterward. It proves a repeated recipe reuses the generation,
reordered installs yield the same ID, and both old and successor generations
share the original object's file inode rather than copying it. The old
generation does not acquire the successor's additional install. Boundary tests
passed for invalid requests, cancellation, budget failure, missing objects and
unexpected existing payload. Full artifact-cache package tests passed (14.27
seconds), `go vet` passed, golangci-lint reported zero issues, and the existing
reviewer returned PASS. Artifacts: `act2-tool-generation-red.log`,
`act2-tool-generation-green.log`, `act2-tool-generation-boundaries.log`,
`act2-tool-generation-package-checks.log`.

The reported generation bytes bound logical file payload; they are not a
physical allocation total. Generation assembly does not yet supply lifetime
reader protection, safe retirement, policy convergence or Bosn enrollment.
Those remain necessary before production activation. The candidate is local
and unpublished.

A further isolated-Docker experiment built the actual candidate CLI, published
a completed 16 MiB install, assembled its generation, and transferred only the
closed tree into a task-owned lower volume. Two nested job containers in one
fresh Dind engine shared a private settings mutation; a job in a second fresh
engine read the original settings. All read the same payload digest and the
materialized completion marker. Each private upper grew from **0 KiB to 4
KiB**, without copying the payload. The exact two engines and lower volume
were removed with ownership verified. Fixture source/store artifacts remain
in the task's private Go container tmpfs. The first transport attempt failed
because its outer Docker exec did not attach stdin; its lower volume was
removed, stdin attachment was corrected, and the subsequent experiment passed.
This uses standard Dind initialization, not Bosn's production profile or a real
act workflow. Artifacts: `retention-tool-generation-nested-experiment.py` and
its JSON result.

#### In-progress generation reader coordination (2026-10-04)

The living spec through generation assembly is on main via PR #487, merge
`c8ab6697`. Its exact-source local gate passed in 627 seconds (Rust 341s,
Linux 286s), stamped `6d1b8a77fd0cb2036c811548830cc8de41dee65c` for tree
`ff728e2a68bc4b2fc5339989c9dc487b6a8f4f85`; pinned verification and required
remote PR checks passed. No Bosn release was made.

The act2 candidate `01a69fa`, published in
[act2 PR #31](https://github.com/zackees/act2/pull/31), creates a per-generation
`.readers-v1.bolt` coordination file in the private stage before directory sync
and atomic publication. Reuse refuses missing reader coordination. The typed
`AcquireToolGenerationLease` API validates a canonical established store,
generation identity/schema and payload under the catalog writer lock, then
acquires that generation's shared reader lock before releasing the catalog.
Multiple readers coexist; publication of other generations remains possible.
Future retirement must take the catalog lock and that generation's exclusive
lock in the same order, and preserve their inode identity throughout deletion.

Reader admission never initializes a store or recreates missing coordination.
Review exposed an established-store bug that could replace a renamed catalog
lock while another process still held its original inode. The regression
reproduced that failure, then passed after both admission and publication were
made to refuse a missing established-store catalog lock. An empty-store
admission also leaves the directory empty.

The caller must retain the reader descriptor for the entire engine lifetime,
including preparation and uncertain cleanup, and close it only after all
mounts/readers are gone. Admission's context bounds validation; it does not
automatically close a successfully acquired lifetime lease. Holding this lease
only in the Bosn daemon is insufficient: daemon death could release it while
the Docker engine still reads its lower. Actual engine-owned holder integration
is still required. There is no generation retirement command or activation yet.

Focused tests verify published coordination, concurrent readers, exclusive
retirement-lock refusal, successor publication while an old reader is held,
missing coordination refusal, and OS lock release after killing the exact
task-owned helper process. Full artifact-cache package tests passed after the
fix (14.39 seconds), `go vet` passed, golangci-lint reported zero issues, and
Darwin/Windows compilation of unsupported-platform stubs passed. The same
reviewer returned PASS for the corrected slice. Artifacts:
`act2-tool-generation-reader-red.log`, `act2-tool-generation-reader-tests.log`,
`act2-tool-generation-catalog-reader-red.log`,
`act2-tool-generation-reader-catalog-fixes.log`,
`act2-tool-generation-reader-final-checks.log`.

Source-bound checks then passed on the clean committed candidate: all 2,547
exported Git files matched the private Docker build source before and after
execution. Complete cache tests, focused tool command tests, vet, lint and
Darwin/Windows compilation passed in 18 seconds. Source SHA:
`01a69faac05e88cf0e2b5b84b23c2adeb47995a1`; tree:
`f6fe365745366bd8e698997c903511c352947f6e`. This scoped local gate does not
replace full act2 CI on the exact release candidate. Artifacts:
`act2-immutable-source-bound-gate.json` and its execution log. No act2 release
or Bosn activation is claimed.

Admission currently audits the full payload while holding the catalog lock.
Its cost on the surveyed 7.7 GiB cache has not been measured; this is a
correctness baseline, not verified efficient production admission. Engine-owned
lifetime protection, efficient admission, coordinated accounting/retirement,
current-generation selection and physical budget convergence remain open.

#### Next implementation contract: immutable generations and private writes

- Publish typed, content-identified closed generations from completed installs.
  Stage and validate their manifest/data before durable publication; readers
  must never mount a stage or treat an incomplete record as an empty cache.
- Reuse closed generation objects when building a successor, so new generations
  do not copy the complete tool set. Hard links may join immutable generation
  objects; they must not expose mutable source objects or job-written upper
  data as shared lower data. No installed lower object changes in place.
- Pin a generation in the engine's frozen ownership/storage profile. Acquire
  its reader protection before mounting and preserve protection for the actual
  engine lifetime, including preparation. Lost create/start acknowledgements
  need the existing exact-ID journal and cleanup rules. A missing generation
  requires replanning or known warm-copy fallback, rather than silently starting
  with an empty tool cache.
- Mount a private writable overlay at the nested tool-cache volume's data path.
  Verify that nested job containers see it, that normal installers can create
  new tools, and that edits/deletes/chmod/copy-up remain private. Save only
  completed new installs back through coordinated publication.
- Account shared closed data once in the toolcache class, include stages and
  unknown objects, and report private upper storage with the owned engine.
  Independent shared/private samples must not imply a single atomic total.
- Expire old generations, unreferenced objects and stages only after verified
  writer/admission and reader exclusion. Deletion must preserve a current warm
  seed and in-use generations, expose protected overflow, and converge under
  sustained publication; stage cleanup and old generations cannot be left
  outside the byte/age policy. This also requires resolving distinct-nonce
  ledger growth and host owned-image/scoped builder retention.

Activation is not implemented. Required acceptance remains actual later-job and
fresh-engine warmth, private mutation isolation, generation durability/replay,
concurrent publisher/reader/GC safety, exact retirement and sustained logical
and physical ceilings. The original broad shared-cache/expiry goal remains open.


### Engine-owned reader descriptor: actual init-chain experiment (2026-10-04)

The reader-contract follow-up is merged in [Bosn PR #488](https://github.com/zackees/bosn/pull/488)
at `5fb7adbd74ccf964ba966156636f72773df4956a`. Its exact-source local gate
passed in 605 seconds (Rust 304 seconds; Linux Docker tests 301 seconds),
and required remote checks passed. [Act2 PR #31](https://github.com/zackees/act2/pull/31)
is merged at `adefc9f75bfbb789ac54aa18bd1546bcf6cc04d1`; the exact-merge
[full CI run](https://github.com/zackees/act2/actions/runs/37215686314) passed. No release or Bosn pin update is claimed for that merge.

A new private-Docker experiment tests the engine-owned descriptor approach
through the actual current `ENGINE_INIT` body, then `docker-init`, then
`dockerd`. It uses the already cached pinned Docker image, a read-only root,
private cgroup namespace, Unix-only daemon socket, bounded private tmpfs storage,
and a read-only volume containing a generation produced by the current act2 CLI.
A private launching client is killed after readiness; the engine remains alive.

- Negative control: without a held descriptor, an exclusive writer obtains the
  generation reader lock while the engine runs, before and after client death.
- With `exec 5</lower/.readers-v1.bolt` and `flock -s -n 5` before the unchanged
  init body, exclusive writers are refused while the engine runs, including
  after client death. `/proc/1/fd/5` still names the generation reader file.
- After stopping that exact engine and verifying it is no longer running,
  an exclusive writer obtains the lock. Both experiment engines and their
  labelled volume are removed after exact ownership checks.
- Engine readiness was 0.529 seconds for the control and 0.409 seconds for the
  protected engine. These tiny-fixture timings do not measure admission costs
  for the 7.7 GiB production tool cache.

Evidence artifacts in the task's private git directory:
`retention-engine-owned-reader-experiment.py`, `.log`, and `.json`. Run key:
`157021de141146fb9399ff225d5141dc`; generation:
`5b50ccf627083156cc21a7c740d94d3a0e8b177991c0c1c78dcf878721e3a07e`;
unchanged base-init SHA-256:
`e269505630ad33e94fcf314157a7f6376c32f73a538275f0b4b76616b1ac54b3`.
The small source/object fixture remains inside the private Go container's
`/tmp` filesystem; no shared cache or host daemon was pruned or stopped.

**Scope:** this proves descriptor inheritance through the real init chain and
release on engine stop. It does not exercise an actual Bosn daemon crash,
catalog-coordinated admission, production generation selection, or retirement.
The generation and its reader file were transferred into a private experiment
volume; this is not proof of mutex identity between that volume and the original
publisher store. Production must retain the same published reader inode,
acquire catalog protection before generation protection, and validate the closed
generation before any mount. A raw shell lock alone is insufficient admission.
The shell prototype would require FD 5 to be reserved in the frozen engine
command/profile and verified during recovery. The native handoff below instead
preserves its dynamically allocated original descriptor; production must freeze
that policy and observe the actual descriptor and inode during recovery. The existing daemon-only reader API is still insufficient for a
surviving engine. Engine admission, mounting, coordinated expiry, and efficient
large-cache admission remain implementation work.


### Native admission and exec handoff (implemented candidate, 2026-10-04)

[Act2 PR #32](https://github.com/zackees/act2/pull/32), candidate
`7fd5beef176674b7f71f5bcfc062c5ad1b118897`, adds:

```sh
act cache tool-exec --cache-server-path STORE --generation ID \
  --max-bytes POSITIVE --apply -- COMMAND ARGS...
```

`ExecWithToolGeneration` is Linux-only and intended for a dedicated engine-init
process. It validates the closed generation under the catalog writer lock,
then opens the original bounded regular reader file with bbolt's shared lock
before releasing catalog protection. It clears close-on-exec on that exact
original descriptor and replaces its process, reporting the dynamic descriptor
number in `BOSN_TOOL_GENERATION_LEASE_FD`. It does not force a fixed FD number or
close/reopen the mutex. Failed exec restores descriptor flags and releases the
reader. The command must keep the descriptor open throughout engine lifetime;
the generation is still only a read-only overlay lower, never a writable mount.

The focused regression was RED because the API was missing, then GREEN: the
same original-store bbolt writer remains blocked across exec and succeeds after
the replaced process exits. Malformed argv, canceled admission and failed exec
also release the reader. Final exact-source scoped gate passed in 18.73 seconds:
2,552 exported files matched private Docker source bytes/modes before and after
full artifactcache tests (14.542 seconds), focused tool commands, vet/lint with
zero findings, and Darwin/Windows unsupported-stub compilation. Cumulative
independent review passed. These checks do not replace full act2 release CI.

A second actual-init experiment runs the native CLI as engine PID 1 before it
execs the unchanged init chain. Publication and admission use **the same labelled
volume**, with read-only admission attachment; no published mutex is reconstructed
for admission. Control allows writers while alive. Native admission blocks writers
before and after killing the launching client, then releases the lock after the
exact engine stops. PID 1 retains original reader descriptor 7 in this run.
Readiness was 0.428 seconds for the control and 0.381 seconds for the native
protected engine, with an eight-byte fixture. Both owned engines and the volume
were removed after label checks. Run `80d4509034074495abc87218ad2f56c5`;
generation `241e5b4bbdcc223b5f5cb38e55bb533eed89b592034a8ce78390ca6faa16f5c4`.
Artifacts: `retention-engine-native-reader-experiment.py`, `.json`, `.log`;
`act2-tool-generation-exec-red.log`, `-green.log`, and
`act2-generation-exec-source-bound-gate.json`/`.log`, in the task's private git
directory. This verifies init handoff, not an actual Bosn daemon-crash path.

Two prior archive-transfer fixtures were refused by canonical payload validation
and their exact engines/volumes cleaned up. Ordinary tar transfer cannot be assumed
to preserve a generation manifest's metadata. Validation was not weakened; the
passing experiment publishes and admits on the same volume. A later diagnostic
accidentally auto-created an empty replacement private volume after cleanup; that
exact replacement was inspected and removed. No shared cache was pruned.

**Still open:** PR #32 is not released or activated in Bosn. The production frozen
profile must reference a selected generation and the native handoff policy;
recovery must verify its mount and live original-inode holder. Coordinated current
selection, actual overlay mounting, completed-install publication with writer
exclusion, efficient admission of the 7.7 GiB cache, generation/object expiry,
physical accounting and image/container/scoped build-cache expiry remain required.


### Durable warm selection and concurrent completed-install updates (candidate, 2026-10-04)

The native handoff spec is merged in [Bosn PR #490](https://github.com/zackees/bosn/pull/490),
merge `eafc7975840567fe0407533c64955a7b80e97185`, after the exact-source
local gate passed in 525 seconds and required remote checks passed. Native
handoff code is merged in [act2 PR #32](https://github.com/zackees/act2/pull/32),
merge `09d1ad66486e04514d651362f4c0a155bc9ac7f5`; no release or Bosn pin
update for that merge is claimed.

[Act2 PR #33](https://github.com/zackees/act2/pull/33), candidate
`3dd0deddbb4b99f589638a7c575453f169af0759`, implements selection separately
from immutable generation publication:

```sh
# Deliberate first selection during store setup or explicit recovery only:
act cache tool-update --manifest COMPLETED_INSTALLS.json \
  --cache-server-path STORE --max-bytes POSITIVE --apply --initialize

# Normal completed-install update; never a missing-cache bootstrap fallback:
act cache tool-update --manifest ONLY_CHANGED_INSTALLS.json \
  --cache-server-path STORE --max-bytes POSITIVE --apply

# Advisory validated selection; does not hold a lifetime reader:
act cache tool-current --cache-server-path STORE --max-bytes POSITIVE
```

`UpdateToolGeneration` takes the original catalog writer lock, validates the
latest selected manifest, full payload and reader coordination, and merges only
supplied install paths into that latest recipe. An exact path replaces its prior
object reference; unrelated completed installs stay warm. The complete merged
recipe is validated and assembled while holding the same catalog lock, then a
synced private JSON stage replaces `.tool-current-v1.json` atomically and the
store directory is synced. Its typed report separates generation publication
from selection. Private selection-stage cleanup failures retain a reported
`pending_selection` path. The payload-byte ceiling is logical, not unique inode
allocation or a machine-wide disk quota.

Initialization is an explicitly named API (`InitializeToolGeneration`) and CLI
flag, required only when selection is unset. It refuses an existing pointer.
Ordinary updates refuse missing or malformed selection rather than constructing
a smaller cold successor. A new engine must never automatically call initialization
because an ordinary selection lookup failed. Deliberate recovery must first
establish which warm state is authoritative; the API does not infer that proof.

Verified tests:

- Concurrent updates from two stale engines retain three completed installs;
  a living reader on the first generation does not block successor publication,
  and the first generation remains unchanged. Repeating an unchanged update
  verifies/reuses the selected generation.
- CLI regression was RED with unknown `--manifest`, then GREEN: separate update
  recipes retain earlier warm payloads and `tool-current` reports the successor.
- A further actual RED showed a lost pointer silently bootstrapping and discarding
  prior installs. The explicit-initialization fix is GREEN: ordinary update refuses
  and leaves the pointer absent and old generation data intact.
- Byte-budget refusal preserves warm selection; symlink, oversize, unknown schema
  or field, missing generation and missing reader coordination are preserved and
  refused. No fallback empty selection is emitted by the current command.
- Full API injection after selection rename reports `selected=true, partial=true`
  on lost final directory-sync acknowledgement. An ordinary update retry verifies
  and reuses the existing publication, then converges. A visible first selection
  must be retried as an ordinary update, since initialization refuses existing data.

Scoped exact-source gate passed in 17.40 seconds: 2,558 exported source files
matched private Docker bytes/modes before and after full artifactcache tests,
focused tool commands including selection, vet/lint with zero findings, and
Darwin/Windows unsupported-stub compilation. Candidate tree:
`7c6308cd9a1c259e867508e6100a32ba4823f110`; cumulative independent review
passed with one reviewer. Artifacts in the task's private git directory:
`act2-tool-selection-red.log`, `-green.log`, `-command-red.log`,
`-lost-pointer-red.log`, `-fixes.log`, `-final-checks.log`, and
`act2-generation-selection-source-bound-gate.json`/`.log`.

**Not yet proven:** PR #33 is unreleased and not activated in Bosn. Current lookup
is advisory: selection and subsequent explicit-ID engine admission can race with
future retirement, so the production planner must coordinate the intent/reader
handoff or refuse and replan; it cannot mount from this report alone. Full payload
validation remains an unbenchmarked admission/update cost for the 7.7 GiB cache.
Generation/object/stage expiry, machine-wide physical accounting, production frozen
profile and recovery, actual job overlay mounts and quiescent completed-install
publication, old image/container expiry and scoped builder pressure remain open.


### Bounded tool-store inode accounting (act2 PR #34)

Implementation: https://github.com/zackees/act2/pull/34, candidate
`84511a0b61f6461af0487f65f30fc562846ee5ec` (unreleased). Linux API
`AuditToolStoreUsage(ctx, root, maxEntries)` and command
`act cache tool-usage --cache-server-path STORE --max-entries N` inventory a
recognized existing store while holding its original catalog writer lock. Missing
coordination is refused, never initialized as part of inventory. Participating
publication and selection cannot mutate the store during this observation.

The bounded metadata walk includes root directories, objects, generations,
private stages, manifests, coordination files and unknown entries. It follows no
symlink targets, refuses a different filesystem device, pages directory reads,
limits entries to the caller's positive bound (at most one million), limits depth
to 72 for store namespace prefixes, and has a 30-second context deadline. Payload
publication retains its existing depth-64 limit. No payload contents are read or
hashed by accounting.

Typed results distinguish four totals:

| Field | Meaning |
| --- | --- |
| `allocated_bytes` | Inode blocks times 512, once per device/inode; includes directory/control allocation |
| `apparent_bytes` | Regular-file and symlink-text size, once per device/inode |
| `unique_file_bytes` | Regular-file size, once per device/inode |
| `referenced_file_bytes` | Regular-file size at every path, including hardlink references |

All byte totals are null on an incomplete scan, cancellation, invalid bounds,
missing coordination, crossing, or arithmetic failure. Observed entry/inode counts
may describe a subset; they are not complete totals. Negative sizes and overflow
are refused. The report includes observation time, device, partial status and a
bounded diagnostic. CLI emits the typed partial report and exits with an error.

Verified evidence:

- A one-MiB immutable object referenced through two generations has four payload
  paths sharing one inode; referenced minus unique regular-file bytes is three MiB.
- Independent GNU `du` comparisons agree with both allocated and apparent totals,
  including manifest/control allocation and excluding a five-MiB external symlink
  target. The first apparent-size comparison failed because directory `st_size`
  was included; the implementation was corrected before publication.
- A sparse sixteen-MiB unknown file contributes its full apparent size and fewer
  allocated bytes. Its inclusion establishes footprint, not deletion eligibility.
- Entry exhaustion, cancellation, invalid bounds and missing catalog coordination
  produce null totals. Command tests cover successful and partial JSON reports.
- Exact-source gate compared all 2,564 exported files, bytes and modes before and
  after complete artifactcache tests, focused tool commands, vet, zero lint findings,
  and Darwin/Windows compilation. Gate passed in 21.03 seconds on tree
  `ade0e002ad94645cdba030b74f4dadae4265e3d0`; independent read-only review passed.
  Private evidence: `act2-tool-store-usage-source-bound-gate.json`/`.log`,
  `act2-tool-store-usage-apparent-red.log`, and `act2-tool-store-usage-checks.log`.

**Limits and next work:** This measures one tool store. It does not deduplicate
inodes across separately scanned stores/classes, measure filesystem journal or
Docker backing-store allocation, or impose a machine-wide quota. Nonparticipating
writers can still invalidate a coordinated observation, so production cutover must
exclude old writers. PR #34 is not released or activated in Bosn. Coordinated
retirement must protect the selected generation and live readers, account before
and after deletion, and preserve unknown/corrupt state. Generation/object/stage
expiry, whole-machine accounting, old engine/image expiry and scoped builder
pressure remain required. No shared object was deleted for this accounting slice.


### Coordinated generation retirement (act2 PR #35)

Implementation: https://github.com/zackees/act2/pull/35, candidate
`f8709458548f5510264babb96f58baee6a0b9c15` (unreleased). Linux API
`RetireToolGeneration(ctx, root, id, maxBytes)` removes one explicitly eligible
unselected generation. The caller must establish age/pressure eligibility; this
API does not choose candidates or implement an automated retention policy.

The original catalog writer stays held while validating the current selection
and its complete payload/reader coordination, validating the candidate manifest
and complete tree, collecting bounded store metadata, and acquiring the original
candidate reader mutex exclusively. Selection, admission and participating
publication cannot race this decision. Selected generations and live reader
holders are refused. Missing/corrupt selection or coordination is preserved and
refused. Candidate control layout must contain exactly the canonical tree,
manifest and reader mutex; unknown entries are preserved. Immutable shared
objects are never removed by this primitive.

Mount safety requires more than device identity. A same-filesystem bind mount
can preserve expected metadata/hashes while exposing a shared object to recursive
deletion. Independent review found this hazard in the initial implementation.
The correction compares Linux statx mount IDs for the store root, generation
namespace, candidate and every bounded descendant, without following symlinks.
Unavailable mount identity or any boundary causes refusal before deletion.
Production must exclude nonparticipating mount changes and writers during this
operation; this is not a defense against a privileged actor changing mounts after
validation. Both original coordination locks remain held through removal and
parent-directory sync. An error can follow partial deletion or lost durability
acknowledgement: reclaimed bytes must come from a fresh accounting observation,
never the logical generation size or a boolean return value.

Verified evidence:

- Focused regression was RED because retirement did not exist, then GREEN:
  selected generation stays intact, live reader prevents retirement, closing that
  reader permits old-generation removal, current selection remains valid, and
  the shared immutable object remains present.
- Lost selection, unknown control entries and missing reader coordination refuse
  deletion. Mount-ID refusal also has a deterministic injected-boundary test.
- A real same-device bind mount in an owned isolated container was refused;
  source/target device IDs agreed and the mounted shared object remained fully
  valid afterward. The container had no host binds or network and was removed.
- Exact-source gate passed in 21.68 seconds against all 2,569 exported files before
  and after complete artifactcache tests, focused command tests, vet/lint with zero
  findings, and Darwin/Windows compilation. Tree:
  `c3539145d22d2bed423b87622dfc9faa7611e1f4`. The first final gate caught an
  unchecked test unmount error; teardown now checks it. Cumulative independent
  review changed from CHANGES REQUESTED to PASS after mount protection.
- Private artifacts: `act2-generation-retirement-red.log`,
  `act2-generation-retirement-actual-bind-mount.log`, and
  `act2-generation-retirement-source-bound-gate.json`/`.log`.

**Remaining required work:** Automated age/pressure selection, object expiry,
owned-stage expiry, measured physical before/after reclamation and convergence,
protected-overflow reporting and bounded retry after partial retirement. This
API has no production Bosn caller yet. Selected-generation intent/reader handoff,
engine lifetime/recovery integration, large-store admission cost, cross-class
machine accounting, old image/container expiry and scoped builder pressure all
remain open. PR #35 needs remote CI/merge and an exact-default-branch full-CI
release before Bosn can pin and activate it.


#### Physical reclamation evidence for the pressure loop

A private Docker regression published a one-MiB object, selected a generation
with one reference, then selected a successor with two references to that same
inode. Retiring the unselected predecessor reduced observed allocated store
bytes from 1,118,208 to 1,097,728: only 20,480 bytes were reclaimed, while the
1,048,576-byte shared payload stayed allocated and valid. The selected successor
and object retain three payload paths sharing one inode. The test checks positive
metadata reclamation smaller than the payload, retained unique payload bytes,
and reference-minus-unique bytes of two MiB; it does not require these exact
filesystem-dependent allocation totals. Evidence:
`act2-generation-reclamation-evidence.log`.

The retention pressure loop must therefore use a fresh complete physical scan
after each mutation, including partial deletion, and continue toward its target
only while eligible candidates remain. Logical generation size is not a predicted
reclamation credit. Reaching a protected/unknown-only remainder above the cap must
report protected overflow rather than deleting selected/live/unknown state or
claiming convergence. This loop and object retirement are not implemented yet.


#### Generation policy implementation in progress

The local act2 `feat/tool-retention-policy` branch now has typed
`ToolRetentionPolicy` and `RetainToolStore` APIs. The positive allocated-byte cap,
explicit publication-age cutoff, maximum scanned entries (one million), maximum
namespace candidates (ten thousand), and payload-validation ceiling are validated
before mutation. The original catalog writer remains held throughout selection
verification, complete initial accounting, bounded candidate enumeration and the
sweep. Candidates are sorted by publication-directory modification time with an
ID tie-break; the current selected generation is always preserved. Older eligible
generations expire even below the cap; under pressure, younger unselected
candidates can also retire. Reader-lock timeout protects a candidate. Other
retirement failures mark the report partial. Every attempted mutation is followed
by a fresh physical observation, including failures that may have partially
removed data. Unknown namespace entries and all immutable objects stay intact.

Focused regression was RED with missing APIs, then GREEN for expiry, live-reader
protection followed by retry, unknown-state preservation and protected overflow.
Further tests pass for young-generation preservation below the cap, pressure
retirement and pre-mutation entry/candidate-bound refusal. Full artifactcache
tests passed; initial lint found excessive function complexity and a deprecated
bbolt error alias. The sweep is now a separate helper and uses the dependency's
current errors package; focused boundary tests and lint pass. Independent review
and final exact-source gate remain pending. No policy change is released or
activated in Bosn. Object/stage expiry, last-use evidence beyond publication age,
CLI integration, measured convergence across classes and production scheduling
remain required.


### Maintenance shutdown observation correction (2026-10-04)

The object/stage spec's required Bosn gate failed in the existing stalled-client
shutdown test: runtime teardown completed in 16.48 ms, but an immediate PID
probe still observed the owned process. The process-session drop contract sends
an asynchronous owner-drop notification; it does not synchronously prove reaping.
The original isolated fixture also failed once in 32 concurrent attempts.

A bounded reaping observation alone remained RED (2/64 attempts). Capturing the
probe output in another 64-attempt series identified four failures caused by
`ValueError: invalid literal for int() with base 10: ''`: cancellation observed
the marker after creation but before its PID contents were written. The fixture
now writes a private sibling and atomically renames it to publish a complete PID.
The probe waits for actual PID absence, and the final elapsed-time assertion
includes runtime teardown and observation within the original five-second
cancellation budget. A live 12-second negative-control process was rejected.

The corrected isolated focused test and all 64 attempts with four concurrent
workers pass. Independent review passes. These are diagnostic checks in the
private Rust harness, not the required gate on this exact source commit; that
gate remains pending. No production cancellation code changed. This evidence
does not prove that stopping a Docker client stops a remote command, nor does it
close the daemon-crash, uncertain creation/start, or engine-lifetime recovery
requirements.


### Verified object retirement and pressure-sweep integration

Act2 PR #38 adds `RetireToolObject(ctx, root, id, maxBytes, maxGenerations)`.
The original catalog writer spans complete current-selection validation, bounded
retained-generation manifest inventory, verified object payload/layout and mount
identity checks, removal and root-directory sync. Every retained generation
reference protects its object, including idle old generations and live readers.
Unknown namespace entries, missing/corrupt manifests, and exhausted reference
bounds refuse deletion. Candidate objects must have exactly the closed tree and
manifest control layout. Unknown state and unverified payloads stay intact.

Missing API regression was RED then GREEN: an object stays while selected,
while an old reader lives, and while its idle old generation remains; after that
last generation is retired, object deletion reclaims at least its one-MiB payload
allocation and the selected replacement remains valid. Strengthened tests use an
otherwise unreferenced orphan to prove unknown/missing/bounded reference refusal.
Complete package tests, vet/lint and cumulative review passed. Exact-source gate
passed in 20.65 seconds on `f63c19243cf128b35923e2d626bad303922b9344`, tree
`958851bf886d9e565ec5b4fad70490c6365c7d3e`, covering 2,578 exported files and
Darwin/Windows compilation. Evidence: `act2-tool-object-retirement-source-bound-gate`
JSON/log and `act2-tool-object-retirement-reference-checks.log`.

The following local `feat/tool-object-retention-policy` slice integrates object
retirement into the age/pressure sweep. A real failing regression showed that
retiring generation metadata alone left an unreferenced one-MiB object allocated.
It now passes: bounded object candidates are inventoried before mutation,
generation retirement runs first, then verified unreferenced objects expire by
age or pressure, oldest first. Each attempted mutation gets fresh accounting.
Typed reports distinguish retired/protected objects and generations. Referenced
objects are protected without claiming a failure; unknown/corrupt reference state
is partial and preserved. The catalog writer remains held throughout both sweeps.
Full package tests, vet/lint and review pass; exact-source gate on
`e9281ab` is pending. Production activation remains absent.

Stage expiry, automatic production scheduling, CLI entry point, last-use evidence,
7.7-GiB scan efficiency, whole-machine cross-class allocation and sustained caps
remain open. These slices have not been released or pinned into Bosn. Old
containers/images and scoped builder-cache pressure remain separate required
parts of the original objective.


#### Crash-leftover stage ownership gap

Source inspection confirms object and generation publishers create private stages
with `MkdirTemp`, report their paths and attempt deferred cleanup. They do not
persist ownership evidence for a later crash-recovery sweep. A stage-looking
prefix is therefore insufficient proof for deleting a discovered old directory.
The planned fix records typed relative path, creation time and original
filesystem/inode identity in synced ownership files while the original catalog
writer is held. The catalog descriptor remains a read-only mutex. Publication and cleanup must retire this record after successful
rename/removal. Crash windows before registration leave unknown preserved state;
a record after rename must never authorize deleting a replacement at its old path.

Focused new regressions are RED with missing stage-ownership and expiry APIs:
original catalog exclusion blocks expiry during a living publisher, registered
old stages expire, same-prefix unregistered directories remain, and replacement
inode identities refuse deletion. Stage cleanup must also apply bounded metadata
and mount-ID checks and report physical accounting after mutation. No stage was
deleted from a production/shared cache during this investigation. Evidence:
`act2-tool-stage-retention-red.log`. This work remains unimplemented.


### Durable stage ownership and bounded expiry (local implementation)

Candidate `32ce983` wires registered stage creation into both object and generation
publishers. Ownership records live in `.tool-publication-ownership-v1`, coordinated
by the original catalog writer. An attempted bbolt transaction failed in the
focused tests because coordination descriptors intentionally stay read-only; that
mutex contract was preserved and the design moved to separately synced files.
Each exclusive record leaf is the SHA-256 of a typed relative stage path, with
canonical schema-1 JSON (at most 1,024 bytes), original device/inode/mount identity
and UTC creation time. Stage parent, record file and ownership directory are
synced before payload fill. Records are bounded at ten thousand; creation refuses
at capacity before creating another stage. Unknown or malformed ledger state
refuses recovery rather than inferring ownership from names.

`RetireToolStages(ctx, root, expireBefore, maxEntries)` holds catalog exclusion,
requires an explicit age cutoff and bounded complete store inventory, and deletes
only registered expired stages with matching original inode and mount identity.
Descendant mount-ID validation rejects bind mounts; unregistered stage-looking
directories and replacement identities stay intact. Cleanup reports physical
allocation after each attempt. Publication/removal sync precedes record deletion.
A stale record whose original path is absent is cleared after parent sync without
touching any published destination. Failed registration retains `pending_stage`
for the private footprint. Deferred publisher cleanup uses bounded time derived
from the caller context with cancellation detached for recovery.

Verified regressions: original catalog prevents expiry while a publisher lives;
registered old stage expires; unknown prefix is preserved; replacement directory
identity refuses; a lost post-rename sync acknowledgement leaves an ownership
record that recovery clears while the published object remains fully valid.
Independent review caught creation of record 10,001 at capacity. Fixed regression
uses ten thousand actual records, proves refusal before stage creation, then
confirms an existing expired stage remains recoverable. Full package tests,
vet/lint and cumulative review pass. Final exact-source gate is running.
Evidence: `act2-tool-stage-capacity-check.log`,
`act2-tool-stage-publication-checks2.log`, `act2-tool-stage-split-lint.log` and
`act2-tool-stage-source-bound-gate` JSON/log.

Not released or activated in Bosn. Stage expiry is not yet part of the combined
retention scheduler or CLI. Earlier unregistered leftovers are deliberately
preserved until separate ownership evidence exists. Root-level selection JSON
stages are still a separate lifecycle requirement. Large-store efficiency,
production overlay/reader admission, whole-machine accounting and old engine,
image and scoped builder-cache expiry remain required.


### Combined sweep and native retention command

Act2 PR #41 runs owned stage expiry before generation/object retirement under the
same original catalog writer. Selection, complete initial accounting and bounded
candidate inventories validate before mutations. A partial stage result stops
later deletion; fresh stage allocation becomes the next sweep's observation.
Regression was RED with a registered one-MiB stage remaining, then GREEN for
stage removal, physical reclamation and selected-generation preservation. Full
package tests, vet/lint, review and exact-source gate passed in 22.40 seconds on
`edd6acc1e331c53883424229493153caa1e8628b`, tree
`4d980f78cf235b7d826bd2926dd1c4faf39e4124` (2,586 exported files).

Local CLI candidate `e6eaabc` exposes the sweep:

```sh
act cache tool-retain --cache-server-path /absolute/tool-store \
  --max-allocated-bytes 8589934592 --max-payload-bytes 17179869184 \
  --expire-before 2026-10-01T00:00:00Z \
  --max-entries 1000000 --max-candidates 10000 --apply
```

These are example policy values, not default production settings. All bounds and
an explicit RFC3339 cutoff are required. Missing `--apply` refuses before mutation;
`tool-usage` remains the read-only inventory command. The retention command emits
a typed JSON report before returning an error for partial results or protected
overflow. A complete pass that remains over cap therefore does not masquerade as
successful quota convergence. The cap is scoped tool-store inode allocation,
not machine-wide or Docker backing allocation.

Command regression was RED with the command/flags absent, then GREEN: omitted
`--apply` preserves an old generation, explicit mutation retires it, cap-one emits
complete protected-overflow JSON and nonzero status, and the selected successor
remains valid. Command vet/lint and cumulative review pass; final exact-source
CLI gate is running. Evidence: `act2-tool-stage-policy-source-bound-gate` JSON/log,
`act2-tool-retention-cli-red.log`, `-green.log` and CLI source-bound gate JSON/log.

The dependent PR chain has been reconstructed on current act2 master to resolve
conflicts from earlier squash merges; rebuilt trees exactly match reviewed trees.
PR #37's updated exact-source gate passed on `6684ba9`. Rebuilt later commits
still require their own source verification before branch updates. All changes
remain unreleased and unused by production Bosn. Automatic scheduling, frozen
engine profile and reader handoff, real workflow overlay/publication, selection
JSON stage lifecycle, large-store efficiency, whole-machine accounting and old
engine/image/scoped-builder expiry remain required.


### Retention and selection recovery checkpoint (2026-10-04)

The reconstructed act2 branch chain retains the previously reviewed source trees.
Each pushed candidate passed a fresh exact-source scoped gate: stage ownership
`dd8e94b` (2,585 exported files), combined stage policy `57fffbd` (2,586 files),
and retention CLI `9fa189c` (2,588 files). Coverage includes full artifactcache
tests, focused tool-command tests, vet, lint, and Darwin/Windows compilation.
These gates do not replace full CI on the exact default-branch release commit.
PRs #40, #41 and #42 are updated; their parent object-retirement PR #38 is waiting
for a rerun of Linux CI after an unrelated hosted-runner test hit GitHub's API
rate limit. Downstream conflicts must be resolved against each actual merged
parent before their remote gates and merge. No newer act2 release or Bosn pin
is claimed.

The remaining selection temporary-file ownership gap has a local implementation
in `8143cf3`. Selection JSON is now written inside the existing registered
`.tool-stage-` private directory while holding the original catalog writer.
The file and its directory are synced before the pointer rename. Successful
selection and stage cleanup leave no ownership record. A failed post-rename
root sync reports `Selected=true`, `Partial=true`, and the owned directory in
`PendingSelection`; cleanup deliberately preserves that uncertainty for recovery.
The existing stage expiry verifies the original inode and mount, then removes
only the private directory and its record. It preserves the selected pointer and
published generation; unknown historical `.tool-current-stage-` files remain
unregistered and cannot be deleted from a name or age inference.

The focused regression was RED because the prior implementation reported an
empty pending path after injected sync failure. It is now GREEN: recovery expires
the owned stage, pointer bytes remain identical, full selected-generation
validation succeeds, and a successful retry leaves no ownership rows. Full
artifactcache tests, focused command tests, vet/lint and cumulative independent
review pass. Final exact-source scoped verification passes for `8143cf3334a930a40a674a9bc8d0267cd0561642` (2,588 exported files, 29.73 seconds), including Darwin/Windows compilation. This is bounded
recovery of owned staging, not daemon-crash or remote-command lifetime proof.
Production handoff, real jobs sharing the warm generation across fresh engines,
whole-machine cross-class accounting, automatic expiry of owned containers/images
and scoped builder cache, and sustained pressure convergence remain required.


### Merged object retirement and rebased retention chain

Act2 #38's Linux rerun passed and the object-retirement primitive merged as
`e8b69c8`. PRs #39–#43 are now reconstructed on that actual merged parent.
Fresh source-bound gates pass before every branch update, and each reconstructed
tree is identical to its reviewed predecessor:

| Slice | Exact source commit | Exported files |
| --- | --- | --- |
| Object sweep (#39) | `8d84fc2e39974771030b7e6d8b4c6253d292d0d9` | 2,580 |
| Stage ownership (#40) | `c58390b1cb0cf27a988236dff4c66c99eb85ee50` | 2,585 |
| Combined stage policy (#41) | `f02fe9bf97231916035f7b71d19ff34baed9db4b` | 2,586 |
| Retention CLI (#42) | `bf47a59619ed8cb84bb2dcff1bd6700c79d53172` | 2,588 |
| Selection-stage recovery (#43) | `6db80530c2d81f6b517a35fe2c8d98dbafbe7bc2` | 2,588 |

The private Go harness's 6 GiB tmpfs filled during the final source export:
3.2 GiB of build cache, 849 MiB of modules, 283 MiB of lint cache and repeated
116 MiB source copies were observed. The incomplete export was not used for
push or verification. A new task-owned, read-only-root container with a 12 GiB
private tmpfs received the existing Go caches and a complete clean source export.
Disabling networking initially failed existing cache-server tests' outbound-IP
discovery; restoring the original bridge configuration fixed the harness. The
final source-bound gate passes in 74.24 seconds, with full cache tests, focused
commands, vet/lint and Darwin/Windows compilation, and 4.5 GiB observed tmpfs use.
This is test-harness capacity evidence, not proof of production cache convergence.

Bosn's committed shutdown-fixture correction passed its exact-source Rust lane
in 669 seconds with clean-source and executed-step proof. Its Linux lane remains
running; no completed full Bosn gate or publication is claimed yet. Act2 remote
checks are running on the reconstructed heads. Preserve ancestry with ordinary
merge commits for this chain where repository settings permit; exact default-
branch full CI and artifact checks remain required before the necessary release.
No release newer than act2.7 exists, and Bosn still has no activation of these
shared-tool-generation retention features.


### Production bootstrap boundary: current code and required change

Inspection of `act_engine/create.rs` and `ci/engine.rs` confirms that the
controller currently registers and starts the created engine before
`prepare_engine` waits for readiness and installs the pinned act binary.
`prepare_run` then copies the legacy tool cache into the inner volume. The current
PID-1 command executes `ENGINE_INIT` directly; it does not admit or retain a
shared-tool-generation reader. The earlier native-exec experiment therefore
does not establish production reader ownership.

The production change must freeze the tool-store mount, chosen generation and
act artifact digests together with the exact startup command. Bootstrap can
reuse the current digest-verified cached archive installation before entering
the existing engine INIT, then replace PID 1 through `act cache tool-exec` so
the original reader descriptor survives into INIT and the running engine.
Installing act only after readiness cannot supply that startup guarantee.
The inner writable tool-cache volume must be an overlay of the verified
read-only generation and private upper/work directories, rather than the
current whole-tree seed copy.

Downloading or staging before reader admission leaves a possible retirement
race. The helper must refuse missing or invalid chosen generations; the
controller must replan/retry from the latest valid warm selection, never
substitute an empty cache. Durable engine ownership remains required before
creation/start, recovery must validate the frozen profile, and uncertain start
or cleanup must not release protection based only on client cancellation or
daemon death. Completed installs may publish successors only after actual
workflow writers are quiescent. These are planned production requirements,
not implemented behavior or completed lifetime proof. Large-cache admission
currently hashes full payloads and still requires measured optimization.


### Publication checkpoint after passing gates (2026-10-04)

Act2 #39, #40, #41 and #42 are merged with ordinary merge commits after all
remote checks passed on their exact reviewed heads. Their ancestry is retained
for the remaining selection-stage PR #43, now retargeted to master. Its Linux
check remains pending. Full release CI on the final default-branch candidate
has not yet been dispatched, and act2.7 remains Bosn's pin.

Bosn #497 is merged as `51df270e`: the atomic PID fixture and bounded reaping
observation passed the complete local gate in 1,379 seconds (Rust669s, Linux710s).
The pinned verifier accepted stamped source `9407300a38d78bfb27fda619a47f52145579812f`
and remote local-gate attestation/minimal checks passed. Earlier pending-gate
statements describe the preceding checkpoint. Production cancellation behavior
is unchanged; remote-command and engine-lifetime proof remains outstanding.

The queued object/stage/CLI/selection documentation is being consolidated onto
this fixed Bosn main so the required documentation publication gate includes
the corrected shutdown test. This checkpoint does not claim an act2 feature
release, production generation admission, machine-wide convergence, or automatic
expiry of all owned engine/image/builder-cache classes.


### Actual hosted-workflow sharing: metadata copy-up regression

A private nested-Docker experiment built act from the exact reviewed source
`6db80530c2d81f6b517a35fe2c8d98dbafbe7bc2`, used Bosn's pinned Ubuntu runner
image, and mounted a task-owned read-only lower containing a 16 MiB payload and
completed install marker. Two actual dependent act workflow jobs read the same
payload; the second observed the first job's settings change. Both workflows
completed successfully, but the physical-efficiency assertion was RED: the
runner's recursive ownership reconciliation copied the entire payload into the
private upper. Its payload allocated 32,768 blocks (512 bytes each), and the
upper grew from zero to 16,404 KiB. Plain Docker jobs had not exercised that
hosted-user setup, so their earlier small-upper result did not cover this case.

The same experiment is GREEN with the tool overlay mounted `metacopy=on`,
confirmed in the live engine's mountinfo. Two actual hosted workflow jobs share
changes in the first engine; a job in a second fresh engine reads the original
warm lower. All three payload digests match. Upper growth is 20 KiB and 12 KiB;
upper payload entries have logical size 16 MiB but zero allocated blocks. Small
settings writes copy their data while the shared payload remains in the lower.
Both exact owned engines and the lower volume were removed after label checks.

[Kernel OverlayFS documentation](https://docs.kernel.org/filesystems/overlayfs.html#metadata-only-copy-up)
explains that metadata-only copy-up delays payload copying for ownership/mode
changes until a write needs file data. It requires trusted upper/lower layers;
production must enforce that boundary, check support and actual mounted mode,
and refuse/replan rather than silently regress to full payload copies or an
empty cache. This fixture used fresh task-owned layers, not arbitrary imported
overlay metadata.

This establishes actual act workflow sharing and the metadata allocation
requirement in a controlled prototype. It does not prove Bosn's frozen
production profile, immutable-generation/native-reader admission, actual
completed-install publication, daemon/uncertain-start recovery, sustained
whole-machine pressure convergence, or cache-action sharing. Artifact and
report evidence: `retention-toolcache-cow-actual-workflow-metadata-copy-red` and
`retention-toolcache-cow-actual-workflow-experiment` JSON/log/script under the
session git artifacts. First transport, minimal-runner-image and capacity
failures are retained separately; their owned fixtures were cleaned. The pinned
runner needed about 2,752,220 KiB of inner image storage, and the successful
fixture gave each engine a 4 GiB private storage tmpfs instead of 2 GiB.

### Release and documentation publication checkpoint

Act2 #43 is merged as `2029595941c40866b7114bf62b33d52ea54fc781`. Full default-
branch CI run `37226748284` passed all five required jobs on that exact commit.
A clean default-branch candidate checkout, absent tag/release and unchanged
remote master were verified before pushing necessary tag `v0.2.89-act2.8`.
Its existing tag-triggered release workflow is publishing; binary/checksum
verification and Bosn pin update are still pending. No runtime activation is
claimed.

The consolidated object/stage/CLI/selection spec passed the complete pinned
Bosn gate in 921 seconds (Rust474s/Linux447s), and exact verification accepted
stamped source `e68f8fe5e7b387ca77a67491898090752632a907`, tree `ea1e13959129`.
Independent documentation review passes; its PR is being published. This
follow-up records the newer workflow experiment and release evidence.


### Verified act2.8 artifact and Bosn pin candidate

The release workflow `37227718381` succeeded on the exact fully tested candidate
`2029595941c40866b7114bf62b33d52ea54fc781`; the annotated release tag resolves
to that same commit. All eleven release archives match both the checksums file
and GitHub asset digests. Their contained binaries have the expected target
architecture and embedded version. The actual Linux x86_64 binary reports
`act version 0.2.89-act2.8` inside the private Linux container; released
`tool-retain` and `tool-exec` command flags are present.

The Linux x86_64 archive digest is
`95b1b7f01da6f5ca22e206847d419c80fa04198f0cd568f986f96448ba7230d4`;
its binary digest is
`743c13bf6c8ee8ff948a14f940f6d1033ab4e09d11d29d94fda8e1bbaeea8628`.
The Bosn candidate updates the one ACT_VERSION constant, artifact URL and both
digests together. The stock runner image remains the pinned published image.
Source review and Bosn's required source-bound gate are pending before push.
The consolidated spec is now merged in Bosn #499, main `fe0a3d3d`.

This pin makes the verified retention and native-reader CLI available to Bosn
engines; it does not enable the new production generation profile, metadata-only
overlay mount, quiescent successor publication or whole-machine scheduling.
The default engine still follows the inspected legacy copy path until those
production changes are implemented and verified.


### Released-binary workflow verification (2026-10-04)

The same actual hosted-runner workflow fixture was repeated with the verified
released Linux x64 binary from `v0.2.89-act2.8`, rather than the source-built
prototype CLI. Its binary SHA-256 is
`743c13bf6c8ee8ff948a14f940f6d1033ab4e09d11d29d94fda8e1bbaeea8628`.
All three jobs passed: two dependent jobs in one engine shared their writable
cache, and a fresh second engine began with the same warm immutable payload.
The first engine's settings mutation was visible to its dependent job and
did not change the shared lower or the fresh engine's initial settings.

All jobs read the same 16 MiB payload digest
`811e721a3e02f4407710f89f854e913d9f5f661104f7e41c8610c58b13705f9d`.
The upper payload had zero allocated blocks in both engines, despite hosted
runner ownership setup; their final upper allocations were 20 KiB and 12 KiB.
The fixture verified the metadata-only overlay mount mode. Both exact engine
IDs and the shared lower volume were removed after checking their original
experiment ownership labels. Evidence: local audit artifacts
`retention-toolcache-cow-released-v8-workflow.{py,json,log}`.

This verifies the released executable against the private workflow fixture.
It does not activate production generation admission, prove daemon recovery
or completed-install publication, measure the real 7.7 GiB cache's admission
cost, or establish machine-wide accounting and automatic expiry. Production
still installs act after engine startup and uses the whole-tree tool seed.
The next implementation must change that startup and recovery contract before
replacing the production seed with the verified shared-generation overlay.


### Production startup candidate: verified act before engine INIT (2026-10-04)

After pin PR #501 merged as `139953f509075ff59a60c19d49414e5abe1e3f9f`,
the next source candidate changes the actual cache-backed engine command.
`creation_profile_with_cache` now freezes a bootstrap command that installs
the pinned archive, checks both archive and extracted binary SHA-256, closes
the archive writer descriptor, and replaces itself with the existing PID-1
INIT → docker-init → dockerd chain. The shared install script is extracted
into `ci/engine/act_install.rs`; startup and post-readiness verification use
the same implementation. Creation refuses an act version/archive identity
that differs from this startup artifact and refuses a noncanonical cache
mount. Engines without the cache retain their original command identity.
Historical engine recovery still checks its recorded command digest; it does
not substitute the new producer's command. This source is not yet published.

The focused regression was RED on the old command and GREEN after the change.
A second RED showed that creation accepted a mismatched recorded artifact;
it is now refused. All 21 engine-boundary tests passed. The compiled Rust
command generator was also exercised in the private Docker daemon: a valid
released archive started dockerd 29.7.2 in 1.66 seconds, exposed the verified
act2.8 binary, and allowed exclusive acquisition of the archive writer lock
while the engine remained live. A corrupt cached archive with no network
exited with code 1 after 5.46 seconds. The exact observed Docker command
matched the compiled generator. Both engines and the cache fixture volume
were ownership-checked and removed. Audit evidence is
`retention-bootstrap-{focused-red,focused-green,identity-red,identity-green,generated-command,runtime-proof}`
(local logs and the runtime proof's Python/JSON artifacts).

Because installation now precedes readiness, readiness has the previous
30-minute install allowance plus the previous 60-second daemon allowance.
It parses Docker's startup state into a typed struct after failed daemon
probes and immediately refuses stopped or unprovable engines; it does not
wait the download budget after an observed exit. Each probe uses the smaller
of the remaining total allowance and the control-operation budget. The typed
state refusal test passed. Full source gates and cumulative review are still
pending for this candidate.

This moves the production bootstrap boundary needed for native generation
admission. It does **not** acquire a generation reader yet: the production
planner still selects the legacy cache route, and `prepare_run` still copies
the legacy tool tree. Frozen generation selection/native exec handoff, trusted
metadata-only overlay mounting, recovery/lifetime proof, quiescent completed
install publication, large-cache optimization, and whole-machine convergence
remain unfinished. The warm-archive runtime proof does not measure cold
network download latency.


### Startup review correction: installer failure propagation (2026-10-04)

Cumulative review rejected the first startup candidate: its shared installer
ended with `tar && binary-check && version`, followed by FD close and INIT.
A nonfinal failure in that AND-list does not trigger shell `set -e`, so the
appended INIT could mask failed extraction or binary verification. The earlier
corrupt-archive experiment exercised the explicit download refusal only and
did not prove these two paths. The earlier source gate, even if it passes,
cannot authorize publishing that candidate.

Two focused shell regressions using the compiled command generator reproduced
the defect independently: failed extraction and failed binary checksum both
entered an INIT sentinel and returned success (RED). The shared installer now
explicitly exits with the failed chain's status before any later command. Both
regressions passed (GREEN), along with all 23 engine-boundary tests. Full Rust
command regeneration and a fresh private runtime proof also passed: verified
released archive → actual INIT/dockerd in 1.56 seconds, correct binary/version
and released archive writer; corrupt archive without network → exit 1 in
5.52 seconds. Exact command identity and label-checked cleanup were confirmed.
Evidence: `retention-bootstrap-install-failure-{red,green}.log` and
`retention-bootstrap-fixed-runtime-proof.{py,json,log}`.

This correction remains an unpublished startup candidate. Cumulative follow-up
review and a new exact-source full gate are required. It does not add production
generation admission or replace the legacy seed; the outstanding cache and
machine-wide retention requirements above still apply.


### Writer-quiescence survey and uncertain-save correction (2026-10-04)

A further source survey found that `ci/lifecycle.rs` calls `save_toolcache`
after cancellation, timeout, and execution transport failure if the engine
claim still verifies. The shared legacy lease covers act execution; it does
not cover the post-execution tool-cache save. Ownership of a live engine does
not prove that its workflow writers have stopped. This is relevant to the
planned native publisher's explicit source-quiescence requirement.

There is actual runtime evidence: the rejected gate28 run
`c2d22a89-ece4-4bf1-b888-a94dd8ffd258`, using released Bosn 0.1.15, ended
`cancelled` but logged `tool cache saved in 0.5s` before its confirmed removal.
The current source had the same unconditional save path. A controlled writer
in the private Docker daemon independently continued changing an install
payload after its launching client was terminated: progress advanced from
2 to 6 over two seconds while the labelled engine remained live. The client
reported exit code **0**. The engine was then ownership-checked and removed.
Evidence: `retention-bootstrap-rejected-gate-cancellation.json`, that run's
`log.jsonl` sequences 2834–2836, and
`retention-toolcache-cancelled-writer-proof.{py,json,log}`. This controlled
writer is not a complete production workflow or publication proof.

The new lifecycle candidate skips tool publication for known cancellation,
timeout, and execution-failure outcomes, preserves the existing machine-wide
warm cache, and records why saving was skipped. Normal reported act exits
still use the existing best-effort save, including completed installs from
failed jobs. The timeout regression was RED. The cancellation fixture's old
fixed delay could cancel before execution, so it now waits for an explicit
execution acknowledgement; that acknowledged cancellation was also RED.
Both are GREEN with the correction. All 14 lifecycle tests passed; the actual
cohort Docker fixture remained ignored and is not claimed as coverage.
Evidence: `retention-toolcache-uncertain-writers-red.log`,
`retention-toolcache-cancellation-acknowledged-red.log`, and
`retention-toolcache-uncertain-writers-acknowledged-green.log`.

This correction is an unpublished child of the startup candidate. It prevents
saving from **known uncertain** executions, but ordinary client exit is not
sufficient proof of writer quiescence, as the zero-exit Docker experiment
demonstrates. Do not set native publication's source-quiescent flag from an
exit code or engine ownership check. The production transition still needs
an engine-scoped mechanism that excludes all relevant writers during snapshot
publication and proves recovery/lifetime behavior. Generation admission,
trusted shared overlays, large-cache efficiency and whole-machine automatic
expiry remain unfinished. Cumulative review and full publication gates remain
required for this child candidate.


### Startup merge and next publication boundary (2026-10-04)

Startup PR #502 is merged as `797447baac73ff86e011eeeb214fdea37dfc72c0`.
The corrected source passed all required local lanes in 937 seconds, exact
stamped-head verification, and remote required checks before merge. Production
cache-backed engine creation now uses verified act bootstrap before INIT; it
still has no tool-generation reader and still copies the legacy tool tree.
The uncertain-save child was rebased onto this main commit with an identical
source tree, preserving its reviewed behavior and focused evidence. Its full
publication gate remains required. Performance issue #503 tracks reducing the
complete ordinary changed-source gate below 60 seconds without dropping checks.

A possible engine-scoped snapshot mechanism is the cgroup v2 freezer. The
[kernel documentation](https://docs.kernel.org/admin-guide/cgroup-v2.html)
requires waiting for the frozen acknowledgement in `cgroup.events`; requesting
a freeze is asynchronous. Process migration remains possible while a group is
frozen. Therefore a production mechanism must also exclude new writers and
uncontrolled migration, identify every relevant writer group, preserve the
engine-owned generation reader, and bind the control operation to the verified
private engine. A frozen flag alone is not a source-quiescence receipt. This
is an investigation direction, not an implemented freezer or publication proof.


### Backing-filesystem attribution correction (2026-10-04, candidate)

A source survey found that `ci/engine.rs::storage_usage` exclusively executes
`df -Pk /var/lib/docker`. For a disk-backed anonymous volume this describes the
whole host backing filesystem: the 1508/1830 GiB reported during gate29 is not
that engine's allocation. The candidate labels warnings, peak logs and failure
explanations as backing-filesystem pressure and explicitly excludes attribution
to engine-owned bytes. The measurement and low-space thresholds are unchanged.

This is a reporting correction, not an allocation implementation. Engine-owned
allocated-byte measurement, cross-class inode deduplication and separate Docker
backing/VHD observations remain required. Do not add these filesystem counters
to shared-cache totals or derive deletion eligibility from them. The four storage unit tests and two affected failure/lifecycle tests passed
in isolated Docker via `soldr cargo`; `git diff --check` passed. Review and the
required publication gate remain pending; production remains unchanged.


An isolated Docker filesystem fixture confirms the attribution and deduplication
risk: a 16 MiB allocated file plus its hard-link alias and a 256 MiB sparse file
had 16 MiB unique file allocation (plus a 4 KiB directory), while summing file
rows gave 32 MiB. The sparse file allocated zero blocks. `df` reported roughly
1.46 TiB used for the backing filesystem. Evidence is retained locally in
`.git/retention-accounting-scope-proof.json`. This fixture verifies Linux inode
and block semantics only; it does not prove production machine-wide inventory,
VHD reclamation, atomic concurrent accounting or automated expiry.


### Released act2.8: 1 GiB warm-cache measurements (2026-10-04)

A controlled fixture used the verified released Linux x64 binary, an isolated
Go container's private `/tmp` tmpfs, one quiescent completed install with a
1 GiB allocated zero-filled payload, and a 2 GiB logical-byte bound. All timings
include `docker exec` client overhead; they are wall time, not CPU time.

| Phase | Seconds | Result |
|---|---:|---|
| Create source payload | 1.526 | Completed before publication; no source writer remained |
| Publish closed object | 6.985 | Published, complete |
| Assemble and initialize selected generation | 2.217 | Selected, complete |
| Validate current selection, three warm runs | 0.926 / 0.881 / 1.082 | Same generation ID |
| Acquire reader and exec, three warm runs | 0.876 / 0.832 / 1.237 | Reader descriptor environment value verified |

The explicit bounded metadata audit visited 19 paths and 18 unique inodes,
reported 1,073,790,976 allocated bytes and 2,147,517,791 referenced file bytes,
and was complete. This confirms object/generation payload hardlinks do not
double the unique allocation. The first audit command omitted `--max-entries`:
it correctly returned nonzero with partial status and unknown totals. That
failed invocation is retained as evidence, not reported as successful.

A negative control changed one payload byte while preserving size and mtime.
`tool-current` refused it in 1.384 seconds with `tool generation payload differs;
existing data preserved`. Restoring the original byte and mtime restored
admission in 1.341 seconds. An optimization based only on size and mtime would
miss this corruption; do not replace content validation with those fields.

Evidence: local `.git/retention-large-tool-benchmark.{py,json,log}`. Generation
`97b8606089402a23068d7bf32bb14cd9e23dbd25eb4dd7878a2506dd8ef08370`
and object `5d66737eddb513d4679de12b8f0f5298c67f21e3401871eb3652b4ca51f5cf72`
remain in the private fixture store for inspection. This does not measure cold
disk, many-file tool trees, 7.7 GiB payloads, production startup, actual PID1
lease lifetime, concurrent admission/publication, or complete gate latency.
The source still validates content on selection and again on reader admission;
production integration should avoid redundant planning validation where the
native admission already supplies the required proof, while preserving exact
frozen generation identity and refusal on missing or changed data.


### Writer uncertainty and concurrent native admission checkpoint (2026-10-04)

Bosn PR #504 is merged as `753d8a1575c6a8bc6a5a5085c353a387acf1afa9`.
Its full local gate passed in 998 seconds (Rust 448, Linux 550), exact stamped
commit `49ba670ab6a28e07cdfa42b1e7165ff3da4ac8eb` verified, and remote checks
passed before merge. Known cancellation, timeout and execution failure now skip
legacy tool saving. Ordinary reported exits still retain best-effort saving:
client exit alone still does not establish native publisher source quiescence.

The act2 admission survey found that generation payload hashing holds the
store-wide catalog mutex for the entire validation. That serializes independent
engines' large-cache admission and unrelated publication. Candidate act2
`071d429` acquires the original generation reader under catalog exclusion,
then releases the catalog before hashing. The reader excludes generation
retirement while content validation still runs unchanged. Failed validation
closes the reader and returns no admitted lease; native exec retains the exact
original descriptor as before.

The focused regression was RED with the old lock scope. GREEN coverage proves
the catalog is available during validation, another admission and unrelated
generation publication proceed, the current generation's exclusive reader-lock
writer remains blocked, and failed validation releases the reader. Existing
generation tests passed, including the race detector, in isolated Docker.
Evidence: `.git/retention-native-admission-lock-{red,green,race}.log`.
Primary source review and full PR platform CI passed. Act2 PR #47 is merged
as `3016b36fbb439b557b76675945b0ab33e02cc14d`; exact-default-commit checks run
`37234469601` passed all five required jobs before tagging. The candidate is not in act2.8 or
Bosn's pin. It does not remove full payload hashing or prove concurrent wall-time
improvement for 7.7 GiB trees. Production frozen selection, reader handoff,
shared overlay, source quiescence, and automatic retention remain required.


The native admission candidate also passed a real CLI concurrency comparison
against the verified released act2.8 Linux binary on the same warm 1 GiB
private tmpfs generation. Three simultaneous admissions per trial, three trials:
act2.8 admitted 3/9 and refused the other 6 with catalog `timeout`; candidate
`071d4299993b862aed724fd5da69bf166f792172` admitted 9/9. Candidate batch
wall times were 0.967, 1.027 and 1.060 seconds, including Docker client overhead.
All successful commands verified a nonempty inherited reader FD environment
value. Candidate binary SHA-256:
`61de1050ac88da869fd1dba0b49b30e6ce97b9a8dbaa70c413d3f96fdfcce1de`.

Evidence: `.git/retention-native-admission-concurrent-runtime.{py,json,log}`.
This is an actual simultaneous CLI admission test, not a production Bosn
workflow or PID1 lifetime test. It confirms catalog contention can fail warm
admission even on a 1 GiB tree, and that shortening exclusion resolves those
observed failures without dropping payload validation. It does not establish
cold-disk scaling, general starvation bounds, many-engine pressure limits,
7.7 GiB performance, or complete gates under 60 seconds. The whole artifact-cache
package passed with the race detector (22.734 seconds), including retention
coverage, after the focused generation tests.


### Post-stop read-only publication survey (2026-10-04, fixture only)

A controlled private-Docker fixture retained a task-owned named source volume
while removing its writer container. The writer advanced from 10 to 21 after
its launching client reported exit code 0. After label-checked source-container
removal, inspection confirmed its absence and no volume attachments. A separate
controlled finalizer wrote the completion marker and exited; it was removed
before the publisher started. A fresh helper mounted the exact original source
volume read-only and a distinct task-owned destination volume read-write.

The verified released act2.8 CLI published a complete `sibling-v1` object from
that read-only source. Payload remained `23` over the stability check and the
published payload matched it. Native report: published=true, partial=false,
2 payload bytes and 2 entries. All source/finalizer/helper containers and both
volumes were label-checked and removed; cleanup completed.

Evidence: `.git/retention-post-stop-publication-proof.{py,json,log}`. Earlier
streaming and noexec-tmpfs harness failures are preserved separately as
`retention-post-stop-publication-proof-{stream,exec}-failure.{json,log}`; their
resources were cleaned up and they are not successful publication evidence.

For disk-backed production runs this supports investigating publication from
retained private storage *after* proven source-engine shutdown, instead of
claiming quiescence from client exit or a completion marker. Required integration:
freeze the exact named storage identity; protect it against cleanup/GC throughout
the handoff; prove original engine/process absence and refuse unknown or foreign
attachments; mount that original source read-only in an owned publisher; retain
old-generation protection while reconstructing any merged overlay; publish only
completed entries into the latest selected generation; then reconcile helper
and source-volume removal. Partial publication must preserve the prior shared
selection and expose pending storage/byte-budget evidence for bounded retry or
safe expiry.

This fixture does not implement that lifecycle, recover overlays/metacopy data,
prove arbitrary workflow process/cgroup exclusion, or authorize removal of active
engines. It does not apply to memory-backed private storage, which disappears
with its engine; that mode requires a separately verified handoff while data
still exists. Bosn still uses legacy best-effort tool saving on ordinary exits.


### Compiler-cache survey during accounting gate32 (2026-10-04)

Rust run `f8909fcd-e93a-43a4-90ec-2e656d228d59` passed in the 863-second
pre-rebase gate. Its setup-soldr post-step explicitly reported setup-cache miss,
target cache disabled, compiler build-cache exact hit, and Cargo-registry cache
disabled. The exact-hit compiler archive was not saved again (`layers_saved=0/1`,
`uploaded=0B`), despite compilation producing new cache objects in the run's
private cache. This merits investigating how useful new objects reach the next
fresh engine; an exact-hit immutable archive alone does not demonstrate that.

The final compiler summary reported 57 hits, 7 misses, 65 compilations and one
non-cacheable invocation. Its own provenance said `global-fallback`, originating
workspace unknown, and missing per-session journal. Do not attribute those
counts, the 89.1% hit rate, or estimated 63.6 seconds saved to the entire Rust
lane: the report is last-writer-wins fallback evidence, not a complete sum of
all compiler commands. Separate log entries include foundation misses. The
workflow test step took 3m45.7s even with cache restoration; successful archive
restore is not proof of efficient end-to-end execution or persistent new-cache
publication.

Evidence: `.git/retention-gate32-compiler-cache-survey.json` and the run's retained
`log.jsonl`. No compiler-cache policy was changed by this survey. Native shared
tool generations, compiler object sharing/coordination, and artifact archive
cohort retention are distinct contracts and need separate accounting and
publication evidence.


The prepared act2.9 release-asset verifier was exercised against all 11 existing
act2.8 archives as a control. Checks passed for archive/checksum-file hashes,
independent GitHub asset digests and sizes, executable architecture headers, and
embedded version strings. Control evidence:
`.git/act2-native-admission-release-v9-verifier-control.json`. This validates the
verification procedure on the existing release; it is not act2.9 release evidence.
This control alone supplies no tag authorization; the separate exact-commit
full CI gate is the release prerequisite.


### Act2.9 tag and release workflow checkpoint (2026-10-04)

Exact default-branch commit `3016b36fbb439b557b76675945b0ab33e02cc14d`
passed dispatched checks run `37234469601`: lint, snapshot, Linux, macOS and
Windows all completed successfully. A clean checkout, unchanged default-branch
head and absence of both the tag and release were verified before tagging.
Annotated `v0.2.89-act2.9` tag object
`60b924efb73d76ff3368fc28cded65190c9597b1` peels to that exact commit.
Existing release workflow run `37235462650` passed on the tag.

Pre-tag evidence: `.git/act2-native-admission-release-v9-pretag-gate.json`.
Release archives, independent asset digests, binary headers/versions and Linux
runtime smoke still require verification. Bosn's pin remains act2.8 until those
checks pass. This release step alone does not activate production shared tools,
archive cohort migration, per-class automatic expiry or machine-wide allocation.


### Released act2.9 artifact verification and overlay recovery evidence (2026-10-04)

All 11 act2.9 release archives passed checks against the checksum manifest and
independent GitHub asset digests/sizes, executable architecture headers, and
embedded version strings. Linux x86_64 executed in the private Go container and
reported `0.2.89-act2.9`. Other platforms have static artifact verification and
exact-commit CI evidence, not local native execution. Evidence:
`.git/act2-native-admission-release-v9-verification.json`.

Accounting PR #505 merged as `eb8cac903cf0df2df605567bc73721e4f585ad58`.
Its exact combined candidate passed gate33 in 999 seconds. Storage pressure is
now explicitly backing-filesystem usage, not engine-owned cache allocation;
this wording correction does not implement cross-class physical accounting.

The post-stop overlay fixture now passes. A task-owned source container used an
immutable lower and retained disk upper/work; chmod created a metadata-only
16 MiB payload copy-up with zero allocated upper blocks. After label-checked
source removal, a fresh helper reconstructed the metacopy overlay and explicitly
remounted the merged view read-only. Released act2.8 published completed
`Python/3.13/x64` and `New/2/x64` objects. Payload hash and mode 600 matched,
changed private settings were published, lower settings stayed unchanged, and
upper payload allocation remained zero blocks. All fixture containers and three
volumes were removed after label checks. Evidence:
`.git/retention-post-stop-overlay-recovery-proof.json`.

The helper mounted retained upper/work storage writable to construct the overlay;
only the merged publication source was read-only. This controlled kernel/native
publication fixture does not prove production Bosn lifecycle fencing, original
reader handoff, migration, arbitrary writer exclusion, whiteout/opaque/redirect
handling, or memory-storage recovery. Production post-stop recovery remains
unimplemented. Earlier capability/remount setup failures are retained separately;
they were corrected before this success was recorded.


Released act2.9 repeated the warm 1 GiB concurrent admission fixture: three
simultaneous readers across three trials succeeded 9/9, versus act2.8's 3/9
(the other six hit catalog-lock timeouts). Released act2.9 batch wall times were
1.339, 1.312 and 1.369 seconds including Docker client overhead. Commands asserted
a nonempty lease-FD environment value; this does not independently prove full
production descriptor lifetime or PID1 handoff. Evidence:
`.git/retention-native-admission-released-v9-runtime.json`.

This branch now updates Bosn's act version, Linux archive URL, archive digest and
executable digest to verified act2.9. Source review and the complete local gate
remain required before publication. The released native admission optimization
permits concurrent content validation while the original generation reader
protects the payload; it does not weaken corruption checks or activate Bosn's
pending shared-cache lifecycle.

### Original native reader through real engine PID1 (2026-10-04)

A released act2.9 fixture published and selected a quiescent 16 MiB tool
install in a task-owned volume, then created a fresh privileged engine in the
private test daemon. `tool-exec` replaced itself with Bosn's actual `ENGINE_INIT`
command and the same pinned dockerd arguments. The engine reached Docker
readiness. `/proc/1/environ` recorded lease FD 7 and `/proc/1/fd/7` referenced the
original selected generation's `.readers-v1.bolt`. An independent helper sharing
the same store failed a nonblocking exclusive flock while the engine was alive
(exit 1, no diagnostic); after label-checked engine removal the same command
succeeded (exit 0). Both fixture containers and both volumes were removed after
ownership/attachment checks. Evidence:
`.git/retention-native-pid1-handoff-proof.json` and its executable fixture.

The fixture used the production init command and dockerd flags, but directly
created its engine: no production planner, frozen generation binding, automatic
migration, tool overlay preparation, job execution or post-stop publication is
claimed. It establishes that native original-reader handoff survives the shell
and `docker-init` chain and excludes an independent writer for the live engine.
The initial fixture attempt misread the typed update report shape; it failed
before engine creation and cleaned up all resources. Its retained failure is
separate from the corrected successful lifetime experiment.

### Frozen generation startup integration (working implementation, 2026-10-04)

The integration branch adds an optional typed `ActToolGenerationBinding` to the
persisted creation profile: a canonical SHA-256 generation ID and positive,
signed-64-bit-bounded payload validation ceiling. It requires a coordinated
shared cache. Absent bindings are omitted from serialization, preserving the
historical profile identity. New engine creation recomputes the complete frozen
command, refusing a changed binding paired with an old command digest.

The generated cache bootstrap installs the verified pinned act, closes its
archive descriptor, then invokes native `cache tool-exec` against the fixed
`/bosn/cache/toolstore-v1` store before replacing itself with the existing engine
init chain. The generation ID and payload bound are individual argv values.
This command path is implemented, but runtime planning does not yet select or
freeze bindings. Generation admission precedes any eventual tool overlay; no
production overlay or lifecycle activation is claimed.

The focused test failed because the persisted profile rejected the generation
field, then passed with typed persistence and command binding. Evidence:
`.git/retention-native-reader-profile-red.log` and `-green.log`. All 24 scoped engine boundary tests passed, and the stalled-discovery shutdown
regression passed alone (one test, 0.27 seconds for the test run). This does not
establish the cause of the earlier full-suite latency failure. Evidence:
`.git/retention-native-reader-profile-scoped.log`. This branch is not reviewed,
gated or published.
Separately, the act2.9 pin's gate34 failed at the stalled-discovery shutdown
latency assertion (6.998 seconds against a five-second ceiling), after format
and Clippy passed. That failure is retained and under investigation; no gate34
attestation authorizes publication.

### Native overlay preparation and real jobs (working implementation, 2026-10-04)

`prepare_run` now receives the frozen typed generation binding from its engine
intent. Legacy plans retain their seed path; a bound generation uses the new
`toolcache_overlay.sh`. Before mounting, the script checks PID1's original reader
FD, exact expected reader path and device/inode identity. It refuses missing or
invalid descriptors, a nonempty target, and preexisting upper/work paths. The
selected immutable generation is the overlay lower; upper/work live in the
engine's private `/var/lib/docker`. The mount explicitly requests metacopy.

The actual generated overlay script passed in a released act2.9 engine using
Bosn's init command and dockerd flags. Two real nested job containers mounted
`act-toolcache`: job one changed the payload mode and wrote private settings;
job two observed both changes and the identical payload hash. The 16 MiB payload
copy-up occupied zero upper blocks. The shared generation acquired no settings
file. Independent exclusive-reader probing remained blocked until engine removal.
All owned fixture containers/volumes were cleaned up. Evidence:
`.git/retention-native-overlay-jobs-proof.json` and its executable fixture.
All 14 existing scoped lifecycle tests passed (one physical cohort test ignored);
evidence `.git/retention-native-overlay-lifecycle-tests.log`.

This connects preparation to a bound plan, but the normal runtime planner still
creates unbound plans. Before activation, completed private upper installs must
be published after writer exclusion, disk storage must survive until verified
publication/recovery, and recovery must revalidate live reader/mount identity.
The existing ordinary-exit legacy saver is not native generation publication;
it must not be used as proof that new overlay installs persist to a fresh engine.
Automatic generation selection and maintenance are still absent.

### Effective overlay and frozen preparation recipe (2026-10-04)

The binding now also requires `overlay_recipe_sha256`. The producer hashes the
exact preparation script with canonical store/target paths and the generation
placeholder; the separately frozen generation ID completes its inputs. Creation
and preparation refuse a recipe mismatch, so a new daemon cannot silently use a
changed overlay recipe with a previously frozen generation profile. Profiles
without a generation still omit the entire binding.

After mount, the actual script reads `/proc/self/mountinfo` and requires exactly
one matching writable overlay with the exact lower/upper/work paths and effective
`metacopy=on`. Success of the mount command alone is insufficient. The updated
real two-job fixture passed these checks with zero allocated blocks for the
16 MiB metadata-only upper payload. A helper without a live PID1 native reader
was refused before overlay setup (`native tool reader descriptor missing`). The
original reader continued excluding a writer until engine removal. Fixture
cleanup completed. Evidence: `.git/retention-native-overlay-jobs-proof.json`;
the earlier successful pre-verification report is retained separately.

The focused frozen-profile test also passed with rejection of a changed recipe
digest: `.git/retention-native-overlay-recipe-binding-test.log`. No automatic
runtime selection, native publication, recovery fencing or expiry activation is
claimed. The act2.9 pin retry gate35 passed its Rust lane in 415 seconds; its
Linux lane is still running, so publication remains unauthorized by that gate.

### Pending lower protection survey and RED retention contract (2026-10-04)

Act2's retirement paths protect the current selection and original live reader
locks. They have no durable reference for a stopped engine's pending overlay.
If every holder dies and another generation becomes current, the old lower is
eligible for age/pressure retirement even while pending private upper data needs
it. The successful live-helper experiment does not cover this holder-free gap.

A new focused contract test on act2 default commit
`3016b36fbb439b557b76675945b0ab33e02cc14d` initializes a generation, selects a
successor, and writes a proposed canonical unexpired recovery reference under
`.tool-recovery-pins-v1`. No reader process remains. Aggressive native retention
removes the old generation; the test fails because that lower is absent.
Evidence: `.git/retention-native-recovery-pin-red.log`; working test
`pkg/artifactcache/tool_recovery_pin_linux_test.go` in the sibling act2 worktree.
This demonstrates a missing desired recovery contract, not support for an
existing pin protocol. No shared machine store was used or pruned.

Required implementation: publish a bounded typed reservation under the original
catalog lock before stopping source writers; bind canonical owner/generation and
an explicit finite expiry; retain its referenced lower and objects without a
living process; reject malformed/ambiguous inventory before destructive work;
report protected allocation/overflow; permit convergence after verified release
or expiry. The native record is a protection request, not proof of Docker source
ownership or writer exclusion. Bosn must separately persist publication intent,
verify source/volume identity and absence, keep recovery state until publication
or authorized bounded expiry, and then reconcile both reference and source-volume
cleanup. Automatic planning remains disabled until these lifetimes are connected.

The act2.9 pin's exact gate35 completed successfully in 791 seconds (Rust 415,
Linux 376); the clean stamped head `30c76a974291e65d613ef32249fe1a08fc90aa32`
passed pull-request attestation verification. No full-objective completion is
implied by that pin gate.

### Durable lower retention checks (working candidate, 2026-10-04)

Act2's recovery-reference contract is now GREEN in the working sibling branch.
The original catalog writer guards a bounded inventory of canonical typed pin
records: owner and generation SHA-256 IDs, schema 1, creation and explicit expiry,
with at most a 24-hour lifetime, 10,000 records and 1,024 bytes per record. Active
references must name a valid existing canonical generation manifest. Future
creation times, malformed/oversized records, owner mismatches, symlinks and
missing active lowers refuse retention before mutation. Context cancellation is
checked during the bounded scan.

The automatic generation sweep preserves unexpired referenced generations and
includes them in protected-generation/overflow reporting. Direct generation
retirement enforces the same check. Object retention retains the surviving
generations' references through its existing generation inventory. Expired
references cease protecting generations. The sweep reads the reference inventory
once under the catalog lock rather than rescanning it for every candidate.

Focused retention/recovery boundary tests passed:
`.git/retention-native-recovery-pin-green.log` and
`.git/retention-native-recovery-pin-boundaries.log`. Test fixtures currently write
records directly. Safe reservation publication, acknowledgement recovery,
verified release, expired-record cleanup, CLI integration, review and release
remain implementation work; no production pin enrollment is claimed. This does
not yet preserve a source volume or implement writer exclusion in Bosn.

Bosn PR #507 is merged as `420c1ddbc889cacfc7f3f524129158f138cee871`.
The verified act2.9 version/archive/executable pin and accumulated measurement
spec are now on main. The native engine integration branch was rebased onto that
exact main commit; its production lifetime activation is still incomplete.

### Recovery reservation publication and older-peer fence (candidate, 2026-10-04)

`PublishToolRecoveryPin` now publishes a typed reservation after full generation
validation under its original reader. Payload validation releases the catalog;
publication reacquires it and re-reads reference state. The owner record is
immutable: identical retry reuses it, differing generation/time intent is refused.
Publication stages are tracked by the existing original-inode ownership ledger.
A synced file is linked atomically without replacement, then the reference
namespace is synced. Partial outcomes never prove absence. An injected final
parent-sync failure reports published-but-partial; retry reconciles and syncs it.

Before acknowledging a reservation, the same catalog writer changes the store
marker to `bosn-tool-snapshots-v2`. The new runtime accepts both epochs; released
act2.9 validates only v1 under that original lock and refuses the fenced store.
Only marker metadata changes: warm selection, objects, generations and original
catalog/reader inodes remain intact. Fence stages also use the owned stage ledger.
A failed fence or reference acknowledgement requires reconciliation; it does not
authorize stopping source writers. Existing legacy stores are not silently
upgraded by ordinary access or initialized as empty during refusal.

An actual released act2.9 binary admitted a private store before reservation and
refused it afterward (`invalid tool store marker`). The candidate retained the
same warm generation and both original coordination inodes. Evidence:
`.git/retention-native-recovery-pin-fence-probe.json`. Its small fixture remains
only in the private Go container. No shared machine store was changed.

Focused retention/publication/snapshot race tests passed in 1.486 seconds.
Tests cover immutable retry, uncertain parent sync and validation that releases
the catalog while retaining the original generation reader. Evidence:
`.git/retention-native-recovery-pin-publication-{red,green,race,fence-tests}.log`.
The API initially failed to compile because it was absent, then passed. Full
package/lint/platform/release gates and review remain pending. Verified release,
expired-record cleanup, CLI integration and Bosn's publication/source-volume
lifecycle still require implementation; normal planning remains unactivated.

### Bounded expired recovery reference cleanup (candidate, 2026-10-04)

Act2 candidate `077e2f7` now removes expired canonical recovery records under
original catalog exclusion after complete usage and candidate inventories pass.
Each removal rechecks the exact typed intent and original record inode. The
sweep removes at most `MaxCandidates` records, syncs the reference directory,
reports remaining expired records, and bounds owner receipts to 32 with an
explicit omitted count. Partial removal/sync outcomes stop further destructive
work and trigger a fresh allocation audit. Active references remain protective.

The focused retention/recovery race suite passed in 1.514 seconds:
`.git/retention-native-recovery-pin-expiry-boundaries.log`. A 41-expired-record
fixture removes eight in the first sweep, then 33 in the second, while retaining
its active reference and protected generation. The second report contains 32
owners and one omitted owner. The existing candidate inventory guard counts root
control entries as well as payload entries; an initial two-entry test budget
refused before mutation. The fixture budget was corrected to eight without
changing production limits or bypassing inventory validation.

Expired-record cleanup is implemented in this candidate; verified explicit
release, CLI integration, review, full package/platform/release validation and
Bosn source-volume lifetime activation remain incomplete. This evidence proves
bounded metadata expiry, not automatic machine-wide cache maintenance.

### Explicit recovery reference release (candidate, 2026-10-04)

Act2 candidate `4cc45cb` adds `ReleaseToolRecoveryPin`. Under original catalog
exclusion it validates the complete reference inventory and matches the exact
frozen owner/generation/creation/expiry intent before unlinking a reference.
Changed intent is refused without removal. A successful acknowledgement requires
syncing the reference namespace; an absent retry repeats that sync to reconcile
an earlier unlink whose acknowledgement was lost. Removed-but-partial does not
prove durable absence. Cancellation before mutation leaves the reference intact.
Non-Linux platforms explicitly refuse this operation without filesystem access.

The caller must durably record completion and exclude further publication by
that owner before release. The API does not establish source quiescence or
source-volume ownership, and it does not retain an unbounded owner tombstone.
These caller lifecycle obligations remain part of Bosn integration work.

Focused release/retention race tests passed, including altered-intent refusal,
injected directory-sync failure, idempotent reconciliation and cancellation:
`.git/retention-native-recovery-pin-release-{red,green,boundaries}.log`.
Before this addition the complete artifact-cache race suite passed in 22.114
seconds (`.git/retention-native-recovery-pin-full-package.log`); that result does
not validate the later release code. Candidate-wide review, lint/platform/full
package validation, CLI exposure and production activation remain pending.

### Recovery CLI and release package validation (candidate, 2026-10-04)

Act2 candidate `208504c` exposes `cache tool-recovery reserve --record FILE
--max-bytes N --apply` and `cache tool-recovery release --record FILE --apply`,
with the existing `--cache-server-path` store selection. Both read a bounded
regular schema-1 intent file into `ToolRecoveryPin`, reject unknown fields and
trailing data, and emit typed JSON receipts even for partial runtime outcomes.
The reservation validates the complete lower; release requires caller lifecycle
exclusion described above. Missing `--apply` refuses before mutation.

The CLI round-trip test publishes and releases a real fixture generation and
checks missing approval, trailing data and unknown-field refusal:
`.git/retention-native-recovery-pin-cli-tests.log` (passed, 0.021 seconds).
The full artifact-cache race suite with explicit release passed in 21.793
seconds: `.git/retention-native-recovery-pin-release-full-package.log`.
Candidate lint is now being checked. Review, cross-platform/full CI, release
artifact verification and Bosn normal lifecycle enrollment remain pending;
these commands are not shipped or called by normal Bosn planning yet.

Candidate lint failed with three complexity-limit violations (reference
inventory, reservation publication, combined retention), one import-format issue
and one trailing whitespace issue. Evidence:
`.git/retention-native-recovery-pin-candidate-lint.log`.
Reference record decoding has since been separated from complete inventory
validation; focused retention/recovery race tests still pass (1.519 seconds,
`.git/retention-native-recovery-pin-refactor-tests.log`). Lint is not yet green;
publication/retention refactoring and final formatting remain required before
review or shipping. No correctness gate was relaxed.

### Recovery candidate lint refactor (2026-10-04)

Act2 candidate `802f5ab` separates verified reservation publication, reference
namespace preparation, atomic linking/stage cleanup and post-expiry allocation
audit. The original lock/reader scopes and partial acknowledgement behavior are
preserved. Lint now reports zero issues:
`.git/retention-native-recovery-pin-refactored-lint.log`.
The complete artifact-cache race suite passed in 21.477 seconds after this
refactor. The combined CLI package run stopped at a default cache directory
creation on the private container's read-only root; this is retained in
`.git/retention-native-recovery-pin-refactored-full-tests.log`. A separate CLI
race run uses an explicit private writable XDG cache directory, without changing
production code or tests. Its result and the existing primary review are pending.

The existing sole primary reviewer completed all 18 changed Go files at
`802f5ab` with no blocking findings. Publication retains the original reader
across validation outside catalog exclusion; fencing and immutable publication
follow catalog reacquisition. Release continues to require durable caller
completion and exclusion of republishing. Review does not prove Bosn enrollment.

Windows amd64 and Darwin amd64/arm64 builds of the changed CLI/artifact-cache
packages passed with CGO disabled:
`.git/retention-native-recovery-pin-platform-builds.log` (terminal exit zero).
These are compile checks, not native platform execution or full release CI.
The complete CLI suite still fails in this private environment: default archive
server setup uses the read-only home cache even with a writable XDG action cache.
An isolated daemon resolved the earlier missing-socket error, without using the
host Docker socket. Failure evidence remains in
`.git/retention-native-recovery-pin-refactored-cmd-isolated-tests.log`.
Focused recovery CLI tests passed; the full CLI package is not claimed green.
Full remote CI must establish the release candidate gate in its normal test
fixture before any release. No tests or assertions were weakened.

The clean reviewed recovery candidate was pushed as act2 PR #50:
https://github.com/zackees/act2/pull/50
at exact head `802f5ab09ba44c26e6088b14c7598750a8acec23`.
The PR body discloses the private-environment full CLI failure and incomplete
Bosn enrollment. Existing remote full checks are the next verification gate;
no release or Bosn pin change has been made for this candidate.

### Physical post-stop recovery and a fresh engine (candidate, 2026-10-04)

A task-owned isolated-Docker experiment now exercises recovery through the act2
candidate CLI (`802f5ab`), the native PID1 reader and the current Bosn overlay
preparation recipe. Two inner jobs share changed settings and a metadata-only
payload mode change; the 16 MiB upper payload still allocates zero blocks. The
shared immutable lower remains unchanged. A durable recovery reservation is
acknowledged before the engine is removed; its source disk volume is retained.

After engine removal the original reader admits an exclusive probe. Selection
is advanced with an additional completed Node install. A pressure/age sweep
preserves the stopped engine's now-unselected lower through its recovery
reference. A helper takes that original native reader, mounts the saved overlay,
reads the identical payload hash and changed settings, and publishes the closed
Python install. Updating the latest generation preserves the newer Node install.
A fresh engine with a new empty private disk then mounts that selected lower;
an actual inner job sees both the recovered settings and newer Node payload,
with the original Python payload hash. The helper and fresh engine are removed,
reference release is durably acknowledged, and a subsequent sweep retires the
old lower. All task-labelled containers and volumes are verified and removed.

Evidence: `.git/retention-native-poststop-recovery-proof.{py,json,log}`.
The first candidate build used dynamic linking unavailable in the Alpine
fixture; a CGO-disabled build corrected the harness. A second attempt reached
reference protection but the harness treated a JSON null retired list as an
array. Both failed attempts and complete cleanup evidence are retained in
`retention-native-poststop-recovery-{dynamic-build,null-receipt}-failure` files.
No production assertion or retention limit was changed to obtain this result.

This is a controlled physical proof of warm recovery across fresh engines.
It manually supplies trusted source-volume ownership and stopped-writer evidence
and directly drives the CLI, bypassing the normal Bosn planner/registry cleanup.
Normal durable intent, bounded recovery scheduling, source volume lifetime,
publication completion and reference release still require Bosn integration.
It is not proof of automatic enrollment, compiler/archive persistence, machine
ceilings, cross-class inode accounting or old image/build/container expiry.

### Bosn durable recovery intent persistence (candidate, 2026-10-04)

Bosn candidate `6313f42` adds optional typed `ActToolRecoveryIntent` to the
self-contained engine registry snapshot. Historical records omit it. Under the
exact unexecuted registered engine claim, `begin_act_tool_recovery` binds the
original engine ID, deterministic named private source volume, shared cache
volume and frozen native lower. Its owner digest includes registry identity,
engine/run identity and the complete frozen creation profile. Recovery requires
persistent named disk storage; anonymous disk and memory-only profiles refuse.
Creation/expiry timestamps are frozen once, with a positive lifetime at most
24 hours. Whole-second reference timestamps preserve the registry's more precise
monotonic transition clock. An identical retry reuses intent; changed intent
refuses. Decoding rechecks the binding against the containing engine record.

The intent transaction rolls back without commit and survives writer close and
reopen after commit. Tests verify changed claim/lifetime refusal and the finite
lifetime bound. All 19 engine-registry lifecycle tests passed in 11.24 seconds;
registry all-target Clippy with warnings denied passed. Evidence:
`.git/retention-bosn-tool-recovery-intent-{red,green,boundaries}.log`.
The initial missing-API compilation evidence also contained a fixture typo in
the preexisting claim method name; it was corrected before the passing runs.

This persistence API does not acknowledge act2 reservation, establish source
quiescence, permit storage removal or schedule a recovery helper. It is not
called by the normal runtime yet. Reservation acknowledgement, source-stop
proof, completed publication/release/expiry transitions, retention of the source
volume during cleanup and bounded recovery enumeration remain implementation
work. The ordinary engine terminal state still requires source-volume absence;
recovery must integrate with that lifecycle before normal enrollment is enabled.

### Recovery reservation transport and act2 merge (candidate, 2026-10-04)

Bosn candidate `542d7a5` adds `DockerActBackend::reserve_tool_recovery`.
It revalidates frozen recovery intent against the containing engine record,
requires the producer cache mount, formats whole-second RFC3339 pin timestamps,
and writes a bounded immutable request into private engine work storage.
Existing request files must be regular non-symlinks with the exact frozen bytes;
new files use no-clobber creation. The command calls the verified installed act
binary with the frozen lower payload bound and explicit mutation approval.

A bounded strict typed receipt must match schema, store and every pin field,
report publication, and have no partial/error/pending-stage state. Command
failure or rejected receipt requires reconciliation and never establishes pin
absence or permission to stop source writers. Tests reject uncertain publication
and changed-generation receipts. Focused receipt validation and service
all-target Clippy with warnings denied passed against the final source:
`.git/retention-bosn-tool-recovery-transport-final-tests.log`.
The prior durable intent slice received the sole primary review with no findings.
The transport slice is awaiting review and is not called by normal planning;
reservation acknowledgement persistence and later source lifetime phases remain
required before activation. No source-stop or removal authority is added here.

All act2 PR #50 checks passed on reviewed head `802f5ab` and the PR merged as
`6c7ae11a0917c24e88035764c4de3f097dcd6c8f`. Existing full checks were dispatched
on that exact default-branch commit as run `37241691702`:
https://github.com/zackees/act2/actions/runs/37241691702
The run is in progress. The candidate is not released or pinned by Bosn yet;
exact default-branch CI and release artifact verification remain mandatory.

### Durable reservation acknowledgement (candidate, 2026-10-04)

Bosn candidate `b82550b` persists a separate reservation acknowledgement timestamp
against the complete frozen intent and exact unexecuted engine claim. The trusted
runtime may supply this only after the strict non-partial act2 receipt check.
Expired acknowledgements, wrong intent/claim and invalid transition times refuse;
identical acknowledged retries avoid another event. Snapshot decoding validates
acknowledgement presence, finite time and its relation to intent and transition.
Focused registry recovery tests pass, including persistence and expiry refusal:
`.git/retention-bosn-tool-recovery-ack-tests.log` (0.12-second test execution).

The prior transport review found no blocking issue, but noted command
substitution strips trailing newlines. Existing request validation now also
requires the exact encoded byte count, so added trailing newlines cannot pass.
That transport adjustment still needs its final focused validation. Neither the
persistence nor transport is called by normal lifecycle planning yet. Source
stop, source-volume retention and completed publication/release transitions
remain implementation work. Full act2 exact-merge CI remains in progress.

### Trusted actor reservation orchestration (candidate, 2026-10-04)

The backend now exposes an ordered `reserve_claimed_tool_recovery` seam:
commit exact frozen intent through the trusted registry actor, read its committed
snapshot, confirm claim continuity, execute the strict act2 reservation transport,
then commit acknowledgement through that same actor. Lost command/commit replies
require reconciliation with the original intent and timestamps. This is a
trusted daemon operation, with no new client/protobuf authority. Recovery
admission closes the actor's startup-interruption window even if refused; a
focused actor test verifies that the live claim cannot subsequently be withdrawn
through startup recovery.

All four actor tests passed (0.38-second test execution) and service all-target
Clippy with warnings denied passed after moving the implementation before its
test module. Evidence: `.git/retention-bosn-tool-recovery-actor-final.log` and
`.git/retention-bosn-tool-recovery-actor-clippy-green.log`. The earlier lint failure
is retained in the actor wiring/final logs. The final request-length adjustment
also passed focused receipt validation and service Clippy in
`.git/retention-bosn-tool-recovery-ack-transport-final.log`.

These tests cover actor authority and compilation, not an end-to-end call through
this new orchestration against a live engine. Normal runtime enrollment remains
unactivated. Source-stop proof, source-volume preservation, publication/release
phases and bounded recovery enumeration are still required. Act2 exact default
branch full CI run `37241691702` remains live; it has not been restarted.

### Durable source-stop phase (candidate, 2026-10-04)

The engine snapshot now records source stop separately from terminal volume
removal. The trusted source-stop receipt binds exact engine name/ID and retained
source-volume identity, requires cleanup_requested state and an acknowledged,
unexpired lower reservation, and persists a monotonic timestamp. Decoder checks
refuse absent acknowledgement and inconsistent state/time. Wrong engine identity
and a receipt before cleanup refuse without advancing recovery state. A client
exit or failed runtime probe cannot establish this receipt.

Two focused recovery tests and registry all-target Clippy with warnings denied
passed: `.git/retention-bosn-tool-source-stop-green.log`. The initial combined
test exceeded the 100-line function lint; it was split by intent and source-stop
responsibility with a shared fixture. No assertion or limit was relaxed.
Runtime Docker probes and source-volume preservation are not wired yet; the
ordinary terminal path still requires volume absence. This persistence phase
alone gives no helper publication or storage-removal authority.

The sole primary reviewer found no blocking issues in the acknowledgement and
actor reservation slices. Exact default-branch act2 CI run `37241691702` now
completed successfully on `6c7ae11a0917c24e88035764c4de3f097dcd6c8f`.
Release preparation can proceed; no candidate release or Bosn pin is claimed yet.

After clean exact-commit CI and duplicate tag/release absence checks, annotated
tag `v0.2.89-act2.10` was pushed. Tag object
`fa12b182110e83ce8f01d9ae2b63d8948e6fd7a6` peels to tested default-branch commit
`6c7ae11a0917c24e88035764c4de3f097dcd6c8f`. The existing Go tag release workflow
is in progress as run `37242493394`:
https://github.com/zackees/act2/actions/runs/37242493394
Release completion, independent archive/executable digests, architecture and
embedded-version verification remain pending. Bosn's shipped pin is unchanged.

### act2.10 release verification and Bosn pin rollout (2026-10-04)

Act2 release run `37242493394` completed **successfully** on exact default-branch
commit `6c7ae11a0917c24e88035764c4de3f097dcd6c8f`; the release is no longer a
draft. All 11 archives plus `checksums.txt` were downloaded and verified
independently:

- every archive matches its release `checksums.txt` entry **and** the GitHub
  asset digest (the manifest and the API digests are byte-identical);
- every archive's GitHub-reported size matches the downloaded size;
- executable architecture confirmed per archive via `llvm-readobj`: Mach-O
  `arm64`/`x86-64`; ELF `EM_AARCH64`, `EM_ARM` (armv6 and armv7), `EM_386`,
  `EM_RISCV`, `EM_X86_64`; COFF `IMAGE_FILE_MACHINE_ARM64`, `_I386`, `_AMD64`;
  all of file type executable;
- every binary embeds the literal `0.2.89-act2.10`;
- the Linux x86_64 binary **executed**: `act --version` reports
  `0.2.89-act2.10`, and `cache tool-recovery reserve|release` expose the typed
  `--record` / `--apply` / `--max-bytes` surface this recovery work depends on.

Bosn's pin moved to `v0.2.89-act2.10`
(`crates/bosn-core/src/act.rs`, `crates/bosn-service/src/ci/pins.rs`) with the
Linux x86_64 archive digest `2e6ed85cd71f17d8f27aa9b66a7c5d338c9927b14b884cf8eeed489c7e707df8`
and extracted executable digest
`4ac7dd7f5660daae13b53f06f12dc3c27e6cff77ede042925078d9c4a69f6e65`, both
measured from the verified release rather than transcribed.

### Runtime source-stop probe and source-volume retention (2026-10-04)

The source-stop phase is no longer a durable field only — the trusted runtime
now constructs the proof from real Docker observations
(`crates/bosn-service/src/act_engine/source_stop.rs`):

- `stop_source_writers` runs the retained-volume half of the phase. It
  `docker volume inspect`s the **frozen** source volume name (never a name
  Docker volunteers) and requires exactly one volume whose driver/scope are
  `local`, whose options are empty, and whose ownership labels are exactly this
  daemon's for this intent. Anything ambiguous, unparsable, renamed, relabelled,
  networked or mounted is a refusal.
- The typed `ActToolSourceStopProof` is then built only from frozen identity —
  engine name from the intent, engine ID from the record, and the frozen source
  volume — and both must agree with the recovery intent. An unresolved or
  different engine ID refuses.
- Engine absence is the other half: startup recovery already proves it with
  two independent exact listings before this probe runs, and the ordinary
  `remove_owned_engine` path reaches the probe only after its own absence
  checks.
- A record with a frozen intent but no acknowledged reservation refuses. An
  ordinary engine with no recovery intent skips the phase entirely.
- The proof is idempotent: an already-recorded stop re-verifies ownership and
  returns without rewriting the timestamp.

**Preservation guard.** `remove_storage_volume` now refuses while a record
holds a **live** recovery reference, so private disk and terminal metadata are
not finalized ahead of publication. Retention is bounded by the reference's own
finite lifetime rather than by a release transition that does not exist yet: an
expired reference is an abandoned recovery, so the volume is reclaimable
instead of wedging cleanup and leaking the disk forever. The guard is
fail-closed for the whole (at most 24 hour) window in which a helper could
still publish through that reference. The underlying destructive primitive was
made module-private (`remove_private_storage`), so the guarded entry is the only
crate-reachable path and the guard cannot be bypassed by adding a caller. This
is a structural guarantee rather than a unit test, because an
`ActEngineRecord` cannot be constructed outside the registry; the registry-side
receipt boundaries are already covered by the existing focused recovery tests.
The liveness/expiry predicate itself is a pure function and is unit-tested.

Pre-push review of the branch raised a blocking defect that was fixed here: the
first guard keyed only on the presence of a recovery intent, which is never
cleared, so one recovery run would have wedged its record in `CleanupRequired`
forever — leaking a volume and a registry row and re-failing the same
retirement on every later cleanup pass. Keying on expiry gives that path a
bounded escape. The review also flagged the retained-source predicate as a
near-verbatim copy of the private-storage predicate; both now share one
`verify_owned_local_volume` predicate that takes the expected name and labels
from the caller, so a Docker-reported name can never substitute a volume.

Workspace Clippy with warnings denied and `--all-targets` passed, and `./lint`
(including file length and include-base) passed. Host test execution is
refused by `ci/test_guard.sh` by design, so the new unit tests compile here and
execute in the local gate's isolated lane. Normal runtime enrollment remains
legacy and unactivated; publication, merge-into-generation, reference release,
source-volume removal and bounded recovery enumeration remain open.

### Gate failure triage: source-text ordering test vs. native overlay (2026-10-04)

The local gate's `tests` lane failed with exactly one failure,
`tests/test_runner_tools.py::RunnerToolsTests::test_tools_are_prepared_after_the_tool_cache_is_seeded`,
at `ValueError: substring not found` (262 passed, 10 skipped). The retained act
log for the failing run is under the gate state directory's `ci/runs/`.

The test asserts on **source text**, and the native overlay slice replaced the
seed call site: `engine.rs` now runs
`Self::exec(engine, &prepare_toolcache_script(generation)?)`, which returns the
legacy seed with no frozen generation and the native overlay with one. The
invariant the test guards — the tool cache is seeded before runner stock tools
install into it — is unchanged, and the ordering assertion now names the new
call site. The test was updated, not weakened: it still pins both call sites
and their relative order.

Two other tests fail when the workspace suite is run directly on the developer
host (`concurrent_engines_with_equal_pids_publish_through_distinct_stages` and
`daemon_shutdown_does_not_wait_for_stalled_cache_discovery_client`, the latter
measuring ~30s against a 5s budget). Both fail identically on unmodified
`origin/main` (`420c1ddb`), and main's GitHub CI at that same SHA is green, so
they are host-environment timing artifacts rather than branch defects. They are
retained here rather than dismissed, because the local gate is the stricter
environment and a host run is not the gate.


### CI attestation pilot: published exact cleanup provider (#362)

Act2 `v0.2.89-act2.14` is published from exact default-branch candidate
`6ec60ce6f3de3fa1fe827d2ccf5b00b1b20294db`. All five full qualification jobs
passed in [run 37627801434](https://github.com/zackees/act2/actions/runs/37627801434);
existing tag-triggered [publication 37629524141](https://github.com/zackees/act2/actions/runs/37629524141)
also passed. The consumed Linux x86_64 archive matches published checksums:
`8549dea52bccef8784754ab8f1ca3ef917ecae710b0adbe0b71d4aa5e58a27aa`.
Its extracted executable SHA-256 is
`ec6aeb07022ad055e852066e4591e8fd399750a6426f51a5ce8987831c43cfcf`.
The public executable reports that version and all four required contracts:
qualified jobs, step results, selected outputs and `cache-exact-delete-v1`.

The server implements exact local deletion through its existing durable
journal, preserving other keys and incomplete entries and refusing busy
transfers. Shared ci-lint owns backend selection and receipt validation;
[ci.yml PR #370](https://github.com/zackees/ci.yml/pull/370) uses that service
under ACT without a GitHub credential fallback. Actual source conformance
reserved/uploaded/committed two entries, deleted exactly one, preserved the
other and repeated with zero deleted count/bytes. This is cleanup transport
proof, not compiler warmth or test-execution proof.

The first exact-main qualification caught a legacy action-cache read/write
race, repaired in [act2 PR #56](https://github.com/zackees/act2/pull/56). Its
process-local Git gate protects manifest reads, copies and deferred Docker
build contexts and rejects a changed manifest revision. It does not establish
cross-process action locking or aggregate host CPU admission.

Released Bosn 0.1.17 pins the verified public artifact and requires the cleanup
capability before workflow execution. The focused isolated capability regression
demonstrated RED to GREEN. Its exact-main full qualification, pretag dry run and
publication completed; [shared pilot evidence](https://github.com/zackees/ci.yml/issues/362)
records all four verified public wheels. Template's complete local replay and
unchanged-tree reuse passed through this provider; its source-only hosted skip
proof remains queued.

The shared delta transport also now uses content identities
([ci.yml PR #372](https://github.com/zackees/ci.yml/pull/372)), preserving
same-size generation-pointer changes and invalidating different content bases.
It is restored privately before compilation; this does not establish safe
concurrent import into a shared writable compiler store. Actual fresh-engine
compiler durability, residual misses and hashing cost remain pilot measurements.


### Published artifact-signature provider (act2.15; Bosn rollout pending)

[Act2 PR #57](https://github.com/zackees/act2/pull/57) URL-encodes artifact
signatures so the stock upload client's query handling preserves base64 padding.
Strict signature verification is unchanged. Exact-main
[qualification 37648485379](https://github.com/zackees/act2/actions/runs/37648485379)
passed all five jobs on `73c1eea4d868bb38a84c3050db395f6f21f4c6e7`;
[publication 37650187671](https://github.com/zackees/act2/actions/runs/37650187671)
then published `v0.2.89-act2.15`. The downloaded Linux x86_64 archive matches
its published checksum `e21535decdac2c6ed8429b8662825b4c1fca606301e0da54e37b180ad2517e6d`;
the executable hashes to `b4be8d7ef98729ad16a9a6ddba331f1d2b0feb8abd52eb9e5f3a93155fb4f1df`.
The public binary reports act2.15 and all four required capability contracts.

The Bosn 0.1.18 candidate pins those verified bytes. It also contains merged
[PR #542](https://github.com/zackees/bosn/pull/542), retaining available parent
history so ordinary local push lockfile comparisons can read `HEAD^`. That
change's full source gate passed in 608s; unchanged reuse took 0.999s and kept
all seven trailers. Exact publication and hosted run 37650279564 passed with
attested Rust and policy skips. This proves the snapshot change through the
previous public provider, not the new provider's execution.

New candidate qualification, exact-main full CI, pretag verification, publication
and actual local writer replay through act2.15 remain pending. Compiler payload
durability and residual misses remain separate unproven requirements.

### Serve-mode provider (act2.17; #547 plan step 3)

[Act2 PR #63](https://github.com/zackees/act2/pull/63) adds `act serve`
([act2#62](https://github.com/zackees/act2/issues/62)): run admission into
slots and port pairs, each run's cgroup v2 scope, labelled network and work
tree, act executed in the run's cgroup with a streamed exit, cancel, and a
close that removes and proves gone every object of the run.
[PR #65](https://github.com/zackees/act2/pull/65) adds the read-only
`act doctor` ([act2#61](https://github.com/zackees/act2/issues/61)), and
[PR #66](https://github.com/zackees/act2/pull/66) keeps a gosec waiver that
CI's golangci-lint v2.11 needs. Full CI on the first candidate
(`c9a9f73a`, [run 38067466017](https://github.com/zackees/act2/actions/runs/38067466017))
failed lint, so no tag was made from it. Full CI then passed all five jobs on
exact master `8998d436146f4ec6a9f76773c91b0c53e70187a0`
([run 38069050519](https://github.com/zackees/act2/actions/runs/38069050519)).
From a clean checkout of master at that SHA, with neither the tag nor the
release present, `v0.2.89-act2.17` was pushed, and the existing tag-triggered
[release run 38069860794](https://github.com/zackees/act2/actions/runs/38069860794)
published it. The downloaded Linux x86_64 archive matches its published
checksum `9adc44420ec4eac8800925c7194c52d2f4c3923f510ef60ef2885197f572aadd`;
the executable hashes to
`29858fabb6ae3824552486ec3df31673c6de0277c366d30260e628747bf6ef18`, reports
`act version 0.2.89-act2.17` and the capabilities
`qualified-job-identity-v1`, `step-stage-result-v1`, `selected-job-outputs-v1`,
`cache-exact-delete-v1` and `serve-v1`.

Bosn pins those bytes and forwards each scoped run to `act serve`. Live
check (2026-10-10): a daemon built from the branch, on its own state
directory, against a throwaway Docker 29.7.2 engine (`memory_gib = 4`,
`idle_retire_secs = 60`). Runs A and B shared one engine (slots 0 and 1, ports
40000 and 40002). A's `head -c 3500m` was `Killed` (`exitcode '137'`) while B
succeeded; each job's `docker ps` showed only its own container. Cancelling
run C mid-step removed its container, network, volumes, cgroup and work tree
(all counts 0 twenty seconds later) while a concurrent B succeeded. Every run
logged `run scope cleaned up`, serve's status showed no runs afterwards, and
the idle engine was retired, leaving only `bosn-ci-cache-v1`. Everything was
removed afterwards.

The per-run Docker proxy, the runner-image load and the tool-cache seed and
save moved to act2 in act2.18 (below), and step 4 removed them from Bosn.

### Serve owns the proxy, runner image and tool cache (act2.18; #547 step 4)

[Act2 PR #68](https://github.com/zackees/act2/pull/68)
([act2#67](https://github.com/zackees/act2/issues/67)) moves each run's Docker
proxy, `act serve prepare` (the runner-image load and proof, plus the tool-cache
seed or overlay), the tool-cache save after a run, and the shared cache budget
into `act serve`. Full CI passed on exact master
`a28192deeb129713ef0a57724aa83369d3cdc6ba`
([run 38076424358](https://github.com/zackees/act2/actions/runs/38076424358)).
From a clean detached checkout of master at that SHA, with neither the tag nor
the release present, the annotated tag `v0.2.89-act2.18` was pushed, and the
tag-triggered [release run 38083462686](https://github.com/zackees/act2/actions/runs/38083462686)
published it. The downloaded Linux x86_64 archive matches its published
checksum `5ebd6231663bc3b2c54d9777cd26d9333575325ebc28a3bd8f9f0ef7ada13ed7`;
the executable hashes to
`a64a5e95cd570d165cecb83097c7b57f963e9f00b2e4a61c2bbc3574606cdc41` and reports
`act version 0.2.89-act2.18`. Bosn pins those bytes.

### Expired source recovery retirement (#508, 2026-10-21)

Reconciliation against main `4bf62b37` found an expiry mismatch in the native
recovery foundation from #510. `remove_storage_volume` permits ordinary cleanup
after the finite reference lifetime, but `stop_source_writers` ran first and
attempted to record a source-stop receipt that the registry rejects after
expiry. An unacknowledged reservation also refused indefinitely, and a retry
after successful deletion with a lost Docker reply tried to inspect the already
absent source. The pure lifetime test did not exercise these paths.

The regression now calls the actual `retire_engine` path with a real SQLite
registry/actor and a synthetic Docker transport. On unchanged implementation it
failed with `tool recovery source stop proof`; with the fix all seven focused
source-stop tests pass, including expired acknowledged/unacknowledged references,
an already absent source, and retry after a lost removal reply. Negative cases
retain a live reference and refuse foreign labels, failed inspection, failed
engine absence and failed volume listing. Terminal receipts are checked after
reopening the registry.

An expired intent now skips only the native source-stop receipt. It provides no
recovery/publication authority. The caller still proves engine absence, and
ordinary storage cleanup checks the exact named local volume's ownership,
performs non-forced removal and proves absence before finalizing. A live
reference still requires acknowledgement and retained-source inspection and
prevents storage removal. This correction makes the foundation's existing
bounded abandonment contract reachable; normal runtime enrollment is still
legacy, and production publication, recovery and shared-class expiry remain
open. Focused synthetic transport tests do not establish a live Docker or full
workflow recovery result.

### Durable publication and reference release (#508, 2026-10-21)

The registry now has separate trusted transitions for completed generation
publication and exact lower-reference release. A publication proof binds the
entire frozen recovery intent and the resulting canonical generation ID; it
requires acknowledged protection and a recorded source stop while the reference
is live. Publication is committed before release can be recorded. Both
transitions preserve the first receipt on identical retries and refuse changed
identities, generations and invalid or regressing timestamps. A completed
publication can be reconciled after expiry without republishing it.

The runtime must stop publication retries once that completion is committed.
An exact, non-partial act2 release receipt must precede the trusted release
transition. Durable release then permits ordinary guarded source removal before
expiry; publication alone does not. The registry also refuses finalization
while the source is retained, making this a persistence invariant shared with
the runtime guard. Unknown clock values retain a live source. Optional fields
preserve reads of historical records, and snapshot validation rejects malformed
completion/release ordering. These are internal persistence commands, with no
new client RPC authority.

The focused registry regression first demonstrated that main allowed terminal
cleanup while a live recovery reference remained. The new registry lifecycle
tests cover transaction rollback, reopening SQLite, receipt ordering, changed
identities, idempotent retries and invalid snapshots. The runtime retirement
fixture also distinguishes published-but-unreleased storage from released
storage, including retry after a lost early-removal reply. These fixtures
supply trusted publication/release receipts; they do not perform act2
publication or prove live Docker recovery. Normal enrollment, helper transport,
publication and release orchestration remain unactivated until those complete
paths are implemented and verified.


### Act2 recovery publication contract (candidate, #508, 2026-10-21)

[Act2 #71](https://github.com/zackees/act2/pull/71) merged at
`9dc019f79de0d8fd4186fcfe33a2be2aecbd747c` after all PR checks passed.
Preparation now persists and verifies the complete generation/store/recipe
binding across serve restarts, and health uses that same bounded metadata.
Historical seeded preparation stays readable; a generation-only stamp lacks
producer evidence and refuses. Linux CI first exposed a separate ownership
race: recursive runner chown encountered a Python temporary file withdrawn by
another job. The correction skips already-owned entries and tolerates only a
path proven absent after failure; existing files and dangling links still
refuse. No CI check was removed or weakened.

[Act2 #72](https://github.com/zackees/act2/pull/72), rebased onto that master,
excludes native overlays and invalid preparation metadata from the legacy
best-effort saver. A native close returns an explicit unsaved recovery note,
retaining source data for post-stop publication. The focused regression first
reproduced legacy-store writes and false saved-success in all three native,
unverifiable and corrupt-readiness cases. The corrected full serve suite and
pinned lint passed. This candidate is not a native save implementation.

[Act2 #73](https://github.com/zackees/act2/pull/73), candidate
`588c6e563d50f3c12d33904d54a34c79203e079c`, adds
`cache tool-recovery publish` and capability `tool-recovery-publication-v1`.
The coordinator must supply an exclusively held, quiescent stopped-source merged
overlay view. Neither a pin nor the explicit CLI confirmation proves Docker
ownership, writer absence or source-volume lifetime exclusion. The command does
not mount/recover an overlay or supervise a helper.

New publication requires the exact live pin and retains the original lower
reader. Completed install discovery bounds namespace entries, install count,
aggregate payload and metadata. Only installs changed from the original lower
merge into the latest verified warm selection, preserving another engine's
newer unchanged install. Incomplete installs stay out; missing/corrupt warm
state refuses instead of becoming a cold cache. Snapshot inspection is reused
for publication without a second planning hash; required source and copied-data
verification remain.

Selection and completion share one fsynced atomic pointer. Before selecting a
successor, later writers durably archive that exact completion. An identical
lost-reply retry returns the receipt without source access or another
publication, preserving even a later same-path update. Publication never
releases the pin: Bosn must commit completion, reconcile exact reference
release, then perform ordinary guarded source removal. Archived receipt history
is bounded to 10,000 canonical records, expires at most 32 past-lifetime receipts
per archival pass, and refuses live overflow. Previous canonical selection
readers refuse an extended pointer rather than overwrite an unarchived receipt.

The actual CLI regression first failed on the missing publication operation,
then proved changed-versus-unchanged installs, incomplete-install exclusion and
lost-reply reconciliation. A real helper subprocess was killed after durable
publication while withholding its reply; retry after source disappearance and
a later selection recovered the same receipt. Focused tests also cover sync
uncertainty, exact-intent conflicts, absent protection, cancellation, corrupt
state, completion conventions and a full 10,000-record overflow/expiry fixture.
Full artifact-cache and race suites passed. Their first local invocation could
not execute an existing privilege-drop child through Go's private build
directory; rebuilding the same test binary into accessible `/tmp` let that
unchanged suite execute. Pinned golangci-lint v2.11.4 and the sole primary review
are clean. Fresh remote checks on each rebased candidate remain required.

These are provider candidates, not a verified dependency release or Bosn pin
rollout. Normal Bosn enrollment and routing remain legacy. Actual helper mount
recovery and source lifetime exclusion, restart orchestration, bounded runtime
scheduling, shared-class accounting/expiry and full workflow/7.7GiB acceptance
remain unfinished. Copy/publication currently holds the native catalog writer;
production-scale performance has not been established. Do not enable the native
route or close #508 based on these helper tests.


### Stopped-overlay helper and durable source exclusion (#508, 2026-10-21)

Act2 [#73](https://github.com/zackees/act2/pull/73) is merged. Its overlay
consumer [#74](https://github.com/zackees/act2/pull/74) is also merged, at
`11382a48807393c20dc33e4c9093311eb41057f1`. All six PR checks passed on
`3e344e5b8868c5dedc6135c42898485bb4e2ed53`. Full default-branch CI
[39683097915](https://github.com/zackees/act2/actions/runs/39683097915)
is running on the exact merge SHA. No new act2 release or Bosn pin update is
established by this section.

The helper opens a read-only metacopy overlay only after acquiring its original
verified lower reader. Exact bounded mountinfo checks prove the producer's
lower, upper, work directory and mount flags. The producer recipe remains
byte-identical. The sole primary reviewer found a failed-unmount lifetime gap:
the reader originally closed despite a surviving view. Both successful-open
and failed-open regressions reproduced retirement after withdrawn protection
and forced GC. The corrected process-owned slot strongly retains the reader
and cleanup handle until cleanup succeeds, and admits at most one view.
Retries must finish pending cleanup before acknowledging a receipt; concurrent
calls refuse immediately. Focused race tests and review pass.

The recompiled privileged Linux fixture actually passed (2.83 s): a random
16 MiB metadata-only install recovered content and mode, preserved a newer
Node install, excluded an incomplete install and left its lower unchanged.
It also proved source-free completion retry and separate reference release.
Its exact container and volume were removed and absence verified. This fixture
supplies ownership/quiescence manually; it does not prove Bosn orchestration.
Full artifact-cache race and serve suites, focused CLI tests and pinned
act2 lint passed. Local evidence is under `/tmp/bosn-693/`, including
`overlay-helper-live-final.json` and `overlay-artifactcache-race-final.log`.

The next Bosn candidate adds a durable `ToolRecoveryV1` helper role binding
its run and complete frozen intent. Admission requires the exact stopped,
reserved, still-live source and pinned engine image; another unresolved create
for that source refuses. Pending creates and acknowledged helpers survive
registry reopen. Their lifetime excludes both private volume removal and
terminal engine finalization even after the lower reference expires. The
existing late-create absence contract remains necessary before an uncertain
create can be forgotten. Generic one-volume helper identity refuses this role
until dedicated two-volume supervision is implemented.

Tests first failed because the role was absent, then caught an incorrect JSON
query prefix that failed duplicate exclusion. A runtime regression with the
new removal guard temporarily absent deleted the expired source despite a
pending helper; restoring the guard preserves it without attempting deletion.
Focused registry recovery and both runtime retirement tests pass. Initial
Clippy rejected an expanded test helper over 100 lines; its setup was split
by responsibility. The primary reviewer then found a check/removal race: a delayed pre-expiry
helper could be admitted after an absence lookup. Its deterministic regression
failed before a durable disposal claim replaced that loose read. The claim
checks helper absence atomically, persists a per-source fence, and excludes
later admissions regardless of their timestamp. The opposite ordering refuses
disposal while a helper is pending. Final all-target Clippy and primary re-review passed. Both cleanup paths
carry the committed disposal timestamp forward into finalization. The full
exact-source Bosn local gate remains required before push.
These are candidate lifecycle changes, not active native routing. Dedicated
Docker helper supervision, source-volume identity, receipt transport,
production enrollment, dependency rollout and the original broad acceptance
remain unfinished.


The first full local gate on `c8f6d116bbd285734ca5053e9ba938c21d5be206`
failed after 504 s: the tests lane passed, but Rust stopped at the existing
slow-removal actor fixture before its deletion step. The fixture estimated
transition time as its pre-create time plus one second; concurrent setup
advanced the committed creation time beyond that estimate. Seeding a stale
caller time reproduced the same `act transition time` refusal without adding
sleeps. The fixture now reads the committed post-create record and uses its
time as a floor. Production monotonic checks, the slow deletion duration and
terminal-state assertions remain unchanged. Primary review found no correctness
issue in this correction; focused validation and the replacement full gate
remain required. Failure evidence is retained under `/tmp/bosn-693/` in
`recovery-helper-full-gate-failed.log`, `recovery-helper-gate-rust-full.log`
and `recovery-helper-stale-fixture-red.log`.

Act2's exact-merge full CI completed successfully on
`11382a48807393c20dc33e4c9093311eb41057f1`. The clean default-branch checkout
and absence of the next tag/release were verified before creating
`v0.2.89-act2.20` at that SHA. Existing tag-triggered release run
[39684898381](https://github.com/zackees/act2/actions/runs/39684898381)
is building its assets. Tag publication is not artifact verification: all
11 archives still need independent digest, size, checksum, executable-header
and version checks before Bosn's pin can change.


Release [act2.20](https://github.com/zackees/act2/releases/tag/v0.2.89-act2.20)
completed successfully. All 11 archives matched independent GitHub asset
SHA256/size metadata and the release checksum file; each extracted executable
passed architecture/header and embedded-version checks. Linux x86_64 executed
`--version` and `--ci-capabilities` in isolated containers with no network,
read-only root and all capabilities dropped, reporting the exact release and
both recovery capability contracts. The initial smoke fixture failed because
the staged binary was owner-only; mode 0755 fixed container execution without
changing its bytes or adding capabilities. Failed and successful evidence is
retained. Each smoke container was proven absent afterward.

Verified Linux archive SHA256:
`659db39ad669f3a27773272ea0f19fff1ba54fcc1f582f75528c0e21e3fef69b`;
executable SHA256:
`6ca926480c332210b6c59159b3f9384df0c67efc2e4fd990af31760e2b6f30fe`.
The candidate Bosn pin now names these values and act2.20; its full exact-source
gate and PR rollout remain required. Verification records are
`/tmp/bosn-693/act2-20-{pretag,metadata,verification,runtime-smoke}.json`.
Cross-platform executable runtime smoke was not performed locally; their
release binaries received static verification and full act2 CI passed on the
exact source commit. This release/pin does not activate native Bosn recovery
or satisfy the original full-workflow and storage acceptance.

### Recovery helper inspection and restart cleanup (candidate, #508, 2026-10-21)

Bosn #697 merged at `bc6871d0ab0bbef73312fec184c098bb5525d25d`, after its
replacement full gate passed in 213 s (Rust 210 s). Exact stamped head
`a7dea745c465051961225faa913ba846879cda5b` was verified before push. Remote
checks finished with nine successes and five configured skips; local Rust
coverage was carried by its verified attestation. The original checkout was
rebased to main and its task branch and watcher file removed.

The actual released act2.20 CLI also passed a six-phase controlled fixture:
prepare, recover, data/mode verification, source-free identical receipt retry,
reference release and old-generation retirement. A random 16 MiB metadata-only
payload, its recovered mode/settings, a newer Node update, incomplete-install
exclusion and the unchanged original lower were checked. Both labelled volumes
and all fixture containers were removed and absence verified. Evidence is
`/tmp/bosn-693/released-overlay-proof.{py,json,log}`. This supplies source
ownership/quiescence manually and does not prove ordinary Bosn orchestration.

The next candidate replaces the helper supervisor's maintenance boolean with
explicit measurement, maintenance and recovery profiles. A recovery intent
may carry `command_sha256`, the SHA256 of the canonical JSON tuple of its
entrypoint and argument vectors. An omitted historical binding remains
readable, but never authorizes adopting a process. Aged pending-name absence
and acknowledged immutable-ID absence are proved before requiring an adoptable
command, so those historical intents can be retired safely without stopping
an unknown process. Recent uncertain creates remain pending.

Recovery inspection requires its own nonce/ownership scope and exactly two
local named writable volume mounts: its frozen shared cache at `/bosn/cache`
and source disk at `/var/lib/docker`. The merged overlay view remains read-only;
the source mount permits the kernel's overlay work-directory operations. The
profile also requires a read-only root, no network, private PID namespace,
explicitly disabled init, automatic removal, no restart policy, 512 MiB memory,
two CPUs, 128 pids, the shared low CPU weight and executable 64 MiB `/tmp`.
The inspected entrypoint/arguments must match their durable digest. Inspection
input is bounded to 64 KiB, including for the existing helper roles.

The new identity regression first failed because the command binding was not
supported. Mutation cases now refuse foreign/missing/extra/swapped mounts,
changed commands, nonce, resource and isolation settings. Eleven restart cases
exercise pending and acknowledged helpers, lost removal replies, wrong source
or command, unreadable Docker state, recent/aged uncertain creates, known
absent IDs and active in-process exclusion. The first restart-fixture attempt
correctly refused its mismatched cache-volume identity; the fixture was fixed
to use the frozen source record rather than weakening validation.

A real Docker create/inspect probe matched the typed profile, including
`--init=false` and Docker's actual resource/mount representation. It was never
started; its exact container and both labelled volumes were removed without
forcing volume deletion and their absence was verified. The opt-in Rust test
was actually executed against the captured inspection and passed. Ordinary CI
leaves that hardware-fixture test explicitly ignored. Probe evidence is under
`/tmp/bosn-693/recovery-profile-*`.

Dedicated helper program creation, source ownership/liveness probes around
startup, receipt transport and durable publication/reference-release ordering
are still to be wired. Native routing remains Legacy. Focused service/registry suites, all-target
Clippy and the sole primary review passed. The full exact-source gate remains
required before pushing this candidate. None of
these primitive/helper proofs closes the original broad #508 acceptance.

### Dedicated PID1 publication transport (candidate, #508, 2026-10-21)

Bosn #698 merged the preceding inspection/restart-cleanup work at
`06125d9c` after its full local gate passed in 513 seconds (Rust 513s,
Linux 477s), with stamped `0f3c6db1414f` verified before push. The original
checkout returned to `main` and rebased after merge. This candidate continues
in that original checkout; no additional Bosn worktree was created.

`DockerActBackend::recover_tool_with_helper` now reads the durable record and
creates a dedicated two-volume recovery helper. It checks the absent source
engine by both frozen ID and name and verifies the private source's exact
ownership through the same predicate terminal storage cleanup uses. It checks
both source and shared-cache ownership again after Docker create, since a
named-volume mount could otherwise recreate a deleted volume. The helper
journal commits before Docker create and excludes concurrent source disposal.
The canonical helper name is independently inspected against the full frozen
profile before accepting its immutable ID; a wrong or lost create reply never
registers an arbitrary ID or starts recovery. Recovery and reaping share the
same active-helper guard and durable journal.

Act is extracted offline from its pinned archive under a shared archive lock;
both archive and executable digests are checked. The extracted executable and
control record live in `/tmp/bosn-recovery`, away from the retained private
source disk. The descriptor is closed before the shell execs act as PID1,
with `--init=false`; there is no keepalive supervisor or `docker exec` recovery
process. The helper has no network and no download fallback. It uses the
existing strict privileged recovery profile, a read-only root, a bounded
executable tmpfs, low CPU shares and fixed CPU/memory/PID limits.

The typed, closed publication receipt must match the complete frozen pin and
store, carry a canonical generation ID, and report complete/non-partial work
without errors or pending stages. Its timestamp must be within the original
finite reference lifetime, no earlier than the durable source-stop state and
no later than observation. The helper must then be explicitly absent before
publication is committed. The journal finish follows that commit, preserving
the disposal fence during an uncertain database acknowledgement. A durably
completed record never starts publication again. Neither this method nor its
cleanup releases the recovery reference or removes either named volume.

Evidence and failures:

- The focused receipt test first failed because the parser did not exist, then
  passed. Fault-injected Docker transport exercised the actual registry actor
  and journal across successful publication, partial/mismatched/trailing
  receipts, failed start, changed source ownership, wrong/lost create replies,
  foreign commands, cleanup denial/lost removal replies and a present source
  engine. These cases passed before expanding the matrix to 16 cases, including
  mixed source/helper/removal errors and oversized receipt output. The mixed
  source-error case first failed: a not-found line alongside permission denial
  was incorrectly accepted. Shared absence/removal checks now require the
  existing stricter `CommandResult::reports_missing` verdict, preserving the
  journal on mixed diagnostics. The final live run also passed after this fix.
- The first actual Docker transport run failed: released act2.20 refused Bosn's
  stale `f6f345…` producer recipe pin. Independent rendering of the pinned
  act2 source and the released fixture both yielded
  `fa1d1738cdd1cc9efb8a7ba10c6ca580de21d2e18ea3a75a79492201ab1e9af9`.
  The pin now matches those exact bytes. Unknown historical bindings remain
  refused; no frozen record is rewritten. Normal routing had not activated
  the old binding. Failure evidence and successful cleanup are retained in
  `/tmp/bosn-693/live-publication-proof-recipe-red.json`.
- The replacement actual Docker run passed all four phases: prepare the stopped
  overlay, stage the verified offline archive, execute the real Rust helper
  transport, and independently verify the published generation. The helper
  `2978736e89289f407b7c3302ad097c7adf84121fc2d7751269cd0d54aaa6c389`
  was absent and its journal `removed`; durable completion named generation
  `ce9d9ca17ff7ec8e22d5772ba69d537ff00b4d4266f1501b9bb3e559a71b8307`,
  while reference release and source disposal remained unset. Verification
  checked the recovered payload/mode/settings, preservation of a newer Node
  install, incomplete-install exclusion, an unchanged original lower and
  retained protection. All fixture containers and both volumes were removed
  with ownership checks and explicit absence verification. Evidence:
  `/tmp/bosn-693/live-publication-proof.{py,json}` and
  `live-publication-proof-final.log`. The opt-in Rust test was actually run in
  a separate isolated container; ordinary CI explicitly ignores this fixture.

The live fixture manually supplies source quiescence and reservation setup;
it does not prove ordinary enrollment or retirement orchestration. Native
routing remains Legacy. Reference-release transport, bounded recovery
scheduling, normal lifecycle activation and the original broad storage,
expiry, large/concurrent/corruption and full-workflow acceptance remain open.
This candidate does not close #508. Final focused recovery validation passed
(3 tests plus the separately executed live opt-in); cache helper validation
passed (23 tests, 5 explicit opt-ins ignored). All-target Clippy passed after
extracting one overlong test function without dropping assertions. The sole
primary review found no remaining correctness issues. Full exact-source gate
evidence is still required before pushing and will be recorded on the PR.

### Source-free reference release and recovery scheduling (candidate, #508, 2026-10-21)

Bosn #699 merged the dedicated publication helper at `8438889b` after a
252-second full local gate (Rust 252s, Linux 234s), with stamped
`dd55e6d3f546b168dcff15d58cbffa0bfbc45f1d` independently verified. Remote
run 39693201859 met its required checks (nine successful, five configured
skips). The original checkout returned to `main` and rebased after merge;
this phase continues there without creating a Bosn worktree.

Publication and reference release now share a journaled transport with separate
typed roles and exact command digests. A release requires durable publication
completion. It can reconcile in `CleanupRequired` or `Terminal`, including
after expiry and source disposal, and never inspects or mounts the old engine
or source disk. It mounts only the frozen owned shared cache, extracts the
verified pinned executable offline, and execs the release command as PID1.
The closed bounded receipt must prove absence of the exact frozen reference
and match the complete pin/store. A retry reporting `removed: false` is valid
only when it still proves that absence. Partial, foreign, oversized or trailing
output is refused. Helper absence precedes the durable release acknowledgement;
the journal finish follows it. A completed acknowledgement retries no Docker
operation. An uncertain helper stays journaled for the existing reaper.

The release profile drops all capabilities and remains unprivileged, with the
same read-only root, no network, private PID namespace, disabled init,
automatic removal, no restart, resource limits and low CPU weight. Its tmpfs
map is exact: executable 64 MiB `/tmp` and read-only/noexec 1 MiB
`/var/lib/docker`. The latter overrides the pinned DIND image's declared
VOLUME; without it Docker silently creates an anonymous disk volume. The real
release proof initially failed this exact identity check before startup. The
fix preserves the strict one-named-mount rule. A subsequent create/inspect
probe confirmed Docker's normalized map and absence of anonymous storage.
The test harness also now captures cleanup inspections and removes anonymous
container volumes explicitly. Earlier fixture cleanup claims covered named
resources only; an audit found seven empty anonymous volumes from five
fixture executions (CLI extraction and the two refused release helpers).
The audit correlates their timestamps with those executions and captured
volume events where available, verifies no attached containers and empty
contents, and records each exact removal/absence. Older unrelated anonymous
volumes were preserved. Evidence: `anonymous-proof-volume-audit.json` under
`/tmp/bosn-693`.

An independent supervisor now runs for already-reserved native records, even
when enrollment discovery has no cohorts. Each pass reads at most eight pages
of 64 bounded latest-record snapshots and attempts one external publication or
release, advancing a keyset cursor on success or failure. Expired unpublished
sources are left to ordinary fenced disposal; completed records can release
after their source becomes terminal. Passes have a 900-second deadline, a
10-second interval, cancellation and a bounded 64-entry diagnostic history.
Daemon shutdown or idle exit can interrupt a pass; the journal remains the
restart authority. The SQL query still aggregates historical events to find
the latest record: result bounds do not establish bounded query cost or
bounded history storage. Those original #508 requirements remain open.

Focused evidence includes a missing release parser RED followed by GREEN,
16 publication and 16 release transport cases, strict release-profile
mutations, registry phase/expiry/reopen/duplicate admission cases and worker
expiry/cursor/failure cases. Ordinary CI explicitly ignores the actual Docker
fixture; it must be executed separately. That fixture still manually supplies
reservation and source quiescence, so it does not establish ordinary native
enrollment. Native routing remains Legacy. Normal lifecycle activation,
archive/all-class routing, allocated inode accounting, scoped GC, bounded
history/storage, realistic large/concurrent/corruption cases and unchanged
full workflows on fresh engines remain required. This phase does not close
#508. Final real-proof, review and full-gate evidence follows on the PR.

The final real Docker fixture passed preparation, offline archive staging,
the actual Rust worker transport and independent generation verification.
It used a real 20-second reference lifetime: publication completed while
protected, ordinary production disposal removed the source at expiry and
finalized the record, then the independent worker acknowledged reference
absence after expiry on that terminal record. The release helper
`e8fa9aa818ce25e7b92d74a5a71f21ac305317db04f85323c3425511e7d53fa0`
was absent and journaled `removed`; retrying the acknowledgement made no
Docker contact. Recovered generation
`e0cfd41b2f38837a34c3bcb6522c62017a26540e1a7c331b4f33460403c771e3`
passed the existing payload/mode/newer-install/incomplete-install checks.
The first corrected-profile run had already passed those four phases, then
hit a harness-only cleanup assertion that mistook the intentionally retained
shared build-target volume for an anonymous fixture volume; that audit was
corrected and exact owned cleanup completed before repeating the full proof.
Evidence: `/tmp/bosn-693/live-reference-release-proof.{py,json}` and
`live-reference-release-proof-final.log`. Focused validation passed six
recovery tests and four identity tests (their separate hardware opt-ins were
explicitly ignored), and all-target Clippy passed. The separately executed
live proof is additional evidence, not an ordinary CI pass.

### Warm tool-cache bootstrap dependency (candidate, #508, 2026-10-21)

Act2 [#75](https://github.com/zackees/act2/pull/75) merged at
`e80072a16cf8737c56d62574098a74e249da102e`. The external checkout returned
to clean `master` and rebased after merge; Bosn development continues in its
original checkout. The exact merged SHA passed the full default-branch
workflow-dispatch run [39700626307](https://github.com/zackees/act2/actions/runs/39700626307):
Linux tests, native Windows/macOS tests, lint and snapshot builds all succeeded.

The dependency adds explicit `cache tool-bootstrap`: a caller that has excluded
all legacy source writers can publish all supported completed installs from
the legacy tool-cache tree as the first selected immutable generation. It
plans bounded namespace/payload metadata before initializing a destination,
then uses the existing closed-object publisher and original catalog writer
through generation publication and selection. Missing, empty, oversized,
invalid and aliased sources are refused. Incomplete installs are excluded;
corrupt existing selection or payload is refused rather than replaced. A
valid selected generation is verified and synced on retry without reopening
the old source. A visible selection on a directory-sync failure remains
reported as partial until reconciliation succeeds.

The focused API and CLI regressions first failed because the operation did
not exist. A separate alias regression then caught a linked namespace being
silently omitted; bootstrap now refuses namespace links outside completed
installs, preserving recovery's previous scan behavior. The broader recovery,
generation and snapshot suites passed (artifactcache 63.563s, CLI 7.568s).
Latest bootstrap tests passed after explicit empty-destination coverage;
golangci-lint 2.11.4 reported zero issues. A sole primary review found no
remaining substantive findings. Local Go execution used the existing Docker
harness with read-only source.

The built CLI also passed an actual Docker-volume proof with 33,554,432 bytes
of completed payload: exact source/generation byte comparison, executable
modes, exclusion of an incomplete install and identical selected generation
on retry after renaming away the source. Its exact labelled volume and two
fixture containers were removed and absence verified. Evidence is retained
under `/tmp/bosn-693/tool-bootstrap-*`. This 32 MiB proof does not satisfy the
original realistic large-cache/concurrency or full-workflow acceptance.

Bootstrap is deliberate enrollment, not a missing-cache fallback. Its caller
must persist enrollment history and exclude older writers; an ordinary
startup must read/validate current selection and refuse missing or corrupt
enrolled state. A report is not a generation reader lease. Bosn still needs
ordinary native selection/admission, reservation/retirement orchestration,
archive/all-class routing, allocated-inode accounting, scoped GC and the
remaining original bounded-storage/history and actual-workflow proofs.
Native routing remains Legacy and #508 stays open.

Post-merge verification found that bootstrap also needed the legacy seeder's
`source/*` boundary: abandoned private root stages must not become selected
installs, consume the public payload budget, or make private root symlinks a
refusal. Independent stage and alias regressions reproduced both failures.
The act2.21 tag remains at the audited candidate SHA; its release run
[39701754299](https://github.com/zackees/act2/actions/runs/39701754299) was
canceled before a GitHub release existed or any Bosn pin changed. Do not move
or reuse that tag.

The correction merged in [act2 PR #76](https://github.com/zackees/act2/pull/76),
master `43445419b24317efd69b37c5cc6cf24071884d3e` (reviewed head
`80281a97391eaa863f7628678fcd886dcd945d00`): a typed scan mode prunes private root entries for deliberate
legacy enrollment while preserving native recovery behavior. Visible aliases
are still refused and nested hidden contents inside public installs stay
validated. Focused bootstrap tests pass, the broader CLI/artifactcache suites
pass, and golangci-lint reports zero issues. Independent review is clean.
The actual built CLI selected exactly 33,554,432 public payload bytes with a
64 MiB limit despite a separate hidden 40 MiB completed stage and private
alias. Those private entries and the incomplete install were excluded; exact
payloads, executable modes and source-gone retry were verified. Exact fixture
cleanup and absence checks passed. Evidence:
`/tmp/bosn-693/tool-bootstrap-private-docker-proof.{py,json,log}`.

All six PR checks succeeded, including Linux tests and native Windows/macOS
checks. The external checkout returned to clean master, rebased, and its
merged branch and watcher files were removed (logs retained with evidence).
Exact merged full CI
[39704094890](https://github.com/zackees/act2/actions/runs/39704094890) passed
all five jobs. The guarded new tag `v0.2.89-act2.22` names that exact SHA,
after clean-default-branch and local/remote tag/release absence checks. The
existing release workflow completed successfully in
[39705227950](https://github.com/zackees/act2/actions/runs/39705227950). The
artifact verification and candidate pin update are recorded below; main
remains on act2.20 until this candidate is merged.


### Leased native binding in run admission (candidate, #508, 2026-10-21)

A focused regression exposed a normal shared-run handoff bug before enabling
native planning. A native engine bound to generation `a` served runs from its
maker and a second registry. Preparation incorrectly received `None` from
the first run's legacy template and generation `b` from the second run's
conflicting template, even though both leased the same engine. Evidence:
`/tmp/bosn-693/native-lease-binding-red.log`. The conflicting template
was then strengthened to use the real producer factory and its correct
frozen command; reverting only the preparation authority to the original
plan-based path reproduced the same failure. That path was restored to the
lease binding immediately afterward. Evidence:
`native-lease-binding-valid-template-red.log` and its GREEN counterpart
under `/tmp/bosn-693` (17 focused tests passed, 3.60 seconds).

The lease now carries the actual engine's frozen native binding, and run
preparation takes it from the lease. Newly created engines expose a bounded,
closed-schema generation annotation so another daemon's typed engine identity
and adopted lease preserve the same binding. Malformed or oversized annotations, annotations with unknown fields, and
invalid binding fields cannot supply an identity; an omitted annotation cannot
match a wanted native identity. Legacy engine labels remain unchanged.
The native maker/adopter regression passes, as do the focused machine and
shared-engine tests (16 passed, 3.60 seconds).
Evidence: `native-lease-binding-green.log` and
`native-lease-binding-suite.log` under `/tmp/bosn-693`.

Independent review caught a historical compatibility edge: this new metadata
must not become a mandatory ownership label for an already frozen native
record. Creation annotations are now emitted separately from
`required_labels`; existing profile, command, ownership and mount checks stay
mandatory for observation and retirement. An explicit regression observes
both historical unannotated and newly annotated native containers against the
same frozen profile, then refuses a changed command. It passes; evidence:
`/tmp/bosn-693/native-lease-historical-observation.log`. Historical records are
not rewritten and unannotated foreign native engines are not adopted.

This fixes binding authority at the handoff; it does not itself enroll a
machine or reserve recovery references. Normal native selection under the
machine claim, durable enrollment history, writer exclusion and lifecycle
reservation/retirement remain required before activation.


### Verified act2.22 candidate and real-cache refusal (2026-10-21, #508)

The release workflow completed successfully on exact merged SHA
`43445419b24317efd69b37c5cc6cf24071884d3e`, after full CI passed that SHA.
All eleven released archives were independently checked against the release
checksum file and GitHub asset SHA-256/size, executable architecture headers,
and embedded release. Linux x86_64 version and execution-capability smoke
passed in isolated containers; those exact containers are absent. Other
architectures received static artifact checks, not local execution claims.
Evidence: `/tmp/bosn-693/act2-22-{release-guard,verification,runtime-smoke}.json`.
The candidate updates Bosn's pin to this verified release:

- Archive: `3eab8e383dcfa389eaba3adccfda6ab7fb7981b0950a063082ee5a540dd551a3`.
- Executable: `bbe3f82d542ed767085b09552f480a0930a4cdd061e82c691d5afa2a8b8d9cfa`.

The released CLI passed the private-stage proof: 32 MiB of completed public
payload was selected under a 64 MiB limit despite a 40 MiB private stage and
private alias. Exact byte/mode comparison, incomplete exclusion and an
identical generation on source-gone retry passed. Cleanup is verified in
`/tmp/bosn-693/tool-bootstrap-22-docker-proof.json`. Initial fixture attempts
needed a scoped read capability for protected source cache files and an
executable permission on the extracted smoke binary; their failed fixtures
were cleaned up and their diagnostic records retained. These were harness
permission failures, not accepted bootstrap results.

A separate real-cache experiment copied the production tool tree through a
read-only bind into an owned private volume, leaving its live engine alone.
The source measured **6,426,096 KiB allocated and 51,359 inodes**, about
6.1 GiB, not the earlier 7.7 GiB survey. That attempted clone normalized
ownership only in its disposable copy; a future compatibility run must also
preserve the real UID/GID metadata, using the required scoped capabilities.
**Bootstrap refused the copied cache** after 16.40 seconds, without selecting
a generation: the completed GNU toolchain contains
`package/x86_64-conda-linux-gnu/lib/libgfortran.so` pointing relatively to
`../../lib/libgfortran.so`, which is absent. Read-only inspection found the
same `.so.5` and `.so.5.0.0` links and a completion marker in the production
package. The publisher recipe selects locked GCC/G++, binutils and sysroot
packages; the native snapshot currently rejects every dangling link.
This is an unresolved real-layout compatibility finding, not a passing
large-cache admission proof. No link or cache entry was removed, skipped,
repaired, or treated as an empty cache to make enrollment succeed. The private
fixture and diagnostic containers are absent. Evidence:
`/tmp/bosn-693/tool-bootstrap-real-cache-proof.json`,
`tool-bootstrap-real-cache-rejected-layout.py`, and
`real-cache-syslib-links.json` in that directory.

The candidate's pin/capability, machine, maker/adopter and historical native
observation tests pass (26 focused tests, 3.60 seconds). The real-layout
closure policy and ordinary native activation remain unfinished. This
candidate does not enable native planning or establish the original full
workflow, large-cache, concurrent writer, inode accounting or retention
acceptance.


The candidate also passed the actual Rust recovery-worker fixture against the
verified, offline-staged act2.22 archive: prepare stopped overlay, stage
archive, execute the real helper transport, and independently verify payload,
mode, preservation of another tool update, incomplete-install exclusion and
unchanged lower. The real 20-second reference protected publication; production
source disposal then ran at expiry, and reference release succeeded after the
source was gone. The final record is terminal with both disposal and release
acknowledged, both helper journals are removed, and exact fixture/container/
volume cleanup is proved. Fixture `bosn693-live-publication-1344c9ec21ef`, generation
`e7175675c2ebf7167853dd734574ea765964a0e2e5bb9e29783853991d39ce32`. Evidence:
`/tmp/bosn-693/live-reference-release-22-proof.{py,json,log}`. This remains a
manually reserved recovery fixture and does not prove ordinary enrollment.

### Enrollment prerequisites after act2.22 (#508, 2026-10-21)

The real-cache refusal above led to three merged act2 changes, not changes to
the source cache or removal of inconvenient entries:

- [act2 #77](https://github.com/zackees/act2/pull/77) preserves bounded,
  kernel-confined relative symlink chains ending at an absent final leaf.
  Independent publisher archive inspection confirmed both the GNU
  `libgfortran.so` aliases and LLVM `clang++/clang-cl -> clang -> clang-20`
  graph are shipped that way. Missing ancestors, escaping or absolute
  targets, cycles and excessive traversal still refuse. The final lookup of
  the original path must return ENOENT under the same kernel confinement;
  this also enforces the aggregate directory/leaf traversal budget. Link
  bytes remain unchanged. Snapshot compatibility does not establish that
  those optional tools work.
- [act2 #78](https://github.com/zackees/act2/pull/78) bounds explicit bulk
  enrollment at fifteen minutes and copies at most four files concurrently.
  Increasing the deadline alone failed after 957.6 seconds including cleanup;
  live descriptor and I/O observations found serialized journal waits.
  Metadata and file sync now finish on the original write descriptor before
  it closes, retaining writeback-error reporting. Workers cancel and drain
  before cleanup or publication; directories finish after their children.
  File lengths, hashes and source/destination identity checks remain.
- [act2 #79](https://github.com/zackees/act2/pull/79) permits an explicit
  `installs: []` generation for a genuinely cold store. Omitted/null installs
  still refuse, as do missing/corrupt current selections and an empty source
  passed to warm bootstrap. This is a caller-selected cold initialization,
  never an implicit repair of lost enrollment state.

The final #78 candidate passed enrollment of a private metadata-preserving
copy of the actual cache, mounted read-only for copying. Source allocation was
6,463,148 KiB with 51,364 inodes. Enrollment took **497.465 seconds**, selected
35 installs, and produced 51,388 generation entries containing 9,060,714,849
logical payload bytes. Independent byte/link comparison and a separate metadata
walk checked 51,335 payload entries (47,000 files and 203 symlinks), including
UID/GID, mode and non-symlink nanosecond mtime. Symlink mtime is outside this
manifest contract. Source-gone retry selected the identical generation
`a0ad51a4dcc57f718052ff868449b1adc2845ff9a1a7df022e08ef9a57ec1f66`.
Exact private container and volume absence was verified. Evidence:
`/tmp/bosn-693/tool-bootstrap-real-cache-durable-copy-proof.{py,json,log}`.
Logical bytes and allocated bytes differ; this is not a rerun of the original
7.7 GiB allocated-cache survey or machine-wide accounting acceptance.

The #79 candidate also passed actual CLI cold initialization, selection,
PID1 execution with the original reader descriptor/inode and close-on-exec
cleared, then the first completed-install successor. Missing/null spec,
missing selection, empty warm source and corrupt selection refused. A
source-gone warm retry preserved the selected generation. Exact fixture
cleanup was verified. Evidence:
`/tmp/bosn-693/tool-empty-generation-docker-proof.{py,json,log}`.

All five exact default-branch CI jobs passed on merged act2 commit
`309daac6f5cf37e27fb7ff78b6bfbb7302e83d23` in
[39716801907](https://github.com/zackees/act2/actions/runs/39716801907).
The release guard verified a clean default checkout, the remote default SHA,
that CI evidence, and absent tag/release before pushing annotated
`v0.2.89-act2.23`. The existing tag-triggered
[release run 39717930961](https://github.com/zackees/act2/actions/runs/39717930961)
completed successfully on that exact SHA. All eleven release archives match
both the checksum file and GitHub asset digest/size; executable headers,
architecture and embedded release were independently checked. Linux x86_64
version/capability execution passed in isolated containers, whose absence was
verified. Other architectures received static checks only. Evidence:
`/tmp/bosn-693/act2-23-{release-guard,verification,runtime-smoke,release-provenance}.json`.
The Linux archive SHA-256 is
`228af48050fedbd1e8983a867c5f450c6f0dd02c5afb7499430bb51f4412a654`;
its executable SHA-256 is
`ca6b67a98e041e6ff531edda1c00df906ee59983682c99d18cc638b3d8dee5b2`.
The released cold-generation proof passed with exact cleanup. The released
large-cache proof also passed as detailed below; the Bosn candidate now pins
these verified bytes.

Ordinary Bosn planning still selects Legacy. Durable machine enrollment history,
older-writer exclusion, production native selection/admission and reservation
before ordinary retirement remain unfinished. The candidate proofs above do
not complete those requirements, shared-class/archive routing and expiry,
bounded history/query/storage, or unchanged full-workflow and concurrent/crash
acceptance. #508 remains open and is the only #693 child being implemented.


The Bosn candidate's isolated `ci-engine-test` task passed 63 engine tests
(11 explicitly ignored), six pin tests and five Python runner-tool checks.
The ignored live-Docker recovery fixture was then provisioned and actually
executed with the newly compiled Rust test binary and verified act2.23 offline
archive. All four phases passed: source preparation, archive staging, actual
Rust helper transport and independent payload/mode/lower verification.
Publication was protected by the real twenty-second reference; production
source disposal completed at expiry, followed by source-free reference release.
Exact helper/container/volume cleanup was verified. Evidence:
`/tmp/bosn-693/act2-23-bosn-targeted.log` and
`/tmp/bosn-693/live-reference-release-23-proof.{py,json,log}`. The Rust test
binary SHA-256 is
`94dfd62928dd57031e44e1f499c6cebe5319a373f94143dd643b56a4938ff284`.
This remains an explicitly reserved fixture; ordinary native enrollment and
retirement are not established by it.


The released act2.23 full-cache proof passed, including exact private fixture
absence. Enrollment took **508.589 seconds**, selected 35 installs and the
same generation `a0ad51a4dcc57f718052ff868449b1adc2845ff9a1a7df022e08ef9a57ec1f66`,
with 9,060,714,849 logical payload bytes and 51,388 generation entries.
Current-selection validation, independent byte/link and metadata comparison,
and source-gone identical-generation retry passed. Actual PID1 admission
preserved the original reader descriptor/inode with close-on-exec cleared;
it took 26.367 seconds for this large generation.

An independent bounded, no-follow Python inode walk matched act2's native
store accounting exactly: **102,802 path entries, 55,802 unique inodes,
9,243,017,216 allocated bytes**, 9,121,036,395 apparent bytes,
9,085,055,955 unique regular-file bytes and 18,145,770,804 referenced
regular-file bytes. Accounting deduplicates `(device, inode)` and counts
`st_blocks * 512`, including metadata blocks. These numbers cover this private
native store, not the entire machine's Docker/image/build-cache footprint or
an enforced machine quota. Evidence:
`/tmp/bosn-693/tool-bootstrap-real-cache-release23-proof.{py,json,log}` and
`/tmp/bosn-693/compare-native-tool-usage.py`. The production cache was bound
read-only only for copying; enrollment and mutations used the owned clone.
The source allocation was 6,463,148 KiB / 51,364 inodes, so the original
7.7 GiB survey/full unchanged-workflow acceptance is still outstanding.


### Native admission lifetime wiring (candidate, #508)

A regression through the actual shared-engine lease path demonstrated that a
native engine could admit its first slot with no durable recovery intent or
acknowledgement. The unchanged baseline failed with `first native slot was
admitted without durable lower protection` in
`/tmp/bosn-693/native-first-admission-red.log`.

The candidate now reserves through the trusted registry/act2 transport after
engine preparation and before the shared engine becomes ready. Unsupported
backends refuse. The initial engine creation epoch freezes retry timestamps;
no retry extends an owner. The reference lasts at most 24 hours. Admission
closes after eleven hours, preserving the existing twelve-hour maximum run
budget plus a one-hour teardown/recovery margin. Invalid/future epochs and
an already closed admission window refuse instead of creating a new lifetime.
This is a budget margin, not proof that an arbitrarily delayed recovery queue
will finish before expiry; bounded abandonment remains explicit.

The cutoff is written once while the slot lock is held. Native leases require
it; both native and ordinary lease operations using the new script enforce it.
At the boundary, the script requests retirement and allocates no new slot;
existing holders remain untouched. The native startup command includes a
versioned admission identity so an older caller without this check cannot
adopt it as an equivalent engine. Owner and adopter use the same fixed epoch.
Legacy engine startup command identity and admission remain unchanged.

The regression is GREEN; uncertainty after durable reservation intent still
opens no slot. Focused admission/lifetime, physical shell slot-lock,
shared-engine/owner/adopter, native startup and protocol-fingerprint checks
pass: 21 tests. Service all-target Clippy with warnings denied passes after
moving the new policy test module below its implementation. Evidence:
`/tmp/bosn-693/native-admission-focused-clippy-final.log`. Exposing the existing
12-hour timeout constant to its sibling policy module changes only Rust
visibility, not any wire value/field/encoding; protocol 2 is retained and the
source fingerprint records that explicit no-bump decision.

**Not activated or complete.** Normal planning remains Legacy. Foreign engine
takeover still has a direct source-volume removal path outside the owning
registry's recovery guard. It must retain protected native uppers through a
finite, verifiable handoff before production native activation. Durable machine
enrollment/history, older-writer exclusion, selection and the remaining original
accounting/expiry/full-workflow acceptance are also unfinished. These candidate
tests do not prove an ordinary native workflow or cross-registry crash recovery.


### Foreign takeover and source-disposal fencing (candidate, #508)

A new regression drives the real `MachineEngine::remove_engine` with a synthetic
Docker transport and a native foreign engine. The unchanged removal path
removes the protected upper: `foreign takeover deleted a protected native
upper`. Evidence: `/tmp/bosn-693/native-foreign-takeover-red.log`. An initial
fixture borrow error was corrected before recording that behavioral RED;
`native-foreign-takeover-test-build-error.log` is not regression evidence.
The Bosn regression is still RED; takeover has not been fixed or enabled.

A source volume's retention timestamp alone cannot authorize disposal. A helper
may already have a durable intent in another registry but not yet have sent
Docker create. Docker can recreate an absent named source volume. Therefore
foreign source cleanup needs a producer-side disposal fence that excludes
active views and refuses later source access, plus source mounting that refuses
absence. Per-registry pending-helper checks alone do not establish that
cross-registry exclusion. Source identity/recovery context must also remain
discoverable after the engine and its owning daemon are gone; that handoff is
not implemented yet.

The external act2 candidate `feat/native-source-disposal` adds the first part:
a per-owner source lease acquired before opening the recovery view and strongly
retained with its original lower reader until namespace cleanup succeeds.
Its regression was RED because an exclusive disposal lease could be acquired
inside a live recovery factory, and is now GREEN. Failed opening/unmount,
explicit pin withdrawal and forced Go GC retain the source lease; successful
pending cleanup releases it. Missing, symlinked, hard-linked, empty and
oversized coordination refuses before factory access and is never recreated by
admission.

Reservation initializes coordination only in a tracked private stage and links
without replacement. File and namespace sync precede acknowledgement, including
parent-sync reconciliation on a visible retry. The original inode is preserved.
Coordination is capped at the existing 10,000-owner reference bound; physical
source acknowledgement/reclamation still needs implementation before that cap
can support continuing production churn.

The full compiled artifact-cache suite, focused source-lifetime race tests and
pinned Go lint pass. Evidence:
`/tmp/bosn-693/act2-source-lease-{red,green}.log`,
`act2-source-lease-focused-lint-final.log`, and
`act2-source-lease-full-race.log` in that directory. Initial fixture attempts
needed a preinitialized Bolt coordination inode; their errors are retained
separately and are not counted as behavioral RED.

No act2 commit, PR, release or Bosn pin change is claimed for this source-fence
candidate. The durable disposal operation, old-runtime/store-epoch exclusion,
noncreating source attachment, exact persistent source context, finite guarded
cleanup and actual cross-daemon acceptance remain unfinished. Lease exclusion
alone is not source-disposal authority. Normal Bosn routing is still Legacy,
and all previously recorded original #508 requirements remain open.


### Source disposal decision candidate (not enrolled or shipped)

The next act2 candidate persists the full frozen recovery pin inside each
original source-coordination Bolt inode. A focused regression reproduced a
source reopening despite a persisted disposal bucket
(`act2-source-disposed-red.log`). Both source admission and reservation retries
now reject disposed/corrupt decision state; neither replaces the inode or
repairs it by reopening the source. Exact stored identity also refuses changed
expiry/generation intent after the live pin has been released.

An internal disposal operation holds the original catalog and obtains the
source's exclusive lock with a 100 ms timeout. It requires store epoch 3 and
either actual expiry or an exact durable completion with the reference absent.
It commits a disposal decision before acknowledging authority and reconciles
file/parent sync on retries. Active readers, live references, released but
incomplete work, older store epochs, and conflicting identities refuse.
Failed parent-sync acknowledgement leaves admission closed. Existing epoch-3
stores remain epoch 3 when reserving; no automatic v2 upgrade is implemented.
Tests set epoch 3 explicitly in private fixtures, not in a live machine store.

The first disposal-write test caught the generic coordination adapter's
read-only file descriptor (`act2-source-disposal-focused-lint.log`). The new
writer opens only an existing file without O_CREATE and verifies the opened
inode before bbolt receives it. Focused tests and lint then passed
(`act2-source-disposal-focused-lint-final.log`). Full-suite/race validation of
the expanded exact candidate is tracked separately in
`act2-source-disposal-full-race-lint.log`.

This remains an internal primitive with no CLI, production enrollment, source
attachment change, physical deletion, or global cleanup integration. Store
upgrade must first exclude older helpers; a marker change alone cannot prove
that no preexisting helper owns a source view. Historical v2 sources acquire
no deletion authority. Source decision/identity reclamation and continuing
churn bounds also remain unfinished. The Bosn foreign-takeover regression is
still RED and normal routing remains Legacy. No PR/release or pin update is
claimed for this unpublished candidate.


Expanded validation is now terminal success: the compiled full artifact-cache
suite passed, focused race tests passed (30.210s), and golangci-lint 2.11.4
reported zero issues in `act2-source-disposal-full-race-lint.log`. The final
test-only addition was validated separately with the race detector
(`act2-source-disposal-real-expiry-race.log`, 4.614s): a real two-second pin
expires while its original source reader stays open; disposal still refuses,
then succeeds and reconciles after reader close without a completion receipt.
A changed generation remains refused after expiry. The full-suite binary
predates only that last test addition; production source is identical.
The sole primary review of all eight external files is clean. These checks
prove this internal source-fence primitive, not machine enrollment or the still
failing Bosn takeover path. All changes remain unpublished.


### Source store enrollment and CLI candidate

[act2 PR #81](https://github.com/zackees/act2/pull/81), candidate
`0e0205a85f574415a2b7c6cf6d07278d8d3df9f0`, exposes fresh-only epoch-3 store
initialization before bootstrap and a closed schema-1 disposal report/CLI.
Existing older stores are refused, never upgraded under possible older live
helpers. Default initialization remains epoch 1. The capability is
`tool-recovery-source-fence-v1`; non-Linux builds return explicit refusal.
No release or Bosn pin update is claimed yet.

The production CLI's missing enrollment command was RED
(`act2-source-enroll-cli-red.log`). Round-trip enrollment/disposal tests now pass,
including actual expiry, live refusal, explicit apply and retry. Review found
that an aliased ancestor could create a store that bootstrap later refuses;
`act2-source-enroll-ancestor-red.log` reproduces it. Enrollment and bootstrap now
share the existing canonical-root validation before any path creation; the
physical target remains absent on refusal. Initial lint failures (import group,
unused wrapper and a test variable shadowing Go's real builtin) are retained in
`act2-source-enroll-cli-focused-lint.log` and
`act2-source-enroll-cli-race-lint-final.log`, and were corrected.

Final focused race checks pass (CLI 4.119s, artifactcache 22.319s), and pinned
lint reports zero issues in `act2-source-enroll-ancestor-green-lint.log`.
The broader recovery race checks also passed (48.848s). The full compiled
artifact-cache suite passed in `act2-source-enroll-full-artifactcache.log`; its
binary predates only the final ancestry-validation extraction, covered by the
focused current tests. Windows amd64 and Darwin arm64 cmd/artifactcache
compile-only checks passed (`-exec /bin/true`; no cross-platform execution
claim). Sole primary review of source and contract documentation is clean.

This supersedes only the prior statement that no CLI/enrollment API exists.
Bosn machine enrollment/older-writer exclusion, noncreating source attachment,
globally discoverable exact source context, physical deletion and coordination
reclamation remain unfinished. The foreign-takeover regression remains RED.
Normal planning still selects Legacy; #508 and the other original acceptance
requirements remain open.


The executed released-binary epoch probe additionally passes:
`act2-source-epoch3-older-reader-proof.{py,json}`. Independently SHA-verified
released act2.23 selects and enters a baseline older store, then refuses both
`tool-current` and `tool-exec` on a newly enrolled epoch-3 store. The candidate
accepts both; selection and coordination identities remain unchanged and the
private fixture is absent after cleanup. Candidate binary SHA256
`5ff181f56503eb1edeb9072198e74a07b7776325070ea3217cdd421444af6e59` was built
from exact PR source with `-buildvcs=false` because the read-only container mount
cannot resolve host Git metadata. This is not a release artifact. The initial
probe's empty warm-bootstrap source correctly refused; the final fixture uses
a completed warm install. This does not prove in-place old-store upgrade.


### Merged producer fence and Bosn noncreating source attachment

act2 #81 merged at `971783f1355d6f1c287fb56fb9e2413971e455c5` after all
six PR checks passed (`39726507364` plus codespell `39726507354`). The external
checkout is clean on master, rebased; its local/remote feature branch is removed.
The exact observer output was archived under
`/tmp/bosn-693/act2-81-observer-owned`; no untracked observer directory remains
in that checkout. Exact merged-source full workflow-dispatch CI is
`39727623124`; it must pass before guarded act2.24 publication. No act2.24 tag,
release, artifact verification or Bosn pin update is claimed at this point.
The original Bosn branch was also fetched/rebased onto origin/main with its
tracked edits safely autostashed/restored after the external merge.

Bosn's actual helper transport regression was RED because it still attached
its retained source with `type=volume` (`native-source-attachment-red.log`).
The first command used --exact with an incomplete module name and selected no
tests; `native-source-attachment-red-attempt.log` is not behavioral evidence.
New helpers now freeze the typed, fully ownership-verified Docker-side local
volume Mountpoint into the durable helper role, attach it with `type=bind`,
and recheck exact path plus ownership before starting PID1. Source absence
therefore refuses instead of implicitly creating a named volume. Existing
field-absent helper journals retain their exact historical named-volume cleanup
profile. Malformed/missing paths refuse before helper journaling/create; a
changed path after create refuses before start. Profile mutation tests check
exact source, mode, type, name and driver. This changes persisted helper metadata,
not the daemon IPC surface.

Actual Docker validation rejected the initial read-only backing bind with
`mount recovery overlay: invalid argument`; the recovered overlay uses the
original upper/work directories, whose backing mount must permit kernel mount
setup. Both failed fixtures were cleaned and retained as
`live-reference-release-bind-proof-initial-failure.{json,log}` and
`live-reference-release-bind-proof-readonly-kernel-failure.{json,log}`. The first
assertion omitted the worker's deferred error; its diagnostic now includes it.
The corrected backing bind is RW, matching the producer's kernel requirements;
the producer still mounts and independently verifies the effective recovery
overlay as read-only. This is not a claim that the backing source mount is RO.

The final actual Rust worker proof passes all five phases in
`live-reference-release-bind-proof.{py,json,log}`: preparation, offline verified
act2.23 archive staging, actual helper publication/teardown, expected late-bind
refusal, and independent payload/mode/settings/newer-Node/incomplete/lower
verification. It exercises actual twenty-second expiry, source physical removal
and subsequent source-free reference release. A real Docker create using the
frozen old source path after removal refuses `bind source path does not exist`,
without recreating the volume. Exact fixture containers/volumes are absent;
before/after test-binary SHA256 is identical:
`1d72b2671d9bcfb838cd3882a8a8b4829b4ebf44fe0b56ff0e6e30f9c91c39c2`.
The helper's frozen source path equals the original Docker inspection.
This isolated fixture still uses released act2.23 on a v2 store; it does not
prove integration with the new producer disposal fence or cross-registry GC.

All-target service/registry Clippy passes on the corrected backing profile
(`native-source-bind-backing-clippy-final.log`). The earlier comprehensive
focused checks pass in `native-source-bind-focused-clippy-final.log` (before
only the evidence-driven backing mode correction): helper faults, exact
identity/historical cleanup and source stop/expiry. Corrected-profile targeted
bind/identity tests pass in `native-source-bind-backing-mode-green-final.log`,
followed by the executed actual Docker proof. Initial overlong test and stale
RO profile during correction are retained in earlier Clippy/mode logs. The
sole primary source-bind review is clean. No Bosn commit, push, PR, full gate
or native activation is claimed. The foreign-takeover regression remains RED;
global frozen source context, producer disposal integration, finite cleanup and
the remaining original #508 requirements are unfinished.


### Source context and producer release progress (2026-10-22)

Act2 PR #81 merged at `971783f1355d6f1c287fb56fb9e2413971e455c5`.
Exact default-branch full CI `39727623124` passed all five jobs. The guarded
`v0.2.89-act2.24` release workflow `39728691570` succeeded. All eleven archives
were independently verified against checksums, GitHub digest/size, executable
headers/architecture and embedded version. Linux version and eight capability
checks passed with fixture cleanup. Archive SHA256:
`0c343b45b524caa3bccb816475ff572f6d31f84782241556ec7338a550a27503`;
executable SHA256:
`0410bd0bad8134f78c7fb044dc6cdf0a9f8df15b1b03747270ce25e956e7071d`.
Evidence: `act2-24-release-provenance.json`, `act2-24-verification.json`,
`act2-24-runtime-smoke.json` under `/tmp/bosn-693`. Released act2.23/24 epoch
exclusion also passed: old readers refuse the enrolled v3 store, new readers
accept it, and selection/coordination inodes stay unchanged. Evidence:
`act2-source-epoch3-release24-older-reader-proof.json`. Bosn still pins act2.23;
release verification does not imply pin rollout or native activation.

The Bosn candidate now publishes a deterministic, machine-visible source
context after verified reservation and before acknowledgement/readiness. It
freezes full immutable engine intent and recovery identity in a never-started
metadata container with no data mounts. Duplicate/lost-create reconciliation
requires the complete canonical profile. Native readiness requires exactly one
matching context and the full immutable 24-hour protection window. Shared
registry validation reconstructs the exact pin without changing owner hashes.
Focused service tests, ten registry recovery tests and all-target Clippy passed
in `native-source-context-ready-registry-clippy.log`.

The actual Docker metadata proof passed create, lookup and duplicate-create
retry: state `created`, not running, no mounts, exact read-only/noexec 1MiB tmpfs.
Owned containers were removed and absence verified. Test binary SHA256 before
and after was identical:
`5c400cb206f6c639dc1c7c800852244b2303d54abbc00900667647ff3577082b`.
Evidence: `native-source-context-live-proof.{py,json,log}` and
`native-source-context-live-result.json`. Earlier fixture report writes failed
under `/tmp`; binding the report at `/context-result.json` fixed the fixture.
Those failed reports/logs remain recorded, and all fixtures were cleaned.
The sole primary review of this source-context prerequisite is clean.

PR #682 merged at `f6b39b63ae1ee61e12c912c1dd4421782d252ae2`; the original
checkout rebased onto it with all unfinished changes preserved. No Bosn
worktree was created. Native foreign takeover is now being changed to require
verified context before engine removal and retain its source for producer-fenced
disposal. Missing context refuses before removing either engine or source;
that focused regression and readiness refusal pass in
`native-foreign-takeover-missing-context-green.log`. This is a safety
prerequisite, not completed finite disposal: producer-disposal transport,
post-engine context discovery, metadata reclamation, bounded retention and
ordinary native activation remain unfinished. Normal routing remains Legacy.


The positive foreign-retirement fixture now also passes: verified context permits
engine removal while preserving source bytes and context. Evidence:
`native-foreign-takeover-context-green-final.log` (two focused tests passed).
The first compile attempt wrapped an already optional identity in `Some`;
that type error and correction are retained in the preceding log. Generic
retention inspection found that a Warm metadata object could expire after six
hours (one hour under pressure), before its 24-hour source lifetime. Context
metadata now uses Pinned retention, like protected native cache resources,
so generic GC cannot destroy its authority. This pin must be removed by the
dedicated, producer-fenced disposal lifecycle after source absence; it does not
establish bounded reclamation by itself. Focused model coverage asserts the
retention label. No release, full Bosn gate or activation is claimed.

Latest focused context checks pass: five passed, one explicit hardware ignore;
service/registry all-target Clippy passed in 27.68s. Evidence:
`native-source-context-retirement-clippy.log`. The hardware proof was then
actually rerun against the pinned profile and passed, including independent
Pinned-label inspection, create/retry and exact fixture absence. Evidence:
`native-source-context-pinned-live-proof.{py,json,log}`. Before/after binary
SHA256: `4a409d53225fa4800942823ae62a4c9ac0b2837798b34c6510e8b3f1b52543df`.
Sole primary static review of the retirement prerequisite found no issues.


### Generic source-pressure bypass (2026-10-22)

Tracing producer disposal exposed a second deletion authority: after the source
engine is absent, the generic managed-retention planner sees an unmounted Warm
volume and may reclaim it after the one-hour idle floor under pressure. Neither
the recovery intent nor the producer's source lease is part of that planner.
A real planner regression fails before the fix at two hours of age, within the
24-hour source lifetime: `native-source-pressure-red.log` reports `pressure
selected a protected native source` (one actual failed test, not a compile error).
New native-generation private volumes now carry Pinned retention; ordinary
private volumes retain Warm. Their exact ownership validator derives the same
profile. A successful dedicated removal still requires exact ownership and
non-forced Docker deletion; pinning generic cleanup does not complete that path.
The initial focused validation passes all three storage tests in
`native-source-pressure-green.log`. Coverage now also checks that pressure
continues selecting ordinary private volumes at the same age. The recovery
transport fixture now explicitly models the pinned native source.

Producer integration must not treat the registry's historical
`tool_recovery_source_disposal_at` as an act2 fence receipt. That timestamp only
excludes this registry's newly admitted publication helpers. A v3 producer fence
must be obtained and strictly verified on every deletion retry, including after
a lost Docker reply; it must precede physical removal and remain closed after
failure. A source-free journaled helper needs a distinct frozen operation/profile,
not a publication/release helper with a substituted command. It must work after
expiry without assuming a source-stop receipt (the existing expiry path skips
that receipt), verify engine ID/name absence independently, and preserve metadata
until exact source absence is proven. Existing v2 stores cannot be upgraded or
used as proof of v3 closure. Pin rollout, enrollment and the source-free disposal
helper remain unfinished; ordinary routing is still Legacy.

Fresh-volume boundary: previously created native sources labeled Warm now fail
exact ownership validation. This change does not retroactively protect or
recover them; no silent relabeling/adoption is authorized. Native routing is
still inactive. Enrollment/migration must explicitly drain or reconcile older
source families before activation; never relax the retention-label check.

The pinned-source actual Docker reproof passes all five phases (prepare,
offline archive staging, actual Rust helper, expected late-bind refusal,
independent payload verification) and exact fixture cleanup:
`live-reference-release-pinned-bind-proof.{py,json,log}`. This proves that
explicit owned cleanup still removes the pinned source at actual expiry and
that source-free reference release succeeds afterward. It remains an isolated
act2.23/v2-store proof, not v3 producer-fence or global-disposal acceptance.

Final source-pressure prerequisite checks pass: 32 source tests (two explicit
ignores), five helper tests (one explicit ignore), and all-target service/registry
Clippy in 49.07s. Evidence: `native-source-pressure-focused-clippy.log`.
Actual pinned-source proof used unchanged binary SHA256
`f01455cba1f19885ac42a3f00310e8e27960e890cad69420efb9fc9f3d83305d`
before and after. Sole primary full-candidate review covers Rust, shell and
living documentation with no static blockers. No full gate or PR is claimed
at this recording point.

Pre-push platform audit found that the context test module imported the
Unix-only recovery fixture without the same guard. The test module now uses
`cfg(all(test, unix))`; production context code remains portable. The first
full gate was intentionally interrupted before reuse (117s; no successful
attestation), and its exact two owned CI runs were explicitly cancelled and
verified done/cancelled: `a3d91c43-19ce-4d0b-9835-c8e8267d694b` (queued),
`7d3a329c-cbc4-4cf4-a96c-9780505eabd2` (cleanup removed). No unrelated runs
were touched. Evidence: `native-lifecycle-local-gate.log`. A corrected commit
requires a fresh complete gate; the interrupted run is not a pass.


### Producer-fenced disposal integration candidate (2026-10-22)

Bosn #703 merged at `5d16c1b51bf688a8a0d050f48d4c6e969f98e21c`.
The original checkout rebased to clean main; the owned branch/applied task stash
were removed, user stashes/worktree preserved. Full local gate passed 480s
(Rust476s, Linux480s including queue), tree
`f71abdae6ba280137e76521d1005119d7e6b5f10`, stamped
`40fff60ee9f96322da1e6b389bbd93eb627b7301`. Independent GATE-003 verification
passed before push. Exact remote run `39733543747` succeeded with nine successful
jobs and five configured skips. #508 remains open; normal routing is Legacy.

The next behavioral RED exercises actual retirement with expired native intent
but no producer fence transport. It previously returned `Ok(())` and removed
the source: `native-producer-disposal-red.log`. The candidate now requires a
producer fence after the registry admission claim and before every physical
removal retry, including when Docker previously removed the source but lost its
reply. A registry disposal timestamp never substitutes for producer authority.
The missing-transport regression passes in
`native-producer-disposal-refusal-green.log`.

A new durable `ToolRecoveryDisposalV1` role freezes the complete pin and command
hash, with a distinct helper name/ownership component. Its unprivileged profile
mounts only the exact cache, drops all capabilities, has no network and overrides
the image's implicit data volume with a small read-only/noexec tmpfs. It never
mounts the source. Engine ID and deterministic name absence plus cache ownership
are checked before creation, before start and again after proven teardown.
A closed bounded receipt must match schema, root and every pin field, report
`disposed=true`, and contain no partial result or error. No producer receipt is
cached in the registry: every retry invokes act2 again. Only after helper
absence may exact-owned, non-forced volume deletion proceed.

Expired/unacknowledged recovery can request this helper only after the registry
has committed its source-disposal admission fence. It need not have a historical
source-stop receipt: the existing expired path deliberately skips that receipt,
and the runtime independently proves writer absence. Missing/corrupt/un-enrolled
producer state refuses rather than allowing deletion. Fresh epoch-3 enrollment,
not an in-place upgrade, remains required. The candidate pins independently
verified act2.24 using the release evidence/digests above; it is not merged yet.

The complete synthetic Docker boundary passes an eighteen-mode disposal/physical
removal fault matrix plus a lost physical-removal reply retry that proves a second
producer helper actually ran: `native-producer-disposal-helper-green.log`.
The expanded checks pass four disposal tests and eight source-stop tests in
`native-producer-disposal-focused-clippy.log`; its initial Clippy run correctly
rejected a literal-boolean assertion in the revised historical fixture. That
assertion now checks the exact CleanupRequired state. The older source-stop
fixture cannot serve producer fencing and now tests refusal/retryability;
successful fenced deletion and lost replies moved to the complete helper fixture,
not an ignored/skipped test. Actual epoch-3/released24 Docker validation and
final Clippy remain pending at this recording point. No native activation,
foreign-context reclamation, finite metadata cleanup or completed #508 is claimed.

Actual released24 epoch-3 Docker validation passes all six phases in
`live-source-disposal-24-proof.{py,json,log}`: the independently verified
released CLI enrolls a fresh epoch-3 store; a matching-source fixture prepares
metacopy state; the verified archive is staged offline; the actual Rust worker
publishes, proves publication-helper teardown, waits real twenty-second expiry,
executes the distinct disposal helper and proves its teardown before physical
source deletion/finalization, then releases the reference with the source gone.
Independent payload/mode/settings/newer-Node/incomplete/lower verification and
late-bind refusal pass; exact containers/volumes/anonymous-fixture absence is
verified. The result records the Removed ToolRecoveryDisposalV1 journal.
Before/after test-binary SHA256 is identical:
`c9340966710c8377a42d7fc5c90ca7b21e26510979af9378adc5a988ff8f9100`.
The corrected focused checks and all-target Clippy pass (26.59s) in
`native-producer-disposal-focused-clippy-final.log`. This isolated owned-registry
proof does not establish foreign orphan enumeration, context reclamation,
coordination-file expiry or global storage ceilings. Normal routing remains
Legacy; the branch has no commit, full gate, push or PR yet.

Final admission coverage passes: one registry integration test proves an
expired unacknowledged intent without a source-stop receipt still requires a
committed disposal claim, refuses changed identity and survives registry reopen;
one service test refuses an unclaimed request before contacting Docker.
All-target Clippy passes in 17.15s (`native-producer-disposal-admission-clippy.log`).
Sole primary full-candidate Rust/docs review is clean. Only comments and removal
of an obsolete historical fixture branch followed that validation; physical
lost-reply coverage lives in the complete fenced transport fixture. The branch
will require the full exact-source local gate and independent attestation
verification before any push.

### Retained native source recovery after registry loss (2026-10-22, #508)

Bosn #704 merged at `1b8c494aacd3fda4c0324ed5e8fccbfff85a816f`.
Its full exact-source local gate passed in 436s (Rust436s, Linux435s),
stamped `17b259af1257dc696615930261d221b7db45ab09`, tree
`028e35159c7ad9acc08b3a5381bb25d0a44fffeb`; independent GATE-003 verification
passed before push. Remote run `39736979563` succeeded with eleven successful
jobs and three configured skips, including independently executed Rust tests.
The original checkout returned to clean rebased main and the owned branch and
watcher files were removed. GitHub interpreted a negated closing-keyword phrase
in that prerequisite PR as an issue-closing instruction. The issue was reopened,
the phrase removed, and the resulting OPEN state and empty closing-reference
list were verified. #508 remains unfinished; normal routing remains Legacy.

A retained source context survives engine deletion, but the earlier helper
requires a row in the original registry. The new shared typed
`ActToolSourceContext` preserves the existing v1 serialized metadata and validates
its full original registry/run/profile/recovery binding. A replacement registry
journals `ToolSourceFinalizationV1` under its own owner, freezing the original
context, immutable context ID, command hash and optional verified source bind.
It neither invents an engine row nor opens the original registry. Pending helpers
exclude another source helper in that journal. A snapshot remains bounded to
24KiB, including the frozen source; this is not bounded historical event storage.

Act2 #82 merged at `9c79f839737a787e5f632939e9e167e0c984ca4d`.
Exact default-branch full CI `39742949161` passed all five jobs before the guarded
annotated `v0.2.89-act2.25` tag triggered release run `39745051172`, which succeeded.
All eleven archives were independently checked against checksums, GitHub asset
digest/size, executable architecture/header and embedded release. Linux version
and all nine capabilities passed runtime smoke with exact cleanup. Archive SHA256
`8855c9fb840309b0eed71f85e17e8105aaecfebd4fef39ce28a8eab2f6a58e21`, executable
SHA256 `f4784de9dd2ebbf6a8682decf855ca3a1a69d32dd458fc6847023585a7a7fb5a`.
The external checkout is clean on rebased master; its task branch and exact
observer files were removed. The Bosn candidate pins this verified release.
Evidence: `/tmp/bosn-693/act2-25-release-provenance.json`,
`act2-25-verification.json`, `act2-25-runtime-smoke.json`.

The producer's `finalize-source` owns publication, successful view cleanup,
reference release and durable source fencing. It requires an existing epoch-3
store and exact existing coordination inode, never enrollment or reconstruction.
Before expiry it publishes under the original source/lower leases; failed unmount
retains those leases and cannot release the reference. After expiry it retries
pending cleanup, fences existing readers, then releases the expired reference
without opening a new source view. Every retry revalidates identity and repeats
the fence. It does not physically delete storage. Its absent-command CLI RED,
complete artifactcache/serve tests, focused CLI recovery tests, zero-issue Go lint,
and actual stopped-overlay candidate proof are recorded in act2's public cache
retention documentation. Initial dynamic-loader refusal and the corrected real
expiry fixture remain explicit rather than being counted as successful checks.

Bosn now has a separate retained-source supervisor in the daemon startup join.
Discovery partitions canonical source-container names by immutable owner hex
prefix. A query returns at most 65 IDs; an overflowing partition splits into
sixteen children instead of silently discarding its older IDs. Docker name
uniqueness bounds a full-owner leaf. The stack is at most 1,024 prefixes and the
pending leaf at most 64 IDs. Each pass issues at most eight inventory queries,
examines at most 32 candidates and attempts one source lifecycle. Cursor advancement
includes failed/invalid candidates; current local nonterminal records remain with
the existing recovery worker. Live engines are skipped. Waits are async,
cancellable and bounded; diagnostic history is capped at 64 bounded keys.

Before create, before helper start and after proven namespace teardown, the
runtime revalidates the exact stopped metadata profile, engine ID/name absence,
shared-cache ownership and any present source's original ownership. Live recovery
uses an absence-refusing bind of the frozen Docker-side source path; expired
finalization uses a source-free unprivileged profile. Both have distinct helper
identity and frozen command binding. A closed, bounded receipt requires the
entire exact pin, `disposed=true`, `reference_absent=true`, no partial result or
error, and a valid publication within the original lifetime when still live.
Only then does exact-owned non-forced volume removal proceed. Every deletion
retry runs producer finalization again, including after a lost removal reply.
Context metadata is removed by immutable ID only after source absence is proved.
An unsupported historical engine image refuses; this is not universal mixed-image
adoption. Missing/corrupt/legacy producer state also refuses.

The actual receipt regression failed against the previous disposal parser, then
passed with the dedicated closed finalization schema:
`orphan-finalization-receipt-red.log`. The replacement-registry test stops the
original actor, moves its database out of its original path, and verifies that
no replacement engine row is fabricated. The 26-mode Docker boundary matrix
passes, as does a lost physical-removal reply requiring a second producer helper
(`orphan-finalization-transport-final-clippy.log`). The first matrix run exposed
an incorrect fixture expectation: an expired source-free helper does not need a
stable unused mountpoint. The changed-path refusal now uses a live source with a
frozen bind. Initial compilation needed the existing backend trait import;
Clippy later required splitting a 105-line test, without relaxing its limit.
Corrected service/registry all-target Clippy passes in
`orphan-finalization-pin25-clippy.log`.

The actual Docker discovery proof provisioned 70 private, never-started metadata
containers, including equal creation-second groups. Its parent query deliberately
returned only 65 IDs; production prefix discovery still visited all 70, with
query counts `[3,2,8,4]` across passes. Exact fixture absence and unchanged test
binary were verified. Evidence: `source-discovery-docker-proof.{py,json,log}`.
The first real replacement-registry disposal proof passed all runtime phases:
fresh epoch-3 enrollment, stopped metacopy overlay, verified offline released
archive, actual Rust helper with unavailable original registry, source/context
removal, source-free producer retry, and independent payload/mode/settings,
newer-Node, incomplete-install and unchanged-lower verification. Its cleanup
assertion initially misclassified the intentionally retained read-only build
target as an anonymous volume. Exact cleanup was completed separately and the
build target preserved; this is recorded in
`orphan-source25-initial-cleanup-refusal.{json,log}` and
`orphan-source25-initial-cleanup-recovered.json`. A corrected full proof through
the production worker is required before push; the earlier direct-helper result
is not that proof.

This candidate does not complete #508. Native creation/enrollment is still
inactive, and the new path requires an already valid frozen context. Pre-Ready
provisioning gaps, old-writer exclusion and migration, finite producer
coordination/completion and registry helper/history reclamation, memory-mode
handoff, all-class routing/accounting and machine ceilings, scoped builder GC,
and unchanged full-workflow/fresh-engine/restart acceptance remain unfinished.
Physical source/context removal does not prove finite lifetime for their producer
coordination files or journal events. Original large/concurrent/many-file
acceptance remains required. No other #693 child has started.


The corrected complete Docker proof now passes through the production
`retry_retained_sources` worker, rather than invoking only its helper. A
fixture-scoped full-owner cursor selects only the private source; the separate
70-container proof covers actual partitioning. All seven phases passed, the
original registry was unavailable, the replacement registry had no engine row,
and its finalization helper journal ended Removed. Source and context were
physically absent before the source-free producer retry. Independent payload,
mode/settings, newer Node, incomplete exclusion and unchanged-lower verification
passed. All exact containers/volumes were cleaned and absence proved, while the
read-only build target was preserved. Evidence:
`orphan-source25-docker-proof.{py,json,log}` and `orphan-source25-result.json`.
Before/after test-binary SHA256 is identical:
`b6a0570acbb291aadaaa1f6c27c0cd3fa5b2e27a17c4007f5e1b3436ff8e4536`.
The strengthened shared context also rejects aliasing the source and shared cache.
Final context tests and service/registry all-target Clippy pass in
`orphan-source-worker-final-clippy.log`. Full exact-source gate and PR are pending
at this recording point. This completes the tested retained-context production
path, not ordinary enrollment or the remaining issue-wide storage/expiry contract.


The first full exact-source gate on `46951ea60d1936c3a9da4028bb36076c74665e5d`
failed, and no lane attestation was accepted. The Rust lane
`27c291b8-40f2-44e6-9016-e1c5144e70de` and tests lane
`320654ec-84e6-4886-b93e-f3e825455bfc` both exposed a new daemon startup
stack overflow on a normal Tokio worker. The installed-wheel smoke also
aborted before daemon readiness. Focused source-helper proofs had missed the
size of the composed production Service future. Evidence is retained in
`orphan-source-local-gate.log`, `orphan-source-rust-full-failure.jsonl` and
`orphan-source-linux-progress.jsonl`.

The correction heap-owns the new retained-source supervisor at its join
boundary and each bounded pass at its timeout boundary. It changes neither
worker stack limits nor acceptance checks. With those two boundaries, the
previously crashing Python registry-diagnostics test and both coalesced setup
ensure/task tests pass with the ordinary dev-profile runtime in the Docker
Rust harness. A fresh full gate, including installed-wheel daemon startup,
is still required before push. This failure and correction do not complete
#508 or authorize ordinary native routing.


### Ordinary native enrollment: active implementation (2026-10-22, #508)

Retained-context PR #705 merged at
`90fde723ab373c1ae60ccb6fafc821d54646f94d`. Its corrected full gate passed
391s (Rust391s, Linux373s), stamped
`3a110a233b799f1c0262013fb373bcb2b6fa3d2c`, tree
`48e68b32ed9e16cef566430ff4d256866ba688b2`; independent GATE-003 verification
passed before push. Rust passed 1,145 tests with49 configured skips; Linux
passed264 with10 configured skips, including both prior startup failures.
Remote run39750419031 completed success with9 successful jobs and5 configured
skips. Wheel placeholders are not artifact validation. The original checkout
returned to clean main and rebased after merge, and the owned local/remote
branch and exact observer file were removed. #508 remains open.

The next real production boundary is ordinary native enrollment, rather than
another manually bound recovery fixture. A behavioral Bosn RED demonstrates
that an ordinary template requesting native tools is still admitted as Legacy:
`native-enrollment-template-red.log`. The original checkout now holds this
uncommitted RED plus a typed optional policy field on
`feat/508-native-enrollment`; production policy remains unset. No native route
has been activated or represented as passing.

The external act2 checkout on `feat/tool-enrollment-history` contains an
uncommitted producer API/CLI for enrollment. Its first behavioral RED proved
that directly reusing explicit Bootstrap recreates a missing warm selection
(`tool-enrollment-history-red.log`). A fixed sibling Bolt history now distinguishes
Pending and Ready across registry loss and whole native-store loss. The history
contains a random256-bit store identity, mirrored and synced in a fixed native
root marker; it never persists VM-specific device/inode numbers as incarnation
authority. Ready only reads the exact epoch3/token and bounded current-generation
metadata. It does not hash payload during planning or invoke bootstrap/initialize;
the original tool-exec reader remains responsible for payload verification.
Corrupt payload is refused there even when the planning hint succeeds.

Early review found and independently reproduced epoch1 initialization, existing
empty-history acceptance and a source/history path collision. Epoch3 is now
enrolled after durable Pending and before payload publication, existing empty or
unidentified histories refuse, fresh creation is exclusive, aliases/special files
and mount/size boundaries refuse, and reserved path collisions refuse before I/O.
Ready marker downgrades and a replacement valid epoch3 empty tree also reproduced
unsafe acceptance; exact epoch/token requirements now refuse both. Evidence:
`tool-enrollment-epoch-red.log`, `tool-enrollment-history-alias-red.log`,
`tool-enrollment-ready-identity-red.log` and subsequent focused green logs.

A machine claim alone cannot exclude legacy writers after a daemon crash while
an external enrollment operation remains alive. The producer therefore has two
explicit phases. Fence-only records Pending, creates source-store epoch3 and its
identity, and atomically exchanges a legacy directory with a regular refusal
file using renameat2(RENAME_EXCHANGE). An absent cold source uses NOREPLACE.
There is no copy/ordinary-rename fallback. A frozen private snapshot retains the
old directory with its own bounded token marker; both namespace entries share
one parent that is synced before acknowledgment. Fence-only admits no generation
and makes no quiescence claim. Ready hints always require the original source
refusal file, without repairing it.

Existing directory descriptors survive exchange. Bosn must re-inventory and
drain every other old engine/helper after fencing and before the explicit
require-fenced completion. That completion never creates missing history or
exchanges/repairs the original source path. A test actually holds a directory FD
across cutover, finishes an install through it, closes the writer and verifies
that completion preserves its late payload. A second RED showed an initially
empty source permanently refusing that completed late install; Pending now
makes a durable one-way cold-to-warm transition after drain and before publishing.
An absent source that gained a directory before fencing also reproduced a
liveness refusal; Pending durably freezes that newly arrived directory before
exchange, preserving its completed install. Ready never takes either path.
The first transition attempt had a stale report Mode despite its updated journal;
that report was corrected before the final focused pass. Evidence:
`tool-enrollment-legacy-fence-red.log`,
`tool-enrollment-late-cold-writer-red.log`,
`tool-enrollment-source-appeared-red.log` and
`tool-enrollment-late-writer-green-final.log`.

Focused artifactcache/CLI/capability tests pass (30.480s/1.792s), lint0issues.
An initial test WriteFile permission was corrected to0600. The actual embedded
seed/save scripts refuse the source file at mkdir, before any Docker call or cold
volume creation; their bytes match release act2.25 exactly, with hashes in
`tool-enrollment-old-recipe-provenance.json`. This is an embedded-recipe proof,
not execution of an old full workflow. Gosec rejected a test's dynamic shell
string; the proof now executes the two fixed repository script paths directly,
without relaxing the linter. Complexity21 was split into a pending-transition
function, preserving the limit20. The earlier full artifactcache race and CLI
suite passed before the physical fence changes; the fresh phased full race/CLI
suite is running at this recording point and is required before publication.

Bosn integration must give enrollment the physical lifetime of its claimed,
ordinary tracked temporary engine. It must never open workflow slots in that
engine. Cancellation, uncertain/late create and takeover must prove its ID and
canonical name absent before releasing its claim; failed/background cleanup
keeps the claim. The own-claim shortcut must not release a claim while its
preparing engine exists. After source fencing, post-cutover writer drain and
publication, retire that engine and create the actual native engine under a new
genuine daemon claim with a frozen generation binding. Native template matching
must validate the exact pinned init/recipe/payload bound while reusing an engine's
frozen generation. Do not make fake process identities, start the claim container,
or create another cache-helper protocol to avoid this lifecycle.

This is WIP, not a new merged fix. No act2 release or Bosn pin has changed after
verified25. Native enrollment, real-engine orchestration, cancellation/restart/
whole-workflow/large-cache proof and the remaining original all-class accounting,
finite metadata/history, archive migration, memory handling and scoped GC remain
unfinished. The retained legacy snapshot is not yet reclaimed; its finite removal
and accounting remain part of the original contract. No other #693 child started.


### Ordinary enrollment continuation: preparation cancellation boundary

The phased act2 artifactcache race binary and focused command suite completed
successfully (`/tmp/bosn-693/tool-enrollment-phased-full-race-cli.log`, session
53267 exit 0). This proves the tested producer implementation, not released
Bosn routing or the remaining whole-machine acceptance requirements.

Bosn preparation now retains its genuine machine claim until normal durable
engine cleanup succeeds and inspection proves the canonical engine name absent.
The cancellation guard reads the committed execution claim from the registry,
including when the original caller never received that token. Cleanup failure
or runtime shutdown retains the machine claim. A subsequent lease retries that
same durable cleanup before releasing the claim; it cannot simply discard an
own-daemon claim whose engine may still exist. Native policy also disables the
legacy-drain ten-minute fallback that would create beside old writers.

Focused evidence: `enrollment-preparation-guard.log` passed the cancellation /
failed-removal check; `enrollment-preparation-shared.log` passed all 11 shared
engine tests, including retry after the fault clears, replacement, and normal
retirement. The existing primary reviewer found no static release-order hole;
late/unacknowledged-create behavior still relies on the existing durable
cleanup grace model and needs integration-level coverage before activation.
No PR or release was made for this continuation. Production native policy
remains unset; the ordinary-template regression remains intentionally RED until
the enrollment controller and actual producer transport are connected.


### Ordinary enrollment controller: template regression GREEN, production still gated

The native policy template is now connected to shared-engine creation. Under a
real daemon-owned machine claim, an ordinary tracked preparation engine installs
the pinned act binary but never calls serve preparation, marks Ready or opens a
workflow slot. The typed `tool-enroll` Fence receipt acknowledges no generation.
Bosn re-inventories and drains every other legacy engine after that physical
source cutover, with cancellation and no timed fallback beside old writers, then
requires a closed Complete receipt. A validated binding freezes the selected
generation, logical payload bound and current overlay recipe. The preparation
engine is normally retired and its canonical name proved absent before its
claim is released. A new UUID and genuine claim create the actual native engine;
existing recovery reservation runs before its first slot opens. The legacy
source refusal file spans that claim gap. Cancellation retains the guard and
normal durable cleanup. No fake daemon identity or running claim is introduced.

Policy matching recomputes the complete expected identity using the held or
foreign engine's frozen generation; ordinary reuse need not force retirement
when selection later advances. Unsupported memory-backed policy and historical
socket layouts refuse before source mutation, at lease entry and creation.
The production planner still sets `native_tools: None`: actual release transport,
retained-snapshot reclamation and real acceptance remain prerequisites to
activation. Existing archive routing is still Legacy. The prior ordinary-template
RED status above is superseded by this tested controller implementation.

Evidence:
- `enrollment-controller-first.log`: original ordinary-template regression GREEN.
- `enrollment-controller-ci-tests.log`: 311 passed, 17 configured ignored hardware/
  large-fixture tests; includes strict closed receipts, ordinary selection and
  frozen-generation reuse. This run precedes the final validation helper split.
- `enrollment-controller-native-final.log`: three native checks passed; the new
  memory-refusal test initially constructed invalid limits and failed at fixture
  setup. It was corrected to use the real memory-policy sizing boundary.
- `enrollment-controller-native-green.log`: all four native checks passed after
  that fixture correction and helper split. Covers initial protection, uncertain
  reservation refusal, ordinary selection/reuse and explicit memory refusal.
- `enrollment-controller-clippy.log`: latest source passes service library/tests
  Clippy with `-D warnings`; function/file limits were preserved.
- Sole primary reviewer found no remaining high/critical controller finding;
  policy validation was also moved to lease entry following their consistency
  observation. These checks are not actual Docker or full-workflow acceptance.

Act2 dependency candidate `6c113bf5b806ef3a6090bce64270126ba5a5170c` is published
for review as https://github.com/zackees/act2/pull/83. Full PR checks are still
running; no merge, dependency release, Bosn pin update or Bosn PR has occurred
in this continuation. Full exact merged default-branch CI and independent asset
verification remain mandatory before using a new released pin.


### Enrollment transport correction and dependency merge

A real transport inspection found that the initial enrollment call used PATH
`act`, although installation verifies the private executable under
`ENGINE_WORK/bin`. The new synthetic-Docker behavioral regression failed with
`unverified PATH binary refused` (`enrollment-private-binary-red.log`) and passed
after the call was corrected to the absolute verified installation
(`enrollment-private-binary-green.log`). Closed receipt validation plus this
transport check passed (`enrollment-transport-preflight.log`: two passed, one
explicit live-fixture ignore). The live transport harness remains unexecuted.
It requires independently verified release provenance, preserves warm bytes and
mode, checks inherited original-reader device/inode and legacy refusal, records
candidate/build/binary provenance, and cleans exact private Docker resources.

Act2 #83 merged normally at `83564159344ccbceaf0f348531967cba4fa21642` after all
six PR checks passed on head `6c113bf5b806ef3a6090bce64270126ba5a5170c`, with no
reviews or unresolved review threads. External checkout is clean on master,
rebased to origin/master, and the merged feature branch was removed after tree
equality. Bosn was fetched and rebased onto current origin/main using a successful
autostash; all three prior user stashes remain unchanged. Exact merged default
full CI is run https://github.com/zackees/act2/actions/runs/39762337638 on the
merge SHA. It is still running; no act2.26 tag or release has been made and Bosn
still pins verified act2.25. Guard and independent eleven-archive/runtime
verification scripts are prepared for act2.26, not executed.

A second explicit hardware fixture now exercises genuine SharedEngine native
admission, protection acknowledgement before first slot, retirement, and another
fresh engine selecting the same warm generation. It positive-matches the
isolated Docker daemon ID and the compiled act version to a typed fixture.
Its harness provisions a private outer Docker daemon, mounts only that daemon's
control volume into the Rust runner, seeds the exact released archive and one
completed legacy install, independently checks payload/mode/source fence and
removes the exact outer containers and volumes. It does not use the production
Docker socket in the runner and makes no workflow-execution claim. The fixture
has compiled but is explicitly ignored until provisioned; latest Clippy passed
with warnings denied (`enrollment-live-controller-clippy.log`). Rebuild against
the verified new pin before executing it; no hardware success is claimed here.
The enrollment controller branch is boxed at the ordinary make boundary to
avoid enlarging the ordinary daemon future; four focused admission checks pass
(`enrollment-boxed-controller-green.log`). Production policy remains unset.


### Exact dependency gate and interrupted-claim liveness

Default-branch full act2 CI 39762337638 completed successfully on exact merged
SHA `83564159344ccbceaf0f348531967cba4fa21642`; all five jobs (lint, Linux,
Windows, macOS, snapshot) passed. The act2.26 release guard passed first without
publishing, then revalidated clean master, exact remote SHA, full CI and absent
tag/release before pushing annotated tag `v0.2.89-act2.26`. Existing tag-triggered
release run https://github.com/zackees/act2/actions/runs/39764326180 is in progress.
No release artifact has yet been accepted or Bosn pin changed in this step.

Cancellation fault tests now enter Fence and Complete separately, cancel the
following request, and prove prompt refusal, no Ready/slot, and retention of
engine plus machine claim when physical cleanup fails. Initial test compilation
needed an explicit closure lifetime and owned run string; the corrected test
passed (`enrollment-phase-cancellation-green.log`). Extending it to clear the
fault and run normal maintenance exposed an actual liveness defect: own live
claims were never retried by tend_machine (`enrollment-claim-reaper-red.log`).
The reaper now uses the existing strict durable preparation cleanup when the
claim is ours and the held-engine mutex is empty; active preparation holds that
mutex and cannot be retired. Failed cleanup preserves its claim. A new ordinary
request waits with a bounded reason instead of failing immediately while
recovery remains pending. No source protection or absence proof was weakened.
The same regression is GREEN (`enrollment-claim-reaper-green.log`), and the
subsequent lifecycle group passes 20 tests with one explicit hardware ignore
(`enrollment-claim-cleanup-lifecycle.log`). The primary reviewer found no
remaining high/critical static issue in these changes.

The isolated controller fixture was corrected to start the actual production
recovery/cohort supervisors and wait boundedly for normal reaper cleanup after
retirement. Native source retention is expected until producer recovery and
release are durable; immediate claim disappearance was an invalid fixture
assumption. It does not bypass that retention, fabricate source completion or
force-delete the claim. The real Docker run remains pending independently
verified release artifacts and a rebuild against that pin. All other #508
acceptance gaps remain open, and no later #693 child has started.


### Verified act2.26 and actual controller RED → GREEN

This section supersedes the pending release/artifact/controller statements above.
The release workflow 39764326180 succeeded on exact
`83564159344ccbceaf0f348531967cba4fa21642`, after all five full default CI jobs
39762337638 passed. Annotated tag `v0.2.89-act2.26` peels to that SHA. All eleven
archives match release checksums, independent GitHub digests/sizes, executable
architecture/header and embedded version. Linux runtime version and all required
capabilities, including `tool-enrollment-v1`, passed with exact smoke cleanup.
Archive SHA256: `a2e1facf5faa0d9493561a38a85aa63dbef2bc66f1c429db8c31496f2ed44553`.
Executable SHA256: `f9086369b440d07ecc3ad8a12d50e7537754940ead8fb30070c932524c836a01`.
Evidence: `act2-26-{release-guard,verification,runtime-smoke,release-provenance}.json`.
The candidate now pins these verified bytes in core ACT_VERSION and service pins;
this is unpushed Bosn work, not a Bosn rollout.

The actual Rust enrollment transport passed against released act2.26. Independent
checks confirmed warm bytes and mode, regular-file legacy refusal, and the child
inheriting the original generation reader device/inode. All exact Docker fixture
resources were removed. Evidence: `enrollment-release26-transport-proof.{log,json}`.

The first full-controller harness stopped before admission because its private
outer Docker daemon lacked delegated cgroup controllers. It was corrected to run
Bosn's exact private ENGINE_INIT rather than changing host cgroups. Cleanup was
verified before retry; see `enrollment-release26-controller-proof-initial-cgroup-failure.json`.
The corrected real controller admitted a protected native engine but failed its
bounded retirement wait: no workflow had yet initialized the recovery overlay.
Producer recovery correctly refused missing source paths. Evidence:
`enrollment-release26-controller-missing-source-red.json`. The source reservation
path now calls existing act2 serve prepare with only the frozen tool generation
(no image, stock tools or workflow) before committing ToolRecoveryBegin. The
original PID1 reader protects the lower during this producer-owned preparation.
Its exact closed receipt must name the frozen generation. Failure/cancellation
before reservation has admitted no work; normal exact-owned preparation cleanup
remains valid. Reservation/source preparation is now cancellable and bounded.

This actual producer call then exposed a second real integration mismatch:
`fa1d1738…` described the manually exercised job path `/opt/hostedtoolcache`,
whereas serve mounts the volume at
`/var/lib/docker/volumes/act-toolcache/_data`. The guard correctly refused it;
see `enrollment-release26-controller-recipe-red.json`. New bindings use exact
recipe SHA256 `f6f345602dcd3a4651d99c771dc5ee918979717bc74511c2225adfcc87669818`,
computed from the unchanged pinned producer script and actual serve target.
Recomputing the historical target reproduces the old digest. Recovery/finalization
use a strict known-recipe-to-target map so historical protected sources retain
their exact old recipe; unknown recipes still refuse. Evidence:
`native-serve-recipe-correction.json`.

The same actual controller proof is now GREEN. It created and protected the first
native engine, retired it through production recovery/cleanup, admitted a fresh
engine with the identical warm generation, retired that engine and released its
claim normally. Independent checks preserved original tool bytes/mode and the
legacy refusal file. Exact private outer containers and volumes, including every
inner Docker object, were removed. Evidence:
`enrollment-release26-controller-warm-green.json` and
`enrollment-release26-controller-recipe-corrected.log`. This proves real ordinary
policy/controller admission and empty-update retirement, not unchanged workflow
execution, new completed updates or whole-machine/class storage acceptance.

The reviewer found a raw transport bound issue in the new preparation receipt:
checked() trimmed whitespace before parsing. A real synthetic-Docker response with
5,000 whitespace bytes demonstrated RED; raw bounded capture, UTF-8 validation
and closed parsing now reject both oversized leading and trailing whitespace,
while accepting the ordinary receipt. Two focused tests pass; see
`enrollment-source-receipt-bound-{red,green}.log`. This hardening followed the real
controller success and preserves its valid receipt contract. Sole primary code
review is clean; the complete exact-source Bosn gate is still required before
any push. Production planner native policy remains unset pending retained-source
reclamation and the remaining original acceptance requirements. #508 stays open;
no other #693 child has started.

### Retained enrollment snapshot reclamation candidate

The next regression demonstrated that successful enrollment retained the entire
legacy payload and unfinished private stages: the snapshot still contained five
root entries rather than just its ownership marker. The unpublished act2 branch
`feat/enrollment-snapshot-reclamation` now reclaims that payload only after Ready
is committed, the native selection metadata validates, and the original legacy
refusal fence remains intact. Fence-only admission never reclaims source data.

Deletion traverses no-follow directory descriptors, refuses directory mount
boundaries, checks cancellation for each entry, bounds the descriptor stack,
and processes at most 4,096 entries per traversal pass under the existing
15-minute enrollment deadline. Interrupted passes resume using the original
root identity marker; the small marker directory remains as the ownership
witness. Symlinks are removed without following their targets. Successful
completion waits for directory synchronization; failure is a partial enrollment
response, not proof of reclamation. This does not complete allocated accounting
or machine-wide pressure policy.

Review caught a liveness defect in the initial depth refusal: legacy private
stages can be deeper than bootstrap's public payload validation. An 80-directory
private-stage regression reproduced permanent admission failure. The correction
lifts a verified deep subtree to a unique sibling inside the same owned snapshot
using no-replace rename, then restarts traversal from the root. This preserves
bounded descriptors without rejecting accepted private depths or placing data
outside the ownership witness. Evidence:
`enrollment-snapshot-deep-{red,green}.log`; the final expanded test additionally
requires only the ownership marker to remain.

The retention regression is GREEN. All focused `TestToolEnrollment` tests pass,
including external-target preservation, native reader validation, refusal before
durable Ready, a three-entry work budget, cancellation without further deletion,
and resumed deletion. Full repository lint reports zero issues, cache/tool/CI
command tests pass, and the final standalone full artifact-cache race suite
passed without excluding the unprivileged recovery tests. Sole-primary review
is clean. No new dependency release,
Bosn pin change, production activation or issue closure is claimed for this slice.

The first full race invocation failed in existing UID-switch tests because Go's
private `/tmp/go-build…` directory prevented executing its test binary as the
unprivileged test user (`permission denied`). That failure is retained in
`enrollment-snapshot-reclamation-race.log`. The final candidate passed
as a standalone race binary with mode 0755 from the artifact-cache package
directory, preserving the same suite and its old-recipe relative fixture paths;
see `enrollment-snapshot-reclamation-race-standalone.log`. Latest package lint
still reports zero issues after the deep-tree and completion-check changes.

The reviewed candidate is committed at
`769632ff535f8f2fa6d346d8bd728b5e17eb139b` in
[act2 #84](https://github.com/zackees/act2/pull/84). Required remote checks are
running; merge and exact-default-commit release verification remain outstanding.
Source and validation-log hashes are recorded in
`enrollment-snapshot-reclamation-evidence.json`. Bosn remains unpushed on its
existing integration branch and continues to pin verified act2.26.
PR checks run `39769650975` is currently in progress on that exact head;
Codespell run `39769650981` passed. The normal PR watcher is running with
evidence in `enrollment-snapshot-reclamation-pr-watch.log`; an observation
timeout must not cause duplicate workflow dispatch or a replacement watcher.

An additional actual Docker nested-volume proof passed on the committed,
unreleased act2 candidate. After phase-zero cutover, a separate task-owned volume
was mounted inside a private unfinished snapshot subtree. Complete refused the
mount boundary with a partial response; it did not claim enrollment success.
After removing that helper/mount, the same durable history completed reclamation.
Independent checks confirmed only the original ownership marker remained,
`preserved-tool` bytes and mode 0751 survived in the selected native generation,
and the separate volume still contained `unrelated-volume`. Both owned volumes
and every helper container were proved absent after cleanup. Evidence:
`enrollment-reclamation-mount-proof.{py,json,log}`, fixture
`bosn693-reclaim-mount-d4368ce30310`, candidate binary SHA256
`519fa68d7feec57e6e2335cde5a21297e2d3945a0e5eec832171a0235b918aa3`.
The first candidate build refused the read-only checkout's VCS stamp; the
explicit `-buildvcs=false` rebuild is recorded separately. Candidate source SHA
and executable hash are evidence for this fixture, not a released artifact claim.

The normal PR watcher reached its 300-second observation timeout (exit 4).
Authoritative GitHub state still shows the same checks run `39769650975` live:
Linux tests and snapshot build are in progress; lint, Windows and macOS passed,
as did Codespell. No workflow or watcher was restarted. Issue #508's latest
execution status now records these published prerequisites and unfinished scope.

### Ordinary retirement with a controlled completed update

The real-controller fixture now has a second opt-in case. After genuine native
admission/protection, it writes one completed mode-0751 tool and one unfinished
install into the producer-mounted writable overlay. Normal production retirement,
publication, source disposal and claim release must finish before a fresh engine
can be admitted. That engine must have a different generation, preserve the
completed bytes/mode, exclude the unfinished install and acquire its own durable
protection. The original unchanged-data fixture remains separate.

This is explicitly a controlled source fixture, not full workflow execution.
Its companion independently checks original and updated tool payloads in the
replacement generation
and removes all resources in its private Docker daemon. Initial compile passed;
Clippy caught a 118-line helper. Extracting isolated-backend validation restored
the existing 100-line gate without suppressions. Final compile passed in 17.18s,
and service library/tests Clippy passed in 15.11s. Evidence:
`enrollment-completed-update-{build,clippy}{,-final}.log`.
The actual released-act2.26 controller execution passed; see
`enrollment-release26-completed-update-proof.{py,log,json}`. Fixture
`bosn693-native-controller-8475c14802f8` normally retired both engines, completed
production publication/disposal/reference release and released both claims.
The frozen generation changed from `b0f186f8…b6a5f` to `15f4976e…921b3`.
The fresh engine and independent read-only helper preserved the completed update
and mode 0751, excluded the unfinished install, and preserved original warm tool
bytes/mode. The Rust executable hash was unchanged before/after
(`f8116218cad1c7729c378d8a9a83c7aceb0e09d554dac48983df715ee8c63860`).
Every private outer container and both host volumes were proved absent, removing
all inner Docker resources. This proves controlled new-update recovery through
the real controller, not unchanged workflow execution or whole-machine limits.

Act2 #84 subsequently passed all six required checks and merged normally at
`aa39a347042a90bf37e5c558725d62c88748bd80`. There were no review threads.
The external checkout is clean on rebased `master`; its reviewed and merged trees
match, and its feature branch was deleted locally/remotely. The original Bosn
checkout fetched/rebased `origin/main` with its WIP autostash applied successfully;
the three pre-existing stashes remain unchanged. Exact default-branch full CI
[39771550138](https://github.com/zackees/act2/actions/runs/39771550138) is running
on that merge SHA. Act2.27 guard/verifier/runtime-smoke scripts are prepared, but
no tag or release has been created and Bosn still pins verified act2.26.

### Native tool accounting coverage and act2.27 publication

Exact default-branch full CI `39771550138` passed all five jobs on
`aa39a347042a90bf37e5c558725d62c88748bd80`. The act2.27 guard dry run and
revalidated publish run passed clean-default-checkout, exact CI coverage and
absent tag/release checks. Annotated `v0.2.89-act2.27` was pushed through the
existing release workflow. Release run `39773009042` is in progress; no artifact
verification or Bosn pin update is claimed yet.

A new accounting regression found that the toolcache component reported only
the 14-byte legacy refusal file, omitting native toolstore data, enrollment
history and retained snapshot data. The actual RED test expected 1,065,002 bytes
from an independent `(device,inode)` inventory. An earlier short-name `--exact`
attempt ran zero tests and is not counted as RED. Evidence:
`native-tool-accounting-red{,-attempt}.log`.

The existing toolcache class now samples legacy/native roots, fixed enrollment
history and canonical enrollment snapshot names in one du invocation per metric,
deduplicating hard links across those roots. No wire enum changed. More than 32
snapshot roots reports unknown class usage and a partial sample; it never reports
zero or a knowingly truncated class total. The separate volume total is not
constructed by adding class samples. Shell glob expansion and filesystem walks
are not newly bounded by this argument cap; machine-wide cross-class union,
traversal limits and pressure policy remain unfinished.

The regression is GREEN; cache-usage tests passed 29 with five explicit hardware
fixtures ignored. The 33-snapshot overflow test passes. Review caught that direct
Linux-shell fixtures must not execute against macOS/BSD du; the new fixture
module is Linux-only, matching the Docker helper's execution platform. An actual
pinned-engine shell/du proof independently counted seven unique inodes and
exactly matched 274,465 apparent and 282,624 allocated tool-class bytes. The
external volume was preserved and all task containers/volumes were proved absent.
Evidence: `native-tool-accounting-docker-proof.{py,json,log}`, fixture
`bosn693-native-accounting-5ec7a0934ee1`, script SHA256
`d79b9c859bf0de5187538bb4a013b1ae8b26129ad08721697fa44e0bb3f443f5`.
Sole-primary static review is clean; final service library/tests Clippy passed in
20.75s (`native-tool-accounting-clippy.log`). This class-coverage
fix is not a claim of completing whole-machine accounting or issue #508.

Act2.27 release `39773009042` completed successfully. All eleven archives match
checksums, independent GitHub digests/sizes, executable architecture/header and
embedded version. Linux version/capability execution passed with exact helper
cleanup, and the annotated tag was verified to peel to the tested merge SHA.
Evidence: `act2-27-{release-guard,verification,runtime-smoke,release-provenance}.json`.
The Linux archive SHA256 is
`c00daa74122f652f0fcbc6612c0711fe9007523c417bb84894180091dde27b1a`;
the extracted executable SHA256 is
`ddee7bc657330634332f3bc1e70d44e7254ba32d09632997938facf046a2a739`.
The Bosn candidate now pins verified act2.27. Its focused rebuild passed in
17.28s with all six pin tests passing. Final Clippy and an actual released27
completed-update/controller/accounting proof are next; no Bosn push, production
activation, full exact-source gate or issue closure is claimed yet. The exact
owned PR84 watcher file was preserved in task evidence and removed with safe-rm.

The final tracked-accounting proof initially tripped the existing 100-line
Clippy gate at 101 lines. Update writing and verification were separated into
focused fixture helpers; no limit was raised. The final rebuild/pin tests passed
(six tests), and service library/tests Clippy passed in 13.61s. Evidence:
`enrollment-release27-controller-build-final.log` and
`enrollment-release27-clippy{,-final}.log`.

The actual released-act2.27 combined proof passed with exact cleanup:
`enrollment-release27-completed-update-proof.{py,json,log}`, fixture
`bosn693-native-controller-ed4adc2d47b1`. It exercised genuine migration,
completed-update publication, fresh-engine recovery/protection, normal retirement
and claim release, then Bosn's actual journaled cache measurement helper. Its
toolcache class exactly matched an independent read-only inventory of 45 unique
`(device,inode)` pairs: 250,217 apparent bytes and 270,336 allocated bytes.
The original and completed tool payloads/modes survived, unfinished data was
excluded, and only the original legacy snapshot ownership marker remained.
The test executable hash was unchanged before/after
(`838ff407a1856332cc13946d298f1c02739d587a44b6b6657696631e246e4c0d`).
Every outer fixture container and both host volumes were proved absent, removing
all inner resources. This remains a controlled-update/tool-class proof; production
native activation, full workflow acceptance and whole-machine policy are pending.


### Full local gate: apparent-size portability correction (2026-10-22)

The first full gate on `e85cc6ebf94266fe17e3772a1e7c1b6a49b96ec7`
failed: Rust run `af361760-4ef8-40a4-9e61-f45cc39dc8ae` passed 420
nextests and failed the new native accounting inode test. Linux run
`ea8ecb53-8578-4273-85b4-ffbe2bfc5d89` passed. The apparent-byte
expectation exceeded the sample by exactly three directory lengths (12,288
bytes); no push was made.

An isolated, read-only-root probe of the exact runner image `4f2d5083a9d1`
confirmed GNU coreutils 9.4: a directory with `stat` length 60 containing one
three-byte file reports three apparent bytes through `du -sbc`, and 4 KiB
allocated through `du -skc`. The local build harness uses GNU coreutils 9.1,
and the pinned engine uses BusyBox; both include directory lengths in apparent
bytes. The probe container `bosn693-du-semantics-probe` was automatically removed.

The independent inode oracle now records directory lengths separately and
accepts exactly either complete apparent-byte convention. File and symlink
lengths, cross-root hard-link deduplication, exact allocated bytes, external
symlink preservation, and overflow refusal remain checked. This changes only
the test: production measurement behavior is unchanged. Both focused accounting
tests passed in the GNU 9.1 harness after the correction. A new full gate on the
corrected commit is still required; the earlier failed gate is not acceptance.


### Ordinary native execution activation and real workflow acceptance (2026-10-22)

PR #706 merged at `65842457896601755eb57e29c5298374da418bc2`.
Its corrected full local gate passed in 253s: 1,155 Rust nextests passed,
52 configured skips; Linux passed in 174s. Stamped commit
`0818f42a896f1e57a31056b745296f68c44e5845` independently passed
GATE-003 verification before push. Required remote checks passed in
`39778264520` (nine successes, five configured skips), with no outstanding
review threads. The original checkout rebased onto main and the owned branch
and exact watcher log were removed. No Bosn worktree was created.

The next ordinary-runtime regression reproduced the remaining bypass: planning
still set `native_tools: None`. Evidence: `production-native-planner-red.log`,
one executed test failed at the explicit enrollment assertion. The planner now
selects native enrollment with a 64 GiB logical generation payload bound and
validates its backing before admission. This bound refuses an oversized warm
source; it is not an allocated machine ceiling and does not authorize dropping
warm data. Memory-backed plans explicitly refuse before engine creation,
claim acquisition or source fencing. The planner tests pass.

An exploratory shared-lease extension of the synthetic planner test exposed
that FakeBackend's retirement identity check omits native-generation creation
labels; after correcting that experimentally, the registry correctly refused
finalization while the fake still retained its recovery source. Those temporary
fake changes were reverted. The final planner test checks policy selection and
absence of planning side effects; retirement acceptance uses the actual Docker
backend below. The failed exploration remains recorded in
`production-native-planner-cleanup-red.log`; the intermediate green run did not
prove durable retirement and is not used as such.

The actual ordinary-runtime proof passed using released act2.27:
`production-native-workflow-proof.{py,json,log}`, fixture
`bosn693-native-workflow-125aa6234d2e`. It used a private Docker daemon,
the genuine machine claim and registry, the pinned runner image and verified
archive, and unchanged two-job workflow YAML across two submissions. The first
job preserved the enrolled legacy payload and wrote a completed tool (mode 751)
and an unfinished install; the dependent job reused the completed tool. After
normal engine retirement, native publication/source finalization and claim
release, the same workflow ran on a fresh engine. Before writing again it
required the recovered completed payload/mode and absence of the unfinished
install. Both runs succeeded with two completed jobs each and removed run scopes:
`b6943e4e-2f4a-4983-8a8e-11e5faafdcdb` and
`00e34e6c-fee1-4449-85b3-41c083c4c6a1`.

The two actual engines were
`bosn-act-839e0f0a-6a27-434b-91fd-5f093966ce1e` and
`bosn-act-1e10d4a8-99bb-4cdd-9802-9024978b145e`. Both had durable
recovery protection and both claims were normally released. The fixture passed
in 196.33s and the executable hash was unchanged before/after execution.
All outer fixture containers and both host volumes were proved absent, removing
the private daemon's inner resources. This proves the controlled real two-job
workflow case; original full workload, concurrency, crash/restart, machine
accounting/ceilings, bounded history and coordinated archive acceptance remain
open. Archive routing remains Legacy. No issue closure is claimed. The candidate
still needs Clippy, primary review and the full exact-source gate before push.


The CI-focused service suite passed: 320 tests, 21 explicitly ignored hardware
fixtures (the new real-runtime fixture was executed separately above).
`production-native-ci-tests.log` records the 123.18s run. Clippy initially
rejected `.err().expect()` in the memory-refusal regression. That assertion now
uses an explicit typed result match; no lint was suppressed and EnginePlan's
production traits were unchanged. The fence/drain ordering in the cache layout
documentation was corrected during primary review. Final Clippy and the full
gate remain pending at this recording point.


Final service library/tests Clippy passed in 16.31s with `-D warnings`
(`production-native-clippy-final.log`). The sole primary review found no
remaining production finding. The full local gate is the remaining pre-push
requirement; all broad original #508 requirements above remain in force.


### Bounded registry recovery work in progress (2026-10-22)

PR #707 merged at `663bebc3c7170b7943ef04b2adf59bf13b66d04c` after
the exact full local gate passed in 559s including queue time. Rust passed 1,157
nextests with 53 configured skips; Linux passed 264 with 10 skips. Stamped
`9952d3e145dc9ceb1d18c870e6539b745e84691d` independently passed GATE-003
before push. Required remote checks passed in `39781729011` (nine successes,
five configured skips). The original checkout rebased onto main; its owned
branch and exact watcher log were removed. Ordinary native recovery is now
selected on main; archive routing remains Legacy.

The next uncommitted work replaces history aggregation with typed, indexed
current authority for engine/helper records. The exact production SQL cost
regression ran in the isolated container: with 100 engine identities and 65
returned rows, 10,000 and 100,000 historical snapshots exceeded the 100,000-VM-
instruction budget. The same regression passes with current-state queues.
`recovery-query-budget-red-executed.log` and `recovery-query-budget-green.log`
record actual test execution; earlier host attempts lacked pytest or were refused
by the host-test guard and are not RED evidence.

The schema-v6 candidate atomically updates current authority with journal writes,
retains three snapshot audit events per identity, and reads pending engines,
native recoveries and helper exclusions through typed indexes. V5 upgrade holds
the sole-writer fence, sets v6 before backfill to exclude older binaries, and
walks reverse event-ID pages of at most 128. The cursor commits with each page;
normal reads/admission remain gated until it completes. Daemon startup calls
those pages on a blocking worker and yields between pages before binding IPC.
Latest snapshots validate typed identities and owners. Superseded history never
replaces newer authority. Explicit imports use that same current-state path.

An isolated 9 MiB unrelated-event regression initially exhausted the migration
page's 8 MiB result budget. Backfill now fetches managed payloads only within
that bounded ID window; the regression passes and preserves the unrelated data.
Evidence: `registry-migration-unrelated-large-red.log` and
`registry-current-migration-corrected-tests.log` (four tests passed, including
interrupted/reopened upgrade, invalid latest authority and 500-update churn).
Managed JSON now has engine/helper byte checks before deserialization/storage.
V6 validation requires its columns and exact canonical queue-index definitions.

The first registry integration run passed 56 and failed four: two tests still
expected malformed snapshots to be stored before read-time rejection, and two
encoded the old current/future schema numbers. Invalid snapshots now refuse at
the write boundary and those tests also require existing authority unchanged;
the future-version fixture is v7. The corrected run passed all 17 library and
60 integration tests (`registry-current-registry-corrected-tests.log`). The legacy
compatibility/version tests moved into `act_lifecycle/record_validation.rs` when
the original file reached 1,000 lines; no gate was raised. Registry/service
library/tests Clippy passed in 22.28s (`registry-current-first-clippy.log`).

The candidate now limits new engine/helper authority to 10,000 identities and
retains terminal authority for seven days. Existing identities can still recover
at capacity or below the replay floor. A trusted background observation clock
drives batches of at most 64; a durable composite cursor advances past protected
rows. Pending helpers, engine references and leases protect their authority.
Terminal abandonment waits for exact source absence and recovery expiry;
unreleased publication remains protected until acknowledgement. Legacy terminal
proofs without the optional newer disposal marker remain collectible.

Two admission/expiry regressions failed before implementation and passed after.
Review found that admission incorrectly used intent creation time as a GC clock:
the future-date regression failed, then passed after removing collection from
admission. Only explicit trusted maintenance now advances collection. The registry
suite passed 23 library and 63 integration tests; registry/service library/tests
Clippy passed in 26.19s. The actual SQL budget tests passed three cases.

The actual service startup test upgrades a previous-layout v5 database containing
400 snapshots, verifies v6 authority and its three-event audit tail through a
read-only connection after IPC becomes ready, then stops and restarts the daemon
and verifies them again. It passed. A bounded one-record maintenance test also
proves a protected parent does not starve a later eligible helper. Its first
fixture used an absence timestamp before the required late-create window and
correctly refused; the corrected fixture and all three retention integration
tests passed. Docker was unavailable inside this unit-test container; this is
registry/startup evidence, not new physical container evidence.

The follow-up daemon suite passed all 48 selected tests in 15.75s; final
registry/service library/tests Clippy passed in 21.01s. The sole primary reviewer
reviewed every Rust, Python and documentation change and found no remaining
concrete finding. Python lint initially rejected two long test lines; both were
split without changing the query or VM budget. File-length and include-base
guards pass.

#### Recovery capacity correction before push

The first whole gate ran on `a6afde0bf7f735b93a1da5bfeabdc8cc3a22b1a9`.
Rust passed in 326s. Linux passed 266 tests with ten skips and failed one old
Python import assertion expecting schema 5; the actual imported database correctly
reported 6. The expectation now matches the new registry schema. That failed gate
does not authorize a push.

A final capacity audit found that ordinary authority at 10,000 records could
block a fresh helper required to recover an existing engine. The former test
covered updates to existing helpers, which did not prove new-helper admission.
An actual typed fixture filled ordinary capacity, then tried twelve sequential
completed recovery attempts; it failed before correction and passed afterward
(`registry-recovery-reserve-red.log`, `registry-recovery-reserve-first-green.log`).

Validated local recovery phases now have a separate finite reserve: at most eight
retained helper attempts per engine and 90,000 total authority identities. An
all-helper engine index bounds lookup. Only `begin_cache_helper` after exact
existing-engine/phase validation can spend this reserve; generic events,
maintenance and foreign finalization cannot. A ninth attempt can recycle one
already-Removed helper with the identical frozen intent. Exact immutable-ID
absence or the late-create absence proof is required before Removed; the parent's
publication/release authority remains separate. Recycling advances durable replay
protection before deleting the old audit/current rows in the same transaction.
The system observation clock independently excludes future-dated recycling.
Pending/uncertain attempts are never evicted: eight uncertain attempts refuse
further admission and keep source protection intact. Existing over-cap legacy
authority is preserved rather than dropped.

The corrected registry suite passed 23 library and 65 integration tests; final
registry/service library/tests Clippy passed in 19.70s. The reserve exhaustion
test also rejects future-clock admission without changing the replay floor.
The sole primary reviewer confirmed the capacity finding resolved and found no
new concrete finding. A fresh whole exact-source gate remains required.

This remains local source, not a shipped schema upgrade or a completed
storage bound. Physical database/WAL accounting and ceilings remain unfinished.
Deleting historical rows does not shrink old allocated SQLite files. Other event
families retain their own unfinished storage policies. No full gate, push, PR or
issue closure is claimed for this candidate.

### Physical registry allocation and reclamation candidate (2026-10-22)

PR #708 merged at `fc2ae6ad16e03b44d9cde8e48b69f403cd699fd5`.
Its corrected whole local gate passed in 271s: Rust 1,173 passed with 53
configured skips; Linux 267 passed with ten skips. Stamped source
`ea6a2a61ed6ed85cee049c132be44c6266fc14cc` independently passed exact
GATE-003 verification before push. Remote run `39792026548` passed nine checks
with five configured skips and no unresolved review threads. The original
checkout rebased onto clean main after merge; the owned local/remote branch and
exact watcher log were removed. User stashes and the unrelated worktree remain.

The next candidate is uncommitted on `feat/508-registry-storage-budget`.
The real-WAL regression performs 100 actual current-authority updates, observes
allocated WAL blocks, runs history maintenance and requires allocation to fall
while the latest authority remains intact. It failed before implementation and
passed afterward (`registry-wal-allocation-red.log`,
`registry-wal-allocation-first-green.log`).

New typed storage samples cover only the database, WAL and shared-memory files,
deduplicate device/inode pairs, distinguish file length from allocation, and make
unknown components produce unknown totals. Symlinked/nonregular files are not
treated as owned storage. Samples read metadata only; stable-inode size/allocation
changes use the larger before/after observations, while identity/type/read
failures produce partial samples. Reports expose no paths or raw OS errors. The Windows facade's
possible logical-size fallback is explicit. `bosn doctor` now includes this
sample in its JSON/text report. This is not a cross-volume machine census.

Ordinary new CI authority now checks a 1 GiB admission budget against both actual
file allocation and separately named SQLite page-count projection plus WAL/shared-
memory extents. The projection catches newly allocated pages before a transaction
flushes them and accounts for the WAL alongside database growth. Existing current
records and the previously validated local recovery-helper reserve stay admitted;
imports and unrelated event families have separate unfinished policies. Thus this
is admission pressure control, not a hard whole-file quota: protected progress,
one admitted record and unrelated families can exceed the threshold. A writer's
explicit override can only lower the default limit and is not persisted.

The actual allocated-pressure regression failed before enforcement and passed
afterward; the single-transaction regression separately proves unflushed new
pages cannot bypass admission. The existing full-capacity fixture now also lowers
its allocation limit and still completes twelve fresh recovery attempts, while
retaining replay protection. Evidence: `registry-allocated-pressure-red.log`,
`registry-allocated-pressure-green.log`, `registry-storage-suite-clippy.log`.

Fresh, schema-free databases enable incremental vacuum. The first attempt set
the pragma after the kernel's WAL setup and the test observed mode 0, not 2.
The corrected creation path rebuilds only that newly created empty database
before installing tables; it never runs a full VACUUM on an existing registry.
Incremental maintenance frees at most 64 freelist pages. Existing legacy files
without pointer maps retain their high-water allocation and must be reported
truthfully; their safe compaction path remains unfinished. SQLite documents the
[incremental-vacuum prerequisites and page limit](https://www.sqlite.org/pragma.html#pragma_auto_vacuum)
and [WAL size/reset behavior](https://www.sqlite.org/pragma.html#pragma_journal_size_limit).

Reader-held WAL tests prove a zero-busy checkpoint reports blockage without
waiting, preserves the reader's old snapshot and restores the ordinary writer's
five-second busy policy; after the reader closes, truncation succeeds. Sparse,
hard-link, symlink and replaced-path tests pass. The corrected fresh-database
test proves actual allocation falls while more free pages remain for later
passes. The first physical suite passed four and failed the mode assertion;
after correction all five passed.

Primary review found that zero busy timeout alone does not bound checkpoint I/O:
TRUNCATE can still copy/sync a large WAL on the registry actor. The actor-side
candidate now defers checkpoints above a measured 4 MiB WAL length, reports
`checkpoint_deferred`, and a real 5 MiB fixture verifies no false reclaim claim.
The independent background large-WAL path was still missing at this point; the
following correction implements it before shipping.

The current isolated registry suite passes 32 library and 65 integration tests;
registry/service library/tests Clippy passes in 36.64s. No full local gate, commit,
push, PR or #508 closure is claimed for this uncommitted storage candidate.

#### Independent checkpoint owner and controlled IPC proof

The automatic-checkpoint regression failed with SQLite's default 1,000-page
setting, then passed after disabling automatic checkpoints on both new and
reopened writer connections. All writer constructors now finish through one
guarded setup path. It establishes connection-before-lease drop ordering before
any fallible secondary initialization, then opens the dedicated checkpoint
connection while retaining the same writer fence and rechecking file identity.
The stored path is canonical, so later working-directory changes cannot retarget
admission or reclamation. Both connections disable automatic checkpoints.

The checkpointer can be taken once through an internal actor command; there is
no client wire operation. Its connection and shared writer-lease clone live on
one blocking worker even between passes. An async 15-second scheduler submits
at most one pass through a one-slot channel. PASSIVE copy and any subsequent
zero-busy TRUNCATE run there, outside the registry request queue. Cancellation
closes the command channel; shutdown asynchronously joins the running pass and
connection close. The registry actor also closes its own connections on a
blocking worker. Lease field ordering keeps writer exclusion until SQLite closes.
Large-copy work and shutdown latency are explicitly not time-bounded; neither
uses a synchronous wait on the async executor. Diagnostics log state transitions.

The actual 5 MiB library fixture proves WAL reclamation, ordinary writer timeout
preservation, lease retention after dropping Registry, and successful reopen only
after checkpointer close. The first readback tried returning the whole 5 MiB
event through the existing 1 MiB page budget and correctly failed. The corrected
test compares the entire known payload inside SQLite and returns only a bounded
scalar result; no result limit was widened.

The full Service fixture starts with a real 5 MiB WAL held by a read-only
snapshot. A test-only gate stalls the actual native checkpoint worker. Authenticated
IPC status still responds; shutdown waits while the worker is held, and a second
writer is refused. Releasing the gate allows the real checkpoint, connection
close and lease release to finish. Reopen verifies exact payload preservation and
zero allocated WAL bytes. Both corrected fixtures passed
(`registry-background-checkpoint-corrected-tests.log`). This is actual SQLite/IPC
lifecycle evidence; Docker is unavailable in that unit container and no new
container/cache-class acceptance is claimed.

The primary reviewer found no remaining Rust blocker after this correction;
the documentation finding about the formerly missing worker and conservative
samples is corrected here. Broad validation and the whole exact-source gate must
still pass before push. The original #508 physical machine/cache-class scope,
legacy offline compaction, other event-family policies and full workload acceptance
remain open.

The first broader background run passed 34 library tests and failed the original
WAL test because it still expected row maintenance alone to shrink a now-large
WAL. With automatic checkpoints disabled, the actor correctly deferred it. The
test now runs row maintenance and the separate checkpoint phase, preserving its
allocation-reduction/latest-authority assertions. The corrected broader run
passed 35 registry library tests, 65 integration tests, all 49 daemon tests and
registry/service Clippy in 19.60s
(`registry-background-storage-corrected-suite-clippy.log`). The legacy test also
proves free pages remain visible without claiming old high-water allocation was
reclaimed or removing the unrelated retained event.

A final pending-allocation regression used a real uncheckpointed database and a
budget between its current physical allocation and projected database-plus-WAL
footprint. It failed because only database page count was charged. Admission now
includes sidecar extents in the separately reported `projected_bytes`; actual
allocation stays a distinct measurement. The same regression and all twelve
storage tests pass (`registry-pending-wal-budget-red.log`,
`registry-pending-wal-budget-green.log`). The single-transaction test still
requires physical allocation below the limit, actual page growth, and projection
at the limit before refusal; no check was dropped. Final exact-source validation
is required after this correction.

Final focused validation after the projection correction passed all 36 registry
library tests and 65 integration tests. Registry/service library-and-test Clippy
passed in 18.34s; file-length and include-base guards passed
(`registry-storage-final-focused-validation.log`). The previously recorded 49
daemon tests also passed. The whole exact-source local gate remains required
before push; these results do not close #508.


### Coordinated archive routing continuation (2026-10-22, unmerged)

PR #709 merged at `a10152fb29833be643e6be6797de8c73a3209df7` after
its 527-second whole local gate (1,187 Rust tests passed, 53 configured skips;
Linux passed). Stamped candidate `d69b116f3c4158fee3936bf6603f09c24065d52b`
independently passed exact GATE-003 verification before push. Remote run
`39802261439` passed 11 jobs with three configured skips and no review threads.
The original checkout returned to clean main and rebased; candidate and merged
trees matched. The task branches and original watcher log were removed, and the
three pre-existing stashes were preserved. Physical machine/cache-class bounds
and the other original #508 acceptance items remain open.

The next ordinary-planner regression ran and failed: production still selects
`CacheRoute::Legacy` (`archive-production-route-red.log`). The first command used
an incorrect exact filter and ran zero tests; that result is not RED evidence.
The corrected command ran one test and failed its explicit coordinated-route
assertion. This regression remains RED until production orchestration is wired.

The unmerged act2 branch `feat/508-archive-enrollment`, in the existing external
checkout, adds a per-namespace journal outside the destination, atomic legacy
source fencing, explicit stopped-source publication, and admission validation.
The old directory is retained privately; the old path becomes a refusal file.
Fence alone never claims existing directory-FD writers have stopped. Completion
requires caller-provided continuous writer exclusion, preserves completed warm
archives through the existing verified import, and reconciles an existing
publication using its receipt plus current inventory. Ready admission must not
bootstrap missing state. It preserves frozen import bounds and does not delete
retained sources. None of this is yet released or selected by Bosn.

Initial focused tests found cold sources lacked the archive storage layout. A
first correction used the public coordinated audit on a legacy source, which
correctly refused absent transfer coordination; it now uses the same read-only
locked legacy audit as import. The directory-FD fixture initially addressed the
wrong archive filename; using Storage's actual filename fixed that fixture.
Warm preservation, legacy reopen refusal, cold absent/empty enrollment, retained
FD mutation refusal, lost publication acknowledgement, missing warm state and
policy/unowned-destination refusal then passed. A command test performs real
HTTP reserve/upload/commit and verifies hits through two fresh destination
servers. Earlier focused package/command coverage passed, but those results
precede the final review corrections.

Primary review identified two defects: persisted device/inode numbers would
refuse unchanged volumes after remount, and an existing untyped Bolt file could
be initialized as fresh authority. The candidate now uses durable incarnation
markers in the destination directory and metadata, with inode comparisons only
within an operation; only a newly created journal may establish first authority.
Regression coverage includes recreated empty metadata, preserved-incarnation
file reopen, and an existing untyped journal. Final validation is pending.

The first broad package/command attempt failed environmental gates: the existing
retirement permission test could not execute its uid-65534 subprocess from Go's
private build directory, and existing command tests had no Docker socket. That
run is not a passing full suite; its chained race test did not execute
(`archive-enrollment-package-race.log`). The focused/race rerun is tracked
separately. No gate, release, push or production acceptance is claimed.

Interrupted import without a destination deliberately refuses explicit private
stage reconciliation rather than creating repeated unknown copies. Automatic
bounded stage reconciliation, retained source expiry/accounting, orchestration,
older-writer draining and unchanged full workflow acceptance remain required.
This candidate does not close #508 or advance another #693 child.


The review-corrected focused package/command run passed in 48.700s and
4.205s; race coverage of archive/tool enrollment passed in 33.715s
(`archive-enrollment-reviewed-tests.log`). The earlier complete artifact-cache
package then passed in 284.735s using a Go execution wrapper that makes only its
generated temporary binary directories traversable for the existing uid-65534
subprocess; Go owns and removes those directories. This pass predates the next
publication reconciliation change and is not current full-suite acceptance.

Primary review then found a first-Ready gap after lost import acknowledgement:
a valid empty destination plus the historical warm receipt could be accepted.
The new actual lost-reply/replaced-store regression failed
(`archive-lost-ack-empty-inventory-red.log`). Initial publication reconciliation
now verifies the retained source fingerprint, reconstructs the frozen selected
imports, and compares every destination metadata row and archive checksum with
the source, including sampled original receipt checksums. Later Ready admission
permits normal retention and new uploads. Focused package/command coverage passed
in 50.023s/2.349s and archive enrollment race coverage in 10.202s
(`archive-publication-reconciliation-green.log`). The destination transfer lease
now additionally spans initial verification, durable incarnation establishment
and Ready commit, excluding maintenance between validation and admission. Final
tests/review for that ordering change remain pending. The Bosn planner is still
RED and production routing remains Legacy; no release or push has occurred.


Final ordering tests passed (artifact-cache focused 35.730s, commands 3.151s,
archive race 4.207s); primary review found no remaining concrete issue. Lint
then rejected the verification function's cyclomatic complexity, one redundant
conversion and two fixture taint warnings. The checksum/metadata loop was split
without dropping comparisons, the conversion removed, and fixture I/O corrected;
no rule was suppressed. The first extraction rerun caught an undefined err
binding, which was corrected. Final golangci-lint on artifact-cache and commands
reports zero issues (`archive-enrollment-lint-final.log`). Final focused tests
passed in 44.629s/2.181s and archive race coverage in 6.422s
(`archive-enrollment-final-validation.log`); the same primary reviewer confirmed
all comparisons and lease ordering survived extraction. The complete package
pass above still predates these changes. No current full gate, push, release or
Bosn production activation is claimed. Interrupted-stage reconciliation remains
the next required recovery change before this candidate can be shipped.


### Bounded archive import-stage recovery continuation (2026-10-22, unmerged)

The existing act2 checkout is checkpointed at `dd07afd` on
`feat/508-archive-enrollment`; the original Bosn checkout remains on
`feat/508-archive-routing` with its production planner regression still RED.
No new Bosn worktree, push or release was created.

A new regression committed actual `ImportStarted` intent without creating a
stage, then requested completion. It failed with the manual-reconciliation
refusal (`archive-intent-before-stage-red.log`). Enrollment now derives one
private stage from its frozen random identity rather than making a new random
stage on each retry. The stage factory takes the cohort lease and verifies its
ownership witness before clearing partial content. Missing witness plus
nonempty content refuses; an empty directory can finish interrupted initial
witness creation. The root witness is preserved throughout partial cleanup.
Traversal uses the existing no-follow descriptor walker, refuses mount
crossings, bounds the descriptor stack, and caps each reclamation pass at
100,000 entries. Exhaustion retains progress and retries the same stage.
Failed imports retain that one owned stage; successful publication consumes it.
Retained original source data and unrelated paths are never cleanup targets.

The ordinary import API keeps its existing random-stage and cleanup behavior.
A shared stage-policy boundary avoids duplicating import/checksum/publication
logic. The tool enrollment walker has only gained a root-marker parameter;
its original wrapper preserves the existing tool witness and behavior.

The intent-before-stage regression and focused archive/tool enrollment/import
coverage passed: artifact-cache 38.689s and commands 2.583s
(`archive-stage-recovery-first-tests.log`). New fixtures cover partial stage
content, preserving an external symlink target, bounded partial reclamation and
retry, and preserving an unowned nonempty stage. The old manual-only refusal
assertion was replaced by automatic recovery acceptance; recreated warm metadata
is still explicitly refused. Primary review found no concrete issue in this
continuation. Full artifact-cache, race and lint validation remain running in
`archive-stage-full-validation.log`; no current full pass is claimed yet.

This is recovery-component evidence. Production needs typed retryable contention
results, a source-fence/older-engine-drain/complete orchestration boundary and
current admission checks before selecting Cohort. The pinned released act2 does
not yet contain the new command. Required dependency CI/release verification,
Bosn rollout, pending-storage accounting/source expiry and unchanged realistic
workflow/restart acceptance remain open; #508 and #693 remain open.


The full stage-recovery artifact-cache suite passed in 182.225s and archive/tool
race coverage in 29.094s. Lint required restoring the two pre-existing
caller-selected-root path annotations at their extracted calls, then passed
(`archive-stage-lint-final.log`). No new rule was disabled.

A separate real contention regression failed because the new command did not
return a typed retryable result (`archive-contention-status-red.log`). Enrollment
now returns `retryable` for native lock timeouts and bounded cleanup continuation;
policy conflicts remain nonretryable. ImportReport retains typed internal causes
without changing the ordinary import command's JSON shape, which older Bosn
clients parse strictly. Focused tests passed45.485s/4.193s, archive race9.002s and
lint zero issues (`archive-recovery-contention-validation.log`).

The journal-lock regression then failed at1.005s, consuming its whole one-second
caller deadline (`archive-journal-contention-red.log`). Archive history now has a
250ms acquisition subdeadline; expiry maps to typed contention only while the
parent context is still live. Shared tool history behavior is unchanged. Final
focused package/command tests passed53.105s/2.367s and archive/tool race52.273s
(`archive-bounded-recovery-final-validation.log`). Lint caught loader complexity22;
extracting acquisition into `openArchiveHistory` preserved the authority checks.
Focused journal/contention/untyped-authority tests passed5.775s and lint reports
zero issues (`archive-journal-extraction-final.log`). The same primary reviewer
confirmed the extraction and found no remaining concrete issue. The earlier full
suite predates retry classification; normal dependency CI remains required.


Act2 PR #85 is open at `d1629f88dddefd8c21668e138ee5a0cf29d3e105`
(two reviewed local commits, clean external checkout). Dependency checks/review
remain pending. Nothing is released or activated in Bosn; original scope and
#508/#693 stay open. The original Bosn checkout contains only the in-progress
planner regression and living-spec updates on its feature branch.


### Bosn archive transport and inactive coordination (2026-10-22)

The typed Docker boundary validates bounded replies, exact source/cohort/
namespace/import bound, the frozen snapshot nonce and phase lifecycle before
returning Ready/Fenced/Retry/Refused. Only empty-output exit75 from the trusted
wrapper is a transport retry. Require uses a shared lifetime lease; Fence and
Complete use the exclusive nonblocking migration lease. Four focused boundary
tests passed (`archive-transport-first-tests.log`). This is command-boundary
coverage, not execution of released act2's new command.

The shared-engine path now calls archive preparation before run preparation;
Legacy returns immediately, so production remains unchanged and the ordinary
planner regression is still RED. For an explicit Cohort invocation it inspects
existing Ready state, fences new enrollment, drains other engines using the
existing ownership/liveness protocol, then completes publication. Every phase
verifies the lease; contention waits are async/cancellable. Unknown transport
errors never trigger fencing. Synthetic backend replies are explicit queued
phase results, not implicit successful enrollment.

Three coordination tests initially passed and one failed after observing actual
fake retirement because its assertion confused the fake removal log's names
with IDs. The corrected test retains both removal-log and engine-absence checks.
It resumes the same pinned future after an observation timeout, proving Complete
is not issued while the old fake engine is busy. All four corrected coordination
tests and four transport tests passed; service library/test Clippy passed24.18s
(`archive-admission-focused-validation.log`). Sole-primary review found no new
issue in this inactive coordination. Full immutable machine-policy agreement
must still precede fencing and be required on Ready admission before activation.
No real-engine claim is made by these synthetic tests.

Act2 PR #85's six checks passed on `d1629f88dddefd8c21668e138ee5a0cf29d3e105`
(run `39810469276` plus Codespell). Merge/default-branch full CI and release
artifact verification are still required before Bosn consumes this dependency.

### Archive policy admission and production candidate (2026-10-22)

Act2 #85 merged at `f82a878a86ebc0790fe02cff74fd92ba18934a01` after all
six PR checks passed. Its clean default checkout was rebased; the original
Bosn checkout was fetched/rebased onto main immediately after that merge.
Exact-default-commit full CI `39813540506` is running. No release or verified
new artifact is claimed yet; Bosn still pins act2.27 while this local candidate
is being completed, and must not be pushed in that state.

Two additional transport regressions reproduced real admission gaps: Ready
could pass with a missing machine policy, and conflicting policy could still
contact source fencing (`archive-policy-both-regressions-red.log`). Full
immutable machine policy now agrees before Fence and is required before
Complete, on every Ready result, and immediately before execution. Contention
is a typed retry and execution waits asynchronously under the original absolute
run deadline/cancellation. Seven transport tests pass, including actual
synthetic-process timeout and cancellation (`archive-policy-timeout-cancel.log`).
The related machine-policy tests passed two with one explicit hardware fixture
ignored; four coordination tests passed (`archive-policy-admission-green.log`).
Clippy initially rejected a nested if; collapsing it passed service library/test
Clippy in 28.50s (`archive-policy-clippy-recheck.log`). Later activation changes
still need their final lint and whole exact-source gate.

The local ordinary planner now selects Cohort, loading configuration once.
Default archive limits are 8 GiB per repository (the existing legacy budget),
32 GiB aggregate (four full repository budgets), 30-day maximum age, seven-day
unused age, and five-minute maintenance. These are completed archive payload
limits, not an allocated machine quota. Existing nonmatching immutable policy
refuses instead of changing another daemon's contract. Explicit validated cache
settings override defaults. Planning does not create or fence an engine.
Synthetic normal-runtime tests use an in-memory frozen-policy/namespace phase
model; scripted fault tests have an explicit queue whose exhaustion fails.
Neither is evidence of Docker or act enrollment.

The ignored actual workflow fixture now restores compiled C and npm package
archives through actions/cache in the subsequent job and second fresh engine,
verifying payload hashes and execution/content alongside completed native tools.
That extended fixture has not yet run. Production activation is a local
candidate only. Retained archive source disposal/accounting, all-class allocated
ceilings, scoped builder expiry, original full-workload/concurrency/restart and
large/many-file acceptance remain open; this does not complete #508.

The first production-route CI-focused suite passed 332 tests with 21 explicit
hardware fixtures ignored in 110.79s (`archive-production-route-first-green.log`).
After adding configured-policy planning coverage, all four planner tests passed
and service library/test Clippy passed58.29s
(`archive-production-route-final-lint.log`). File-length/include-base guards and
whitespace checks passed; sole-primary code and documentation review is clean.
The extended Docker fixture remains unexecuted, and no full exact-source Bosn
gate, new release pin or push is claimed by these results.

Exact act2 default-branch CI `39813540506` passed all five required jobs on
`f82a878a86ebc0790fe02cff74fd92ba18934a01`. The release guard initially refused
because a cached jobs endpoint still reported Linux in progress despite the run
summary being complete. A fresh job collection and the direct Linux job record
both confirmed success (Linux completed16:34:58Z). The guard then passed clean
master, independent remote SHA, all jobs and absent tag/release checks. Annotated
`v0.2.89-act2.28` was pushed through the existing tag release workflow;
release run `39816300762` is queued, not yet a successful release. Evidence:
`/tmp/bosn-693/act2-28-release-guard.json` and `act2-28-release-run.json`.
Future observations use a fresh query to avoid reusing that stale response.
Bosn candidate `b7876c4e` remains local/unpushed and still requires independent
verification of all release archives and the actual extended workflow proof
before its dependency pin/whole-source gate can be completed.

### Verified act2.28 and ordinary archive workflow (2026-10-22)

Release `39816300762` completed successfully on the exact tested act2 merge
`f82a878a86ebc0790fe02cff74fd92ba18934a01`. All eleven published archives were
independently checked against checksum text, GitHub digest/size, executable
headers/architectures and embedded version. Linux version/capability execution,
annotated tag target and smoke-container cleanup passed. Linux archive SHA256
`7c169ccd338b41b7df48a1f8be215d609b8a737f932b8ce2b2a0c4e595ad811d`;
executable SHA256
`cbadf22037868f2ee3c19400f0450262324a7cb3b7a56bf06234295035ee33fe`.
Evidence: `/tmp/bosn-693/act2-28-release-provenance.json` and related verifier
records. Bosn's unpushed candidate now pins these verified artifacts.

The first extended workflow failed on its recovered run because the fixture
omitted actions/checkout and `phase` was absent from the job workspace. The
conditional's first-run behavior had masked this absence. This was a fixture
failure, not evidence of an archive transport failure. The corrected workflow
checks out the frozen source in both jobs and asserts `phase` exists before
branching; no cache-hit, hash, executable/package-content, completed-tool or
mode checks were relaxed. The failed trace is retained as
`production-archive-workflow-proof-failure1.{json,log}`.

The corrected ordinary-runtime fixture passed267.80s on released act2.28:
`bosn693-archive-workflow-0e90925331a0`. Two successive identical two-job
workflows used distinct engines
`bosn-act-9e2755bd-2b39-44b3-9607-2949e338bb88` and
`bosn-act-051e5878-16d1-429b-8bb7-3233f52ba94a`. Actual actions/cache restore
preserved newly compiled C executable/source and npm package hashes/content
between jobs and fresh engines. Native completed tools and mode751 survived
normal retirement/post-stop recovery; unfinished installs remained excluded.
Both claims released; all task fixture containers and host volumes were proven
absent. Observed source-tree digest
`0dcb69f54be80b6767565b720b743d944e3dddc458805fb36d8d35d03937c457`,
test binary SHA256
`e13bda4c29b54af7559dd873e776f78191e475a5bac21082b70a1681dcd809d7`.
The harness verified the binary remained unchanged through the proof. Evidence:
`production-archive-workflow-proof.{py,json,log}`. This is a controlled workflow
fixture, not the original full-project/concurrency/crash/restart/large-file
acceptance or an exact-source local gate.

### Archive source reclamation and pending-publication protection candidate

Act2 PR #86 (`8fa722b`) is a draft; this code is not in act2.28 and is not yet
consumed by Bosn. Two regressions failed before implementation: Complete and
idle cohort maintenance retained source `bolt.db` and archive payload after
Ready (`archive-source-reclamation-red.log`). The candidate reclaims payload
only after durable verified Ready, checking exact fence, snapshot witness and
current destination authority. A descriptor-relative, mount-checked walk visits
at most100,000 entries per attempt and keeps the small ownership witness.
Interrupted cleanup resumes from idle cohort maintenance before namespace
metadata is opened. Symlink/witness/fence/destination/pending refusals retain
source data; bounded partial removal and cancellation remain resumable.

A separate real RED demonstrated aggregate GC deleting an imported archive
before first-publication reconciliation (`archive-pending-publication-maintenance-red.log`).
Maintenance now refuses a pending published namespace before archive eviction;
the corrected test requires exact retained destination archive count/bytes and
source metadata. This safety fix must be released before the Bosn candidate is
pushed. No closure or production safety claim is based on act2.28 alone.

Focused tests passed22.025s; affected archive/aggregate/tool race tests
passed60.896s. The initial CLI filter selected zero tests and is not validation;
corrected archive/tool command tests passed1.976s. Windows amd64 and Darwin
arm64 artifact-cache cross-builds passed. Gosec initially traced maintenance
paths into existing readers; reads are now anchored with os.Root, with opened
directory/file identity checks. No rule was disabled. Final lint reports zero
issues, and the sole-primary read-only reviewer is clean. A full package pass
239.740s preceded the final pending-publication guard; the whole final-candidate
suite is running again (`archive-source-reclamation-final-full-tests.log`). PR
and exact merged-default-branch full CI, release and all-archive verification
remain required. Source expiry is one part of the original #508 scope; physical
all-class/machine bounds, scoped builders and remaining original acceptance are
still open.

The final source-reclamation artifact-cache suite passed277.876s on exact act2
`8fa722b7a80ebdd973e50afa0d613f5e302644b7`, including the pending-publication
guard and corrected source-symlink/bounded-retry fixtures. PR #86 was marked
ready after the final local pass. Remote checks run `39819257462` is live:
lint/Windows/macOS passed; Linux/snapshot remain running. No merge, release or
Bosn rollout of #86 is claimed yet. Bosn's verified .28 pin, corrected physical
workflow and evidence update have clean sole-primary review, but remain
unpushed pending that safety rollout and the whole exact-source Bosn gate.

### Merged source safety fix and released-binary regression (2026-10-22)

Act2 #86 passed all six PR checks, including run `39819257462`, and merged at
`5a5c973960c8b0ed94db3d59fe455579a3c296dc` with no review threads. The external
checkout switched to master/rebased; its merged tree exactly matched candidate
`8fa722b`, and its local/remote feature branches were removed after identity
checks. The original Bosn checkout fetched/rebased onto main immediately after
merge. Full workflow-dispatch CI `39821118708` then passed all five jobs on the
exact merged default-branch SHA. The clean-default-branch/independent remote
SHA/full-CI/absent-tag-and-release guard passed. Annotated act2.29 was requested
through its existing tag workflow; release `39823012819` is building. No verified
.29 artifact or Bosn pin is claimed yet.

The actual runtime workflow now independently checks that the old archive path
is a refusal file and its snapshot retains only the ownership witness after
both ordinary runs. Its first attempt incorrectly included the journal file in
a snapshot glob and failed generically; this fixture failure is not counted as
RED. The corrected check accepts only directories, rejects symlinks, and its
separate contract probe verifies marker-only success, journal exclusion and
rejection of a newline-named payload. Probe scratch was moved to trash with
safe-rm. Evidence: `archive-source-check-contract.json` and
`archive-source-reclamation-release28-fixture-glob-failure.{json,log}`.

The corrected actual Docker regression against independently verified released
act2.28 failed135.60s with **archive source still retains payload entries**
after the first ordinary two-job workflow completed successfully. This is the
expected retained-source defect, not a workflow or parser failure. Fixture
`bosn693-archive-reclaim-red-5abf999e04d6` released all private resources; the
harness verified cleanup and unchanged test binary. Observed source-tree digest
`bad2bc283d0b98472380f140de53e8fecbd190269a432d5e2883dded03aa331b`.
Evidence: `archive-source-reclamation-release28-red-proof.{py,json,log}`. The
same assertions must pass on verified act2.29 before Bosn is pushed.

Read-only audit confirms remaining gaps: the run proxy intentionally narrows
build-cache prune to no IDs (`pkg/serve/proxy_rules.go`, scopeBuildCache), and
Bosn's storage samples are backing-filesystem df signals (`ci/storage.rs`), not
owned engine allocation or a machine cache quota. Those mechanisms do not
establish scoped-builder expiry or allocated machine ceilings. These remain
implementation work, not satisfied acceptance criteria.

### Verified act2.29 rollout and physical regression pass (2026-10-22)

Release `39823012819` succeeded for exact act2 master
`5a5c973960c8b0ed94db3d59fe455579a3c296dc`. All eleven archives passed
checksum, independent GitHub digest/size, executable-header and version checks.
The annotated tag target and Linux version/capability execution passed, with
smoke resources absent afterward. Bosn now pins verified act2.29: Linux archive
`f9f811f5a248c6cac62afc2920c80d6baecf6dea2de25fe71e1967921594deb4`,
executable `21373f9fa8e2315230ca1b708e0893b54ad4857f100c4bba9b776a0710278599`.
Evidence: `act2-29-release-provenance.json`.

The same corrected workflow and source-reclamation assertions that failed on
released act2.28 passed on released act2.29 in231.48s. Both ordinary two-job
workflows succeeded, archives restored with their original hashes, completed
native tools survived retirement, unfinished tools stayed excluded, and both
legacy archive snapshots retained only their ownership witnesses. Engines
`bosn-act-4bf37da9-0c1e-498c-95fa-ae91276e3c85` and
`bosn-act-1e46c879-374c-477a-b4d7-2149740cfe38` were distinct; claims were
released and private test resources were proved absent. Fixture
`bosn693-archive-reclaim-green-534a5eff06ad` verified an unchanged test binary
`80fc7cf7966fa64865631a07dade8855afc7a4b1c5fa00f277bc0a1a24bb55d8`.
Observed source-tree digest:
`771ca45680d40ed0a307a29ff5714264a4e7d4ce88fc00c4f7742eeade64e963`.
Evidence: `archive-source-reclamation-release29-green-proof.{py,json,log}`.

This proves the controlled archive/source-reclamation regression, not the
remaining original #508 acceptance. Physical all-class/machine ceilings,
scoped-builder expiry, pending/retained storage bounds and realistic original
full-project/concurrent/crash/restart acceptance remain open. The whole
exact-source Bosn gate is still required before pushing this rollout.

### Ordinary archive rollout merged and bounded physical inventory (2026-10-22)

Bosn #710 passed the whole local gate in 428s (Linux 378s, Rust 427s;
1,200 Rust tests passed, 53 configured skips). Full stamped SHA
`f2e0801f30f0f471878614beeb8398632f8b8cce` passed independent GATE-003
verification before push. All 14 remote checks were terminal (nine successes,
five configured skips), with no unresolved review threads. Merged at
`4940873379c2dbb34ad78d8cf5c4e1bed0fff50a`; the original checkout returned to
clean main and rebased, exact tested/main trees matched, and task branches were
removed after head checks. User stashes and unrelated worktree were preserved.
Evidence: `archive-routing-local-gate{,-verify}.log`, `pr710-final-status.json`.

The next act2 candidate adds bounded read-only whole-cache metadata accounting
and shares the original native-tool inode/byte accumulator. The actual CLI
regression failed on unavailable bounded reporting, then passed cross-class
hardlink deduplication, sparse allocation and outside-link exclusion. Independent
du agrees with allocated root bytes; unknown/incomplete state remains included.
Review found final-component O_NOFOLLOW insufficient for aliased ancestors or
trailing '/.' roots. Three additional regressions reproduced that bug; opening
every clean root component relative to the preceding descriptor fixes it.
Reports never follow symlinks, read payload contents or cross mount identities.
Directory descriptors, traversal entry/depth limits and cancellation remain
bounded; incomplete scans expose null byte totals.

Actual same-filesystem bind-file and bind-directory fixtures passed in a
private privileged container with networknone/nohostbinds; the container was
proved absent. The first cleanup parser compared uppercase Docker absence text
against lowercase output and failed its cleanup assertion; this harness failure
is preserved, not counted as a production failure. Corrected proof passed in 0.42s; the final complete candidate was rebuilt and
passed in 0.46s with cleanup verified, binary
`233608fa71d0553376656d4033ef83680e190338e630b5e91a17c6ae9b3186cd`.
Evidence: `cache-footprint-command-red.log`, `cache-footprint-root-alias-red.log`,
`cache-footprint-mount-proof{,-cleanup-parser-failure}.{json,log}`.

Namespace details retain only the largest 256 rows using a bounded heap; omitted
rows still contribute to complete all-class total. A 258-namespace fixture checks
legacy/cohort identity, hardlinks, largest selection and deterministic ties.
Its initial expected tie ordering was wrong; the fixture now explicitly sizes
the cohort payload and checks the defined path tie order. This inventory is a
non-atomic sample, not a machine quota or deletion eligibility proof. No release,
Bosn integration or complete #508 acceptance is claimed for this candidate yet.


Final candidate `17f9d822a11f56dacab9f649f267b5a074b9d874` has clean
sole-primary review, focused race tests (artifactcache2.924s, command1.466s),
zero lint issues and Windows/Darwin package builds. Namespace lint initially
reported two De Morgan simplifications; both were corrected without suppression.
The full exact-candidate artifact-cache suite is running before push.

The complete act2 candidate `17f9d822a11f56dacab9f649f267b5a074b9d874`
passed the full artifact-cache suite in 170.954s, then opened act2 #87. Its six PR
checks passed, including Linux job 119998604771 in run 39828652770. No dependency
merge/release is claimed at this observation.

Bosn's integration consumes a bounded typed JSON receipt and extracts only the
digest-verified executable from a held archive descriptor into private tmpfs.
The volume remains read-only and the helper has no network. Accounting helpers
use 512 MiB for bounded inode maps; existing policy readers retain 128 MiB. Old du is
now test-only independent reference sampling, with no production fallback.
Initial focused transport tests failed seven cases because their synthetic
Docker fixture omitted the newly required architecture lookup; adding that
command and typed receipt preserved every cleanup/uncertain-create assertion.
Review then caught raw output passing through trimming/lossy decoding before
the 256 KiB parser bound. Actual transport regressions reproduced acceptance of
leading padding, trailing padding and invalid UTF-8. Strict raw bounded capture
fixes all three while retaining exact helper cleanup. The focused suite passed
32 tests, with 6 explicitly ignored private-Docker tests (one newly added helper
versus reference proof still to run). Evidence:
`cache-footprint-bosn-focused.log`, `cache-footprint-bosn-focused-final.log`,
`cache-footprint-raw-transport-red.log`, `cache-footprint-bosn-raw-green.log`.
No physical helper proof, verified next pin or full Bosn gate is claimed yet.

Act2 #87 merged at `cdf2cc3ae814b03ee57f0a4ddb6a0a2266bc9bfd` after all
six checks and no unresolved threads. Both checkouts rebased; the tested/merged
trees matched and task branches were removed after identity checks. Exact-master
full CI 39831194698 passed all five jobs. Clean checkout, independent remote SHA,
full-CI and absent-tag/release guards passed, then requested act2.30 through the
existing release workflow 39832895745. Release verification remains pending.

The reader now preserves the actual scanner exit: only 0/complete or 1/partial
receipts are accepted; a failed process cannot publish known complete bytes.
Initial exit-state testing exposed lost stderr for an empty failed reply; bounded
diagnostic preservation fixes it while retaining the original failure assertion.
The focused suite passed 34 tests/6 explicit private-Docker ignores in 10.04s.
Clippy initially required simplifying an exit-condition Boolean; after that
correction it passed in 18.50s, with no suppression. The actual helper fixture's
101-line function was split by responsibility after its first Clippy failure.
Sole-primary review is clean. Evidence: `cache-footprint-bosn-exit-tests.log`,
`cache-footprint-bosn-complete-tests.log`, `cache-footprint-bosn-{exit,complete}-clippy.log`.
The prepared physical proof uses 51,000 small files and an 8 GiB sparse file; this
is controlled logical-size evidence, not the original 7.7 GiB allocated survey.

Release39832895745 succeeded; independent all-eleven-archive verification is
running before any Bosn pin change. The replacement parser's maximum namespace
reply check passed:256 rows with maximum signed64-bit byte counters and a
maximum-length volume name remain below the existing60KiB tool reply limit.
Evidence: `cache-footprint-bosn-reply-bound.log`. The whole-source Bosn gate and
actual helper/reference proof remain outstanding.

Act2.30 is now independently verified: release39832895745 succeeded on exact
`cdf2cc3ae814b03ee57f0a4ddb6a0a2266bc9bfd`; all eleven archives passed
checksums, independent GitHub size/digest, architecture-header and version
checks. Annotated tag identity, Linux version/capability execution and smoke
cleanup passed. Linux archive:
`80cdc0dfd81e9a0ada8bcc9855a72b3b7accf4553cd2f0cbc2143add594f2af6`;
executable `0187cd478cff9c3ace3fdb200cdd321b18d30546d9649e1c0419a12e048cd54f`.
Bosn's local pin now uses these verified digests. Evidence:
`act2-30-release-provenance.json`, `act2-30-guarded-release.log`.

The pinned focused suite passed35 tests with6 explicit private-Docker ignores
in4.76s. The footprint helper's private executable tmpfs is now explicitly
`exec,size=64m` (the verified executable is23,576,760 bytes), under its512MiB
memory limit. Network remains disabled and the cache remains read-only. Its
actual reference comparison is queued; no physical-helper success is claimed.

### Production bounded accounting proof (2026-10-22)

The actual read-only helper passed against independently verified released
act2.30. Fixture `bosn693-physical-footprint-7c0bf2ab0ef2` used 51,000 small
files, an 8 GiB sparse file, cross-class hardlinks, native history/snapshot data,
and legacy/cohort directories for the same namespace. Total apparent bytes
were **8,601,266,061**; allocated inode bytes were **220,192,768**. The total,
every class and both namespace rows exactly matched the independent du oracle.
Production measurement took **1,739 ms**; the Rust test finished in 3.58 seconds.
Measurement helpers were absent afterward; all private controller containers and
volumes were proved absent. The private Docker ID differed from the host's.

The harness verified unchanged binary
`0bcf76fd33ae89ee2a79697f16264fe8e9999b661168777f6c5980cf3cc3fcd0`.
Observed source-tree digest:
`920e26942d2d5b20369a042107b6fe8b91301e73fa13c0ec2e116f4df50b2d26`.
Evidence: `cache-footprint-production-helper-proof.{py,json,log}` and
`cache-footprint-bosn-final-build.log` (35 focused tests passed, six explicit
private-Docker ignores, 7.08 seconds). The new physical proof was then run
explicitly rather than counted among those configured ignores.

**Bootstrap limitation:** offline measurement requires the current pinned act
archive already present in the shared tools cache from CI preparation. Missing
or corrupt archive data yields unknown/error, with no network or writable-cache
fallback. Bootstrap before pressure/admission remains work; this limitation
must not turn into a false zero or an admission/deletion authorization.

This proves controlled bounded shared-volume accounting. It is not the original
7.7 GiB allocated survey, a whole-machine quota, engine-layer/scoped-builder
accounting, retained-source storage bounds, or full-project/concurrent/crash/
restart acceptance. Those original #508 requirements remain open. Sole-primary
code review is clean; the whole exact-source Bosn gate remains required before
push.

The first full gate failed after 331 seconds when the shared installed-driver
engine was killed and removed. Linux install, lint, policy validation and unit/
Docker tests had passed, but teardown failed; Rust ended with act exit 137.
Docker events record explicit signal 9 at Unix1792695429 and engine removal,
with no OOM event in the captured stream. The caller is unidentified. No lane
pass was attested, no push occurred, and no check is bypassed. Failed logs and
engine events are preserved under `cache-footprint-local-gate-engine-killed.log`,
`cache-footprint-first-gate-{rust,tests}.log` and
`cache-footprint-gate-engine-removal-summary.json`. Retry uses the ordinary gate
and its replacement-engine lifecycle, without stopping unrelated work.


### Whole-gate retry: discovery fixture readiness race

Candidate `5ff8394e2a428b81da1d65dc3bb9fd3081f6708a` failed the Rust
lane after 479s: 485 tests passed before fail-fast, one failed, 54 configured
skips. `discovery_absence_and_outage_never_start_default_maintenance` observed
the synthetic Docker log file immediately after open, cancelled its process,
and then read an empty file instead of the expected complete inspect command.
The observer now waits for that exact complete command before cancellation;
the final exact command assertion and absence/outage maintenance assertions
remain unchanged. Linux completed successfully with cleanup removed: 267 tests passed with ten
configured skips. The whole gate failed after 570s. No passing full attestation or
push is claimed for this failed candidate.

The corrected exact-name focused test passed: one test, no failures or ignores,
3.79s (`cache-footprint-discovery-observer-green.log`). A prior short-name
`--exact` invocation selected zero tests and is not validation. The existing
sole primary reviewer accepted the correction without findings. A new clean
committed whole gate remains required before push.


### Second external engine removal; isolated gate retry

The corrected candidate `5336eec5` whole gate failed after254s without a
passing lane. Both runs exited137 and could not close their scopes because
shared driver engine `13583a134d252dfd6bcfb15fadd20ebde8a4bfe3e57317b35556493d4ab4f29a`
was killed with explicit signal9 at Unix1792696520, died137 at1792696523 and
was destroyed. Captured events do not establish an OOM cause or caller.
Evidence: `cache-footprint-third-engine-events.jsonl` and
`cache-footprint-local-gate-second-engine-kill.log`. Neither failed gate is
reused as a pass. The next retry uses the same ordinary whole gate and installed
driver with a separate state directory and a task-owned private Docker daemon;
it does not alter the installed host daemon, driver, source checks or verifier.
Private controller identity/cleanup evidence lives in
`cache-footprint-private-gate.json` under `/tmp/bosn-693`.


The first private-controller gate stopped before tests: both engine creates
refused invalid nested cgroup domain controllers. The harness had launched
dockerd directly. Its task-owned outer container was replaced, retaining its
owned data volume, with the same ENGINE_INIT/private-cgroup-namespace setup
used by earlier passing physical proofs. A real nested container now starts
successfully and its exact absence was verified. The private and host Docker
IDs differ. This repairs the validation harness; it changes no product source
or gate checks. The failed pre-test runs remain recorded in
`cache-footprint-private-runs.json` and `cache-footprint-private-local-gate.log`.


### PR #711 merged; reader archive bootstrap

PR #711 merged as `c75e7ea2` after the private-controller whole gate passed
(577 s; 1,208 Rust tests passed with 54 skips; 267 Python tests passed with
ten skips). The full stamped SHA `795b94bb` was verified, and nine remote checks
passed with five configured skips. Its tree equals the tested tree.

The next change closes the reader's archive precondition (#508 item 4). The
footprint program exits 3 when `/cache/tools/act-<version>-<sha>.tgz` is absent
or fails its digest. `measure_cache_tracked` then runs one `docker run --rm` seed
helper (`cache_usage/seed.rs`): read-only root, all capabilities dropped,
128 MiB, one CPU, 64 PIDs, `--pull never`, and the cache mounted writable at
`/bosn/cache`. It runs the same `fetch_act_script` that engine preparation
uses (exclusive `flock`, staged download, digest check, atomic rename). The
helper then measures once more. A second exit 3 or a failed seed is an
error. The measurement helper itself is unchanged: read-only and no network.
Synthetic transport tests failed (RED) without the retry and pass (GREEN) with
it. Remaining #508 items 1-3 and 5 are unchanged.


### Owned footprint ceiling (#508 item 1)

Disk-pressure handling had only one input: free space under Docker's data root
(#658). This change adds a ceiling on owned allocation. The managed retention
pass sums the measured sizes of the Bosn-labelled containers (writable layers),
volumes and images it observed, logs `owned footprint X GiB of 256 GiB ceiling
(containers, volumes, images; N unmeasured)` on every pass, and above the
ceiling plans with `RetentionPolicy::under_disk_pressure`. All gates are
unchanged. The shared CI cache keeps its act2-side budget, and its volume is
included in the volume total. Engine Docker roots inside the shared engine are
still measured only as the engine's storage volume.


### act2.31: engine build-cache budget and pending-publication isolation

act2 #88 merged as `059bd512`. Full CI 39857299927 passed all five jobs on that
exact master SHA. Release 39858864727 published `v0.2.89-act2.31`, and all
eleven archives were verified (checksums, GitHub digests and sizes,
architecture headers, embedded version). The tag target and Linux
`--version`/`--ci-capabilities` smoke also passed (`act2-31-release-provenance.json`).
Linux archive `15bda868...d367`, executable `f11007ca...819e`.

- Serve bounds the engine's BuildKit cache hourly (`--builder-budget-*`,
  8 GiB, 7 days unused) through the upstream dockerd's `/build/prune`.
- `MaintainCohort` no longer fails the whole pass when one namespace's first
  publication is pending. It leaves that namespace untouched, lists it in
  `pending_namespaces`, stays partial, and maintains the rest. Bosn's typed
  report accepts the field, refuses it on a complete pass, and refuses a
  pending namespace that is also listed as maintained.

The act2 #88 merge also tracked one stray `pr_merge_watch` log under `.clud/`.
It is not part of the build and is removed in a follow-up.

### Acceptance found a daemon deadlock in `ci doctor` (#508 item 4)

The first real acceptance pass ran on a task-owned private Docker daemon
(`bosn693-accept-58876e4b077a`, Docker ID distinct from the host) with Bosn
built from main and act2.31. Its cold full-project run (`--mode test`, 416 s)
built and tested the whole workspace. It failed only on the timing-sensitive
unit test `a_run_whose_client_went_away_still_ends_and_is_cleaned` (1 s run
timeout, fake backend), which lost a cleanup race under cold-build load. The
pass after it hung. `bosn ci doctor` never replied, and the daemon then
answered no client.

A core dump (ptrace is denied on the host, so `kill -ABRT` plus `coredumpctl`
was used; stacks in `acceptance-daemon-hang-stacks.txt`) showed a worker
blocked locking the widget-state mutex inside `CiRuntime::doctor`. That reply's
struct literal read `self.widget_state().release(..)`, whose `MutexGuard`
temporary lives to the end of the statement, then called
`self.widget_presence()`, which locks the same non-reentrant `std::sync::Mutex`.
Every `bosn ci doctor` call self-deadlocked the worker and every later
widget-state caller. The fix reads both fields under one guard
(`widget_report`). The regression test runs two doctor calls with a 30 s
deadline measured from another OS thread, because the deadlock blocks the async
worker itself. Without the fix it fails (an async-timeout variant hung 600 s
instead); with the fix it passes.

### Full-project acceptance results (#508 item 4, 2026-10-22)

Private daemon `bosn693-accept-58876e4b077a` (Docker ID distinct from the host),
Bosn built from main after #715, act2.31. Every phase ran this repository's own
`ci.yml` (`--mode test`): rust, linux, both Darwin cross wheels, native wheel and
policy jobs. In every run below, each locally runnable job succeeded. "incomplete"
(exit 3) means only the jobs Bosn cannot run locally were not run: the Windows
wheel, the hosted macOS smokes, the GitHub-API queue timing, and their dependants.
Evidence: `acceptance-508.py`, `acceptance-508-final.json`.

| Phase | Result | Shared cache allocated (complete, not partial) |
|---|---|---|
| Cold (empty cache) | 416 s; one timing-flaky unit test failed (fixed alongside) | 0.56 GiB |
| Daemon stopped, then two clients at once (warm) | 408 s; the clients joined one run (2 submitters), all local jobs passed | 3.85 GiB |
| Pinned reader archive corrupted | Accounting reseeded it (correct SHA, rewritten) and measured the same totals | 3.85 GiB |
| Engine containers removed (fresh engine), warm cache | 500 s; all local jobs passed | 7.22 GiB (toolcache 6.03, images 0.53, actcache 0.64) |

The steady state, 7.22 GiB allocated, reproduces the original ~7.7 GiB survey's
scale and its tool-cache share. `ci doctor` deadlocked the daemon in the first
pass (fixed in #715, now 0.1 s). The aborted daemon was a real crash, and the
next daemon took over the registry and its runs without loss.

Host survey (read-only: verified act2.31 binary bind-mounted, no network,
volume read-only, helper proven absent afterwards; 2.2 s, 61,361 entries): the
host's long-lived `bosn-ci-cache-v1` holds 43.89 GiB allocated (actcache 37.09 in
7 repositories, each within its 8 GiB bound, toolcache 6.21, images 0.53). The
37 GiB exceeds the 32 GiB cohort ceiling because the host still runs installed
Bosn 0.1.23, which predates the aggregate cohort budget, the owned-footprint
ceiling, the engine build-cache budget and pending-publication isolation. Those
take effect when a release containing them ships through the pretag gate.
Evidence: `host-cache-survey-act2-31.json`.


### Engine-layer accounting is part of the owned footprint (correction)

Earlier sections say shared-volume accounting is "not engine-layer accounting
or a whole-machine quota". That remains true of `runners cache` alone. It is
superseded for the machine as a whole: the shared engine's Docker data root
(every image, layer and BuildKit record inside the engine) is the owned volume
`bosn-act-storage-<generation>`. That volume carries `com.zackees.bosn.kind`,
the label the managed retention pass observes, and was verified on the
development host. Its `docker system df` size is therefore part of the owned
footprint that #713 logs and bounds (256 GiB), together with the shared cache
volume, setup volumes, containers and images. Whole-machine Bosn allocation is
the owned footprint; free space under Docker's root is the separate
disk-pressure input. Neither is a hard filesystem quota.


### Volume sizes were silently unmeasured on the host (#508 item 1, 2026-10-23)

On released 0.1.24 the development host's daemon logged `owned footprint 39.4 GiB
of 256.0 GiB ceiling (containers 28.4, volumes 0.0, images 11.0 GiB; 42 unmeasured)`.
At that moment `docker system df -v` reported 41 Bosn-labelled volumes totalling about
200 GB, `bosn-ci-cache-v1` alone 50.39 GB. Volume sizes come only from that read, and it
shared the pass's 30 s per-read deadline. On the busy host the read overran that
deadline (the pass's observation phase took 41 s), and the failure fell back to an
empty map without a log line. So the ceiling counted every volume as unmeasured and
could not fire. The size read now has its own `VOLUME_SIZE_DEADLINE` (300 s) and
output cap. A failure logs `bosn retention: volume sizes unmeasured: ...`. Unmeasured
volumes stay counted, never guessed.

### act2.32: run-keyed container references (#449, 2026-10-24)

act2 #90 merged as `5ef168de`. Full checks 40074055010 passed all five jobs on that
exact master SHA. Release 40074802643 published `v0.2.89-act2.32` from `master`
(eleven archives and `checksums.txt`). Linux archive `fafadec7...749e` matches
`checksums.txt`; executable `c8a4724a...54c8` reports `act version 0.2.89-act2.32`.

- The run proxy suffixes every named container create with the run's key, but a
  later create's `container:<name>` network, pid and ipc modes, `VolumesFrom` and
  `Links` still named the original, so a `docker://` action step that joins the job
  container's network failed with `No such container` (the remaining
  `TestRunDifferentArchitecture` failure after the binfmt_misc mount fix). Each such
  reference now resolves to `name-<key>` when this run created that container;
  anything else is left to the access check.

### act2.33: tool-store writers wait for a busy catalog (#747, 2026-10-24)

act2 #92 merged as `94f2a653`. Full checks 40113618619 passed on that exact master
SHA. Release 40115120081 published `v0.2.89-act2.33` from tag `v0.2.89-act2.33` on
that SHA. Linux archive `cbcb24de...b910` matches `checksums.txt`; executable
`5762fd5f...5127` reports `act version 0.2.89-act2.33`.

- `prepareToolSnapshotStore` and `enrollToolRecoverySourceStore` opened the tool
  store's catalog once with a 100 ms bbolt timeout. When two local-gate lanes enrolled
  in one engine, the second failed with `tool enrollment incomplete: timeout` or
  `tool recovery reservation incomplete: timeout`: 3 of 4 gate runs on 2026-10-24
  failed so before any test ran. Writers with an operation context now retry
  contention until that context ends, as generation updates already did; the
  context-free admission keeps the short probe.


### act2.34: recovery reservation contention and enrollment diagnostics

Act2 PRs #95 and #96 merged on master at `5fbd948ce73d4f548f81aa8e316d9a6960fb9b64`.
Full act2 checks on that exact SHA passed in run `40456325205`; the tag-triggered
release run `40457499039` published `v0.2.89-act2.34`. The Linux x86_64
archive SHA-256 `4e81c4d65fe0ac27fccf5ff761c6d6380c6ab901beca7aeca987c9049a813d32`
matched the published checksums and GitHub asset digest. Its extracted `act`
binary SHA-256 is `c776fab31bc7baf72567947601df161b7cd133f79947fae8a0423bd3860180aa`
and reports `act version 0.2.89-act2.34`.

Initial recovery reservation catalog acquisition now retries ordinary contention
within its existing deadline before holding any generation reader lease. The
later reacquisition keeps its short fail-closed probe. Enrollment selection
errors identify their existing short catalog probe without changing lock order.
Bosn adoption is pending its own review, full CI and release gate.


### act2.35: opt-in native operation budgets

Act2 #97 merged at `306620a106b10010c18a21258b5ef9788b7999b2`. Full CI
run `40463244634` passed on that exact SHA, and release run `40464307215`
published `v0.2.89-act2.35`. The Linux x86_64 archive SHA-256
`6adab1d4b44458e2b3bc0bb0a1297a4bf20911eb8459ac39bff3beb7a06e4852` matched
the published checksums and GitHub asset digest. Its binary SHA-256 is
`3054f4e0593d9c147d1fa3cebb5d6ead1e66bb00b9c1245ee28a6ccd659417ab`; it
reports version `0.2.89-act2.35` and capability `tool-operation-timeouts-v1`.
The verified binary exposes `--timeout-secs` on enrollment and reservation.
Unchanged defaults keep act2's historical nested deadlines; an explicit changed
budget propagates to the operation while parent cancellation and separate
cleanup limits remain in force. Bosn adoption awaits its own release gates.

### act2.36: retry native lock probes after releasing leases

Act2 #99 merged at `9d2b1ce09390e7330f5bc36438e2097e796dba2e`. Full CI
run `40480905628` passed on that SHA, and release run `40482226594`
published `v0.2.89-act2.36`. The Linux x86_64 archive SHA-256
`5faef9235421a5a15d20b0c366df3754f83f07b247d31d455cef2bb805b2fa49` matched
the release asset digest and checksums file. Its extracted binary SHA-256 is
`2ce98c3fb7a8c29fb63b1f963b4026196b6a8827929a61f22851c3ec1d07325b`.
The release retries contention probes within the existing operation deadline
after releasing held leases, preserving the recovery ownership fences.
