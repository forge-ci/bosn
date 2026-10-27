//! Pure retention policy for resources Bosn owns.
//!
//! The unmanaged census deliberately protects everything this registry owns
//! (`ProtectedReason::OwnedByThisRegistry`), so Bosn's own containers, volumes and images were
//! never a candidate for anything: `bosn-setup-v2-*` is created without `--rm` and has no idle
//! reaper, `bosn-v-stack-*` / `bosn-v-machine-*` are explicit-release only, and `bosn-setup:*`
//! images have no expiry at all. On a long-lived machine those accumulate without bound, and a
//! pinned container keeps every volume it ever mounted alive behind it.
//!
//! This module is the missing half: given observations of Bosn-owned resources, decide which are
//! past their age gate and safe to reclaim. It answers one question — "may this exact object be
//! removed?" — and deliberately does not decide *when* to ask. Callers own the clock, the engine
//! and the registry; this module only validates data and returns conservative verdicts, the same
//! contract as the rest of `bosn-core`.
//!
//! Three rules make the policy safe to run unattended:
//!
//! 1. **Ownership is proven, never assumed.** An object is reclaimable only when its complete
//!    Bosn label set is present and names *this* registry. Incomplete labels fail closed.
//! 2. **In use means alive.** A container that is running, or a volume any container still
//!    mounts, is never reclaimable regardless of age.
//! 3. **Pinned means forever.** `Retention::Pinned` is an explicit human promise and no age gate
//!    overrides it.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::unmanaged::is_bosn_label;
use crate::{
    LABEL_RETENTION, ObservedArtifact, OwnershipClass, ResourceKind, Retention, classify_ownership,
};

/// Default age gate for a stopped container that Bosn owns.
///
/// Setup containers are recreated on demand and carry no state worth keeping once stopped, so
/// this is deliberately short: the leak that motivated this module was an unbounded pile of
/// exited containers each pinning a stack volume.
pub const DEFAULT_CONTAINER_TTL: Duration = Duration::from_secs(6 * 3600);

/// Default age gate for a Bosn-owned volume.
///
/// Durable stack and machine scopes are cached build/tool state. The gate is long because a
/// cold rebuild is expensive, and short enough that the cache cannot grow without bound.
pub const DEFAULT_VOLUME_TTL: Duration = Duration::from_secs(14 * 86_400);

/// Default age gate for a Bosn-owned image.
///
/// Images are content-addressed by tag, so a removed one is rebuilt rather than lost. This is
/// long because `bosn-setup:*` builds are slow, and every unreferenced image is a candidate for
/// a later generation.
pub const DEFAULT_IMAGE_TTL: Duration = Duration::from_secs(30 * 86_400);

/// Default byte budget for idle owned volumes (#545 follow-up).
///
/// The 14-day volume gate alone is not a bound: setup content changes mint fresh
/// `bosn-v-*` volumes, so on a busy machine abandoned ones accumulate at ~100 GB/day and the
/// age gate lets two weeks of them pile up. Once the proven-ours, unmounted, unpinned volumes
/// held only by their age gate exceed this many bytes, the oldest past
/// [`DEFAULT_IDLE_VOLUME_FLOOR`] are reclaimed until the remainder fits.
pub const DEFAULT_IDLE_VOLUME_BUDGET: i128 = 64 * 1024 * 1024 * 1024;

/// Minimum idle age before the byte budget may reclaim a volume, so one in the middle of a
/// stop/start cycle is never taken.
pub const DEFAULT_IDLE_VOLUME_FLOOR: Duration = Duration::from_secs(3600);

/// Disk pressure (#658): free space under Docker's data root below this percentage of the
/// filesystem.
pub const PRESSURE_FREE_PERCENT: u64 = 15;

/// Disk pressure (#658): free space under Docker's data root below this many bytes, whatever the
/// filesystem's size.
pub const PRESSURE_FREE_BYTES: u64 = 100 * 1024 * 1024 * 1024;

/// The stopped-container gate under disk pressure (#658), so stopped setup containers stop
/// pinning volumes the zero budget could otherwise reclaim.
pub const PRESSURE_CONTAINER_TTL: Duration = Duration::from_secs(3600);

/// Free and total bytes of the filesystem holding Docker's data root, as measured by the caller.
/// The planner never measures it: the service probes and passes a pressured policy in.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DiskSpace {
    pub free_bytes: u64,
    pub total_bytes: u64,
}

