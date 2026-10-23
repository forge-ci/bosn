//! Eager reclamation of idle volumes that can never be reused, or that a repository family
//! has too many of (#659).
//!
//! A stack or spec volume's identity hashes its workspace path, so every worktree or temporary
//! checkout mints a fresh volume, and the age gate kept each one for two weeks. Two facts the
//! caller observes let the planner reclaim them early:
//!
//! 1. **Workspace gone.** A volume whose recorded workspace no longer exists can never be
//!    mounted again: no ensure can reproduce its identity.
//! 2. **Family cap.** Checkouts of one clone (the same git common directory) are one family.
//!    Per family and stack, only the volumes of the [`FAMILY_KEEP`] most recently used
//!    workspaces are kept; older checkouts' idle volumes are reclaimed. A workspace with no
//!    observed family is never grouped by guessing.
//!
//! Only a volume held *solely* by its age gate qualifies, exactly as for the byte budget:
//! `WithinTtl` is reached after the ownership, liveness and pin checks. The idle floor still
//! applies, and the pre-removal re-check still re-proves every gate.

use std::collections::{BTreeMap, BTreeSet};

use super::{HoldReason, RetentionPlan, RetentionPolicy, RetentionVerdict};
use crate::{LABEL_SCOPE, LABEL_STACK, LABEL_WORKSPACE, ObservedArtifact, ResourceKind, Scope};

/// Workspaces per repository family (and stack) whose idle volumes are kept.
pub const FAMILY_KEEP: usize = 3;

/// Why a volume was reclaimed before its age gate.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum EagerReason {
    /// Its recorded workspace directory no longer exists.
    WorkspaceGone,
    /// Its workspace is older than the family's [`FAMILY_KEEP`] most recently used ones.
    FamilyCap,
}

impl EagerReason {
    pub const ALL: [Self; 2] = [Self::WorkspaceGone, Self::FamilyCap];

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::WorkspaceGone => "workspace-gone",
            Self::FamilyCap => "family-cap",
        }
    }

    /// The inverse of [`Self::as_str`].
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|reason| reason.as_str() == value)
    }
}

/// What the caller observed about the workspaces named by volume labels. The planner never
/// touches the filesystem.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EagerFacts {
    /// Workspaces proven absent (not found, as opposed to unreadable).
    pub gone_workspaces: BTreeSet<String>,
    /// Workspace → its repository family (the canonical git common directory).
    pub families: BTreeMap<String, String>,
}

/// The workspace and stack of a volume whose identity depends on its workspace.
struct Key {
    workspace: String,
    stack: String,
}

fn key(artifact: &ObservedArtifact) -> Option<Key> {
    if artifact.kind != ResourceKind::Volume {
        return None;
    }
    let scope = Scope::parse(artifact.labels.get(LABEL_SCOPE)?)?;
    if !matches!(scope, Scope::Stack | Scope::Spec) {
        return None;
    }
    let workspace = artifact.labels.get(LABEL_WORKSPACE)?;
    if workspace.is_empty() {
        return None;
    }
    Some(Key {
        workspace: workspace.clone(),
        stack: artifact.labels.get(LABEL_STACK)?.clone(),
    })
}

/// Move held volumes the facts prove obsolete from `held` to `candidates`, tagged with why.
pub(super) fn promote(
    plan: &mut RetentionPlan,
    artifacts: &[ObservedArtifact],
    policy: RetentionPolicy,
    facts: &EagerFacts,
) {
    let keys: BTreeMap<&str, Key> = artifacts
        .iter()
        .filter_map(|artifact| key(artifact).map(|key| (artifact.id.as_str(), key)))
        .collect();
    let idle = |verdict: &RetentionVerdict| {
        verdict.kind == ResourceKind::Volume && verdict.hold == Some(HoldReason::WithinTtl)
    };
    let floor = policy.idle_volume_floor.as_secs_f64();

    // Per (family, stack), the most recent use of each workspace among its idle volumes.
    let mut newest: BTreeMap<(&str, &str), BTreeMap<&str, f64>> = BTreeMap::new();
    for verdict in plan.held.iter().filter(|verdict| idle(verdict)) {
        let (Some(key), Some(age)) = (keys.get(verdict.id.as_str()), verdict.age_seconds) else {
            continue;
        };
        let Some(family) = facts.families.get(&key.workspace) else {
            continue;
        };
        let slot = newest
            .entry((family.as_str(), key.stack.as_str()))
            .or_default()
            .entry(key.workspace.as_str())
            .or_insert(f64::MAX);
        *slot = slot.min(age);
    }
    let mut over_cap: BTreeSet<(String, String)> = BTreeSet::new();
    for ((_, stack), workspaces) in &newest {
        let mut ranked: Vec<(&str, f64)> = workspaces.iter().map(|(w, a)| (*w, *a)).collect();
        ranked.sort_by(|left, right| left.1.total_cmp(&right.1).then(left.0.cmp(right.0)));
        for (workspace, _) in ranked.into_iter().skip(FAMILY_KEEP) {
            over_cap.insert(((*workspace).to_owned(), (*stack).to_owned()));
        }
    }

    let mut index = 0;
    while index < plan.held.len() {
        let verdict = &plan.held[index];
        let reason = keys
            .get(verdict.id.as_str())
            .filter(|_| idle(verdict) && verdict.age_seconds.is_some_and(|age| age >= floor))
            .and_then(|key| {
                if facts.gone_workspaces.contains(&key.workspace) {
                    Some(EagerReason::WorkspaceGone)
                } else if over_cap.contains(&(key.workspace.clone(), key.stack.clone())) {
                    Some(EagerReason::FamilyCap)
                } else {
                    None
                }
            });
        if let Some(reason) = reason {
            let mut verdict = plan.held.remove(index);
            verdict.hold = None;
            verdict.eager = Some(reason);
            plan.candidates.push(verdict);
        } else {
            index += 1;
        }
    }
}