impl DiskSpace {
    /// Free below [`PRESSURE_FREE_PERCENT`] of the filesystem, or below [`PRESSURE_FREE_BYTES`].
    /// An empty filesystem report is not evidence of pressure.
    #[must_use]
    pub fn under_pressure(self) -> bool {
        if self.total_bytes == 0 {
            return false;
        }
        let low_fraction = u128::from(self.free_bytes) * 100
            < u128::from(self.total_bytes) * u128::from(PRESSURE_FREE_PERCENT);
        low_fraction || self.free_bytes < PRESSURE_FREE_BYTES
    }

    /// Free space as a percentage of the filesystem, for the log line.
    #[must_use]
    pub fn free_percent(self) -> f64 {
        if self.total_bytes == 0 {
            return 0.0;
        }
        // Precision loss is irrelevant for a log percentage.
        #[allow(clippy::cast_precision_loss)]
        let percent = self.free_bytes as f64 * 100.0 / self.total_bytes as f64;
        percent
    }

    /// Free space in GiB, for the log line.
    #[must_use]
    pub fn free_gib(self) -> f64 {
        #[allow(clippy::cast_precision_loss)]
        let gib = self.free_bytes as f64 / (1024.0 * 1024.0 * 1024.0);
        gib
    }
}

/// Upper bound on one reclamation pass. A plan larger than this stops early and reports the
/// remainder rather than running unbounded.
pub const MAX_MANAGED_REMOVALS: usize = 1024;

/// Per-kind age gates and the size ceiling for one pass.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RetentionPolicy {
    pub container_ttl: Duration,
    pub volume_ttl: Duration,
    pub image_ttl: Duration,
    /// Reclaim at most this many bytes per pass. Exceeding it is not an error: the pass takes
    /// the oldest first and leaves the rest for the next one.
    pub max_bytes: Option<i128>,
    /// Bytes of idle owned volumes kept by their age gate before the oldest are reclaimed
    /// early. `None` disables the budget, leaving the age gate as the only rule.
    pub idle_volume_budget: Option<i128>,
    /// Minimum age for a volume the budget reclaims early.
    pub idle_volume_floor: Duration,
}

impl Default for RetentionPolicy {
    fn default() -> Self {
        Self {
            container_ttl: DEFAULT_CONTAINER_TTL,
            volume_ttl: DEFAULT_VOLUME_TTL,
            image_ttl: DEFAULT_IMAGE_TTL,
            max_bytes: None,
            idle_volume_budget: Some(DEFAULT_IDLE_VOLUME_BUDGET),
            idle_volume_floor: DEFAULT_IDLE_VOLUME_FLOOR,
        }
    }
}

impl RetentionPolicy {
    /// The age gate for one resource kind. A kind with no gate is never age-reclaimed.
    #[must_use]
    pub fn ttl_for(self, kind: ResourceKind) -> Option<Duration> {
        match kind {
            ResourceKind::Container => Some(self.container_ttl),
            ResourceKind::Volume => Some(self.volume_ttl),
            ResourceKind::Image => Some(self.image_ttl),
            ResourceKind::Builder | ResourceKind::Network => None,
        }
    }

    /// This policy on a disk under pressure (#658): the idle volume budget is zero, so every
    /// proven-ours, unmounted, unpinned volume past the idle floor is reclaimed, and the stopped
    /// container gate is at most [`PRESSURE_CONTAINER_TTL`]. Ownership, liveness and pin gates,
    /// the floor and the pre-removal re-check are unchanged.
    #[must_use]
    pub fn under_disk_pressure(self) -> Self {
        Self {
            container_ttl: self.container_ttl.min(PRESSURE_CONTAINER_TTL),
            idle_volume_budget: Some(0),
            ..self
        }
    }

    /// [`Self::under_disk_pressure`] when `disk` is measured and under pressure, else `self`.
    #[must_use]
    pub fn for_disk(self, disk: Option<DiskSpace>) -> Self {
        if disk.is_some_and(DiskSpace::under_pressure) {
            self.under_disk_pressure()
        } else {
            self
        }
    }

    /// The gates a pre-removal re-check applies. The plan decides *which* objects go; the
    /// re-check proves each is still ours, idle and unpinned, and no younger than the shortest
    /// gate the plan could have selected it under: the idle floor for a volume, which both the
    /// byte budget and the eager rules (#659) use.
    #[must_use]
    pub fn recheck_gates(self) -> Self {
        Self {
            volume_ttl: self.volume_ttl.min(self.idle_volume_floor),
            ..self
        }
    }
}

/// Why a Bosn-owned object is being kept despite being past its age gate.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, strum::IntoStaticStr)]
pub enum HoldReason {
    /// Running, or mounted by a container that exists.
    #[strum(serialize = "in-use")]
    InUse,
    /// `Retention::Pinned`: an explicit promise no age gate overrides.
    #[strum(serialize = "pinned")]
    Pinned,
    /// Still within its age gate.
    #[strum(serialize = "within-ttl")]
    WithinTtl,
    /// The engine did not report an age, so eligibility cannot be proven. Fail closed.
    #[strum(serialize = "age-unknown")]
    AgeUnknown,
    /// Complete labels naming a different registry.
    #[strum(serialize = "foreign-registry")]
    ForeignRegistry,
    /// Bosn labels present but incomplete: ownership cannot be proven.
    #[strum(serialize = "incomplete-labels")]
    IncompleteLabels,
    /// Carries no Bosn label at all. Not ours to reclaim.
    #[strum(serialize = "not-bosn-owned")]
    NotBosnOwned,
    /// A network or builder record: outside this policy's scope.
    #[strum(serialize = "unsupported-kind")]
    UnsupportedKind,
}

impl HoldReason {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.into()
    }

    /// Every reason, in report order.
    pub const ALL: [Self; 8] = [
        Self::InUse,
        Self::Pinned,
        Self::WithinTtl,
        Self::AgeUnknown,
        Self::ForeignRegistry,
        Self::IncompleteLabels,
        Self::NotBosnOwned,
        Self::UnsupportedKind,
    ];

    /// The inverse of [`Self::as_str`].
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|reason| reason.as_str() == value)
    }
}

/// The verdict for one observed object.
#[derive(Clone, Debug, PartialEq)]
pub struct RetentionVerdict {
    pub id: String,
    pub kind: ResourceKind,
    /// `None` when the object may be removed.
    pub hold: Option<HoldReason>,
    /// Age past the applicable gate, when the object is held only by its age gate.
    pub age_seconds: Option<f64>,
    pub bytes: Option<i128>,
    /// Set when the object was reclaimed before its age gate, and why (#659).
    pub eager: Option<EagerReason>,
}

impl RetentionVerdict {
    /// Whether this exact object may be removed right now.
    #[must_use]
    pub fn is_reclaimable(&self) -> bool {
        self.hold.is_none()
    }
}

/// Classify one observed object against the policy.
///
/// The order of the checks is the safety contract. Ownership is settled first, because an object
/// that is not provably ours is never a candidate no matter how old or idle it is. Liveness is
/// settled next, before age, so a running container is never even measured against a gate. Only
/// then does the age gate apply.
#[must_use]
pub fn classify_managed(
    artifact: &ObservedArtifact,
    our_registry: Option<&str>,
    policy: RetentionPolicy,
) -> RetentionVerdict {
    let mut verdict = RetentionVerdict {
        id: artifact.id.clone(),
        kind: artifact.kind,
        hold: None,
        age_seconds: artifact.age_seconds,
        bytes: artifact.bytes,
        eager: None,
    };

    // 1. Ownership. A bare `bosn-act-*` name is not evidence; only the label set is.
    if !artifact.labels.keys().any(|key| is_bosn_label(key)) {
        verdict.hold = Some(HoldReason::NotBosnOwned);
        return verdict;
    }
    match classify_ownership(&artifact.labels, our_registry) {
        OwnershipClass::IncompleteLabels => {
            verdict.hold = Some(HoldReason::IncompleteLabels);
            return verdict;
        }
        OwnershipClass::ForeignRegistry => {
            verdict.hold = Some(HoldReason::ForeignRegistry);
            return verdict;
        }
        // `Unlabeled` is unreachable: the first check proved a Bosn label exists.
        OwnershipClass::Ours | OwnershipClass::Unlabeled => {}
    }

    // 2. Scope. A network or builder record has no age-based contract here.
    let Some(ttl) = policy.ttl_for(artifact.kind) else {
        verdict.hold = Some(HoldReason::UnsupportedKind);
        return verdict;
    };

    // 3. Liveness, before age. A container that is running, or a volume something still mounts,
    //    is never reclaimable at any age.
    if artifact.signals.in_use {
        verdict.hold = Some(HoldReason::InUse);
        return verdict;
    }

    // 4. An explicit human promise outranks every age gate.
    if is_pinned(&artifact.labels) {
        verdict.hold = Some(HoldReason::Pinned);
        return verdict;
    }

    // 5. The age gate itself. An unmeasured age cannot authorize a removal.
    let Some(age) = artifact.age_seconds else {
        verdict.hold = Some(HoldReason::AgeUnknown);
        return verdict;
    };
    let gate = ttl.as_secs_f64();
    if age < gate {
        verdict.hold = Some(HoldReason::WithinTtl);
    }
    verdict
}

/// Whether the labels carry an explicit pin.
///
/// A missing retention label is *not* a pin: durable volumes that predate the label, or that
/// were written by an older build, still get the benefit of the doubt and are held rather than
/// reclaimed on the strength of an absent field.
fn is_pinned(labels: &BTreeMap<String, String>) -> bool {
    labels
        .get(LABEL_RETENTION)
        .and_then(|value| Retention::parse(value))
        == Some(Retention::Pinned)
}

/// One reclamation pass.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RetentionPlan {
    /// Reclaimable objects, in removal order and capped by the policy's budget.
    pub candidates: Vec<RetentionVerdict>,
    /// Objects held despite being past their age gate, with the reason.
    pub held: Vec<RetentionVerdict>,
    /// Bytes the capped candidates would reclaim.
    pub bytes: i128,
    /// Reclaimable objects left over because a cap was reached.
    pub deferred: usize,
}

impl RetentionPlan {
    /// How many observed objects each reason held (#545). An object nothing reclaimed is never
    /// silent: "no candidates" and "held for a reason" read differently.
    #[must_use]
    pub fn held_counts(&self) -> BTreeMap<HoldReason, u64> {
        let mut counts = BTreeMap::new();
        for verdict in &self.held {
            if let Some(reason) = verdict.hold {
                *counts.entry(reason).or_insert(0) += 1;
            }
        }
        counts
    }

    /// How many candidates each eager rule selected (#659).
    #[must_use]
    pub fn eager_counts(&self) -> BTreeMap<EagerReason, u64> {
        let mut counts = BTreeMap::new();
        for reason in self.candidates.iter().filter_map(|verdict| verdict.eager) {
            *counts.entry(reason).or_insert(0) += 1;
        }
        counts
    }
}

/// Build a plan for one pass over the observed Bosn-owned objects.
///
/// Removal order is the dependency order Docker requires: containers first, because a stopped
/// container pins every volume it mounted; then volumes, which only become removable once no
/// container references them; then images. Within a kind, oldest first, so a capped pass takes
/// the bytes that have been idle longest.
#[must_use]
pub fn plan_managed(
    artifacts: &[ObservedArtifact],
    our_registry: Option<&str>,
    policy: RetentionPolicy,
) -> RetentionPlan {
    plan_managed_with(artifacts, our_registry, policy, &EagerFacts::default())
}

/// [`plan_managed`], also reclaiming idle volumes the caller's workspace observations prove
/// obsolete (#659). The eager rules run before the byte budget, so the volumes they take no
/// longer count against it.
#[must_use]
pub fn plan_managed_with(
    artifacts: &[ObservedArtifact],
    our_registry: Option<&str>,
    policy: RetentionPolicy,
    facts: &EagerFacts,
) -> RetentionPlan {
    let mut classified: Vec<RetentionVerdict> = artifacts
        .iter()
        .map(|artifact| classify_managed(artifact, our_registry, policy))
        .collect();

    let mut plan = RetentionPlan::default();
    for verdict in &mut classified {
        if verdict.is_reclaimable() {
            plan.candidates.push(verdict.clone());
        } else {
            plan.held.push(verdict.clone());
        }
    }

    eager::promote(&mut plan, artifacts, policy, facts);
    promote_over_idle_volume_budget(&mut plan, policy);
    // Oldest first inside each kind, so ties on an unmeasured age keep a stable order.
    plan.candidates.sort_by(|left, right| {
        kind_rank(left.kind)
            .cmp(&kind_rank(right.kind))
            .then_with(|| {
                right
                    .age_seconds
                    .unwrap_or(f64::MIN)
                    .total_cmp(&left.age_seconds.unwrap_or(f64::MIN))
            })
            .then_with(|| left.id.cmp(&right.id))
    });
    apply_budget(&mut plan, policy);
    plan
}

/// Move the oldest idle owned volumes from `held` to `candidates` while the idle volumes the
/// age gate keeps exceed the policy's byte budget.
///
/// Only a volume held *solely* by its age gate qualifies: `WithinTtl` is reached after the
/// ownership, liveness and pin checks, so every one is proven ours, mounted by nothing and
/// unpinned. An unmeasured volume neither counts toward the total nor is promoted, since
/// removing it cannot be shown to bring the total down.
fn promote_over_idle_volume_budget(plan: &mut RetentionPlan, policy: RetentionPolicy) {
    let Some(budget) = policy.idle_volume_budget else {
        return;
    };
    let floor = policy.idle_volume_floor.as_secs_f64();
    let idle = |verdict: &RetentionVerdict| {
        verdict.kind == ResourceKind::Volume && verdict.hold == Some(HoldReason::WithinTtl)
    };
    let mut total: i128 = plan
        .held
        .iter()
        .filter(|verdict| idle(verdict))
        .filter_map(|verdict| verdict.bytes)
        .sum();
    if total <= budget {
        return;
    }
    let mut eligible: Vec<usize> = (0..plan.held.len())
        .filter(|&index| {
            let verdict = &plan.held[index];
            idle(verdict)
                && verdict.bytes.is_some()
                && verdict.age_seconds.is_some_and(|age| age >= floor)
        })
        .collect();
    eligible.sort_by(|&left, &right| {
        let (left, right) = (&plan.held[left], &plan.held[right]);
        right
            .age_seconds
            .unwrap_or(f64::MIN)
            .total_cmp(&left.age_seconds.unwrap_or(f64::MIN))
            .then_with(|| left.id.cmp(&right.id))
    });
    let mut promoted = Vec::new();
    for index in eligible {
        if total <= budget {
            break;
        }
        total -= plan.held[index].bytes.unwrap_or(0);
        promoted.push(index);
    }
    promoted.sort_unstable_by(|left, right| right.cmp(left));
    let mut moved: Vec<RetentionVerdict> = promoted
        .into_iter()
        .map(|index| {
            let mut verdict = plan.held.remove(index);
            verdict.hold = None;
            verdict
        })
        .collect();
    moved.reverse();
    plan.candidates.extend(moved);
}

/// Truncate the plan to `MAX_MANAGED_REMOVALS` and the optional byte ceiling.
///
/// An unmeasured size is deferred rather than counted as zero: a byte ceiling exists to bound
/// what a pass destroys, and an object whose size the engine would not report cannot be proven
/// to fit inside it. It still counts against the object cap either way.
fn apply_budget(plan: &mut RetentionPlan, policy: RetentionPolicy) {
    let mut bytes = 0_i128;
    let mut kept: Vec<RetentionVerdict> = Vec::with_capacity(plan.candidates.len());
    let mut deferred = 0_usize;

    for verdict in plan.candidates.drain(..) {
        if kept.len() >= MAX_MANAGED_REMOVALS {
            deferred += 1;
            continue;
        }
        if let Some(ceiling) = policy.max_bytes {
            // An unmeasured size is not zero. Defer it rather than assume it is cheap.
            let Some(size) = verdict.bytes else {
                deferred += 1;
                continue;
            };
            if bytes.saturating_add(size) > ceiling {
                deferred += 1;
                continue;
            }
            bytes = bytes.saturating_add(size);
            kept.push(verdict);
            continue;
        }
        bytes = bytes.saturating_add(verdict.bytes.unwrap_or(0));
        kept.push(verdict);
    }

    plan.candidates = kept;
    plan.bytes = bytes;
    plan.deferred = deferred;
}

/// Containers before volumes before images.
fn kind_rank(kind: ResourceKind) -> u8 {
    match kind {
        ResourceKind::Container => 0,
        ResourceKind::Volume => 1,
        ResourceKind::Image => 2,
        ResourceKind::Builder => 3,
        ResourceKind::Network => 4,
    }
}

/// Seconds since the Unix epoch, or `None` if the clock predates it.
///
/// A negative observation is not an age: a clock skewed into the future must fail closed rather
/// than make everything look infinitely old.
#[must_use]
pub fn age_seconds(created: SystemTime) -> Option<f64> {
    let secs = created.duration_since(UNIX_EPOCH).ok()?.as_secs_f64();
    (secs >= 0.0).then_some(secs)
}

mod eager;
mod footprint;
pub use eager::{EagerFacts, EagerReason, FAMILY_KEEP};
pub use footprint::{DEFAULT_OWNED_CEILING, OwnedFootprint};

#[cfg(test)]
mod tests;
