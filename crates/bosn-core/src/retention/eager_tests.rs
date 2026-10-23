//! #659: idle volumes of deleted workspaces, and of a family's older checkouts, go early.

use super::*;

/// A 1 GiB idle stack volume of `workspace`, idle `hours`.
fn stack_volume(id: &str, workspace: &str, hours: f64) -> ObservedArtifact {
    let mut item = artifact(
        id,
        ResourceKind::Volume,
        Some(hours * 3600.0),
        false,
        &[
            (LABEL_SCOPE, "stack"),
            (LABEL_STACK, "cook"),
            (LABEL_WORKSPACE, workspace),
        ],
    );
    item.bytes = Some(GIB);
    item
}

fn ids(plan: &RetentionPlan) -> Vec<(&str, Option<EagerReason>)> {
    plan.candidates
        .iter()
        .map(|verdict| (verdict.id.as_str(), verdict.eager))
        .collect()
}

fn facts(gone: &[&str], families: &[(&str, &str)]) -> EagerFacts {
    EagerFacts {
        gone_workspaces: gone.iter().map(|w| (*w).to_owned()).collect(),
        families: families
            .iter()
            .map(|(w, f)| ((*w).to_owned(), (*f).to_owned()))
            .collect(),
    }
}

#[test]
fn a_volume_whose_workspace_is_gone_is_reclaimed_past_the_idle_floor() {
    let artifacts = vec![
        stack_volume("gone-old", "/w/deleted", 2.0),
        stack_volume("gone-young", "/w/deleted", 0.5),
        stack_volume("live", "/w/kept", 2.0),
    ];
    let observed = facts(&["/w/deleted"], &[]);
    // RED without the facts: everything fits the budget and waits out the 14-day gate.
    assert!(
        plan_managed(&artifacts, Some(REGISTRY), RetentionPolicy::default())
            .candidates
            .is_empty()
    );
    for policy in [
        RetentionPolicy::default(),
        RetentionPolicy::default().under_disk_pressure(),
    ] {
        let plan = plan_managed_with(&artifacts, Some(REGISTRY), policy, &observed);
        let expected = if policy.idle_volume_budget == Some(0) {
            // Under pressure the zero budget already takes every idle volume past the floor.
            vec![
                ("gone-old", Some(EagerReason::WorkspaceGone)),
                ("live", None),
            ]
        } else {
            vec![("gone-old", Some(EagerReason::WorkspaceGone))]
        };
        assert_eq!(ids(&plan), expected, "{policy:?}");
        assert!(
            plan.held.iter().any(|v| v.id == "gone-young"),
            "the idle floor holds"
        );
    }
}

#[test]
fn a_family_keeps_its_three_most_recently_used_workspaces() {
    let artifacts = vec![
        stack_volume("a", "/w/a", 1.5),
        stack_volume("b", "/w/b", 2.0),
        stack_volume("c", "/w/c", 3.0),
        stack_volume("d", "/w/d", 4.0),
        stack_volume("e", "/w/e", 5.0),
        stack_volume("other", "/x/o", 9.0),
        stack_volume("loose", "/w/no-family", 9.0),
    ];
    let family = "/clone/.git";
    let observed = facts(
        &[],
        &[
            ("/w/a", family),
            ("/w/b", family),
            ("/w/c", family),
            ("/w/d", family),
            ("/w/e", family),
            ("/x/o", "/other/.git"),
        ],
    );
    let plan = plan_managed_with(
        &artifacts,
        Some(REGISTRY),
        RetentionPolicy::default(),
        &observed,
    );
    assert_eq!(
        ids(&plan),
        [
            ("e", Some(EagerReason::FamilyCap)),
            ("d", Some(EagerReason::FamilyCap))
        ]
    );
    assert_eq!(
        plan.eager_counts(),
        BTreeMap::from([(EagerReason::FamilyCap, 2)])
    );

    let pressured = RetentionPolicy::default().under_disk_pressure();
    let plan = plan_managed_with(&artifacts, Some(REGISTRY), pressured, &observed);
    assert_eq!(
        plan.candidates.len(),
        artifacts.len(),
        "pressure takes every idle volume"
    );
    assert_eq!(plan.eager_counts()[&EagerReason::FamilyCap], 2);
}

#[test]
fn eager_rules_never_take_a_mounted_pinned_young_or_machine_volume() {
    let mut mounted = stack_volume("mounted", "/w/deleted", 9.0);
    mounted.signals.in_use = true;
    let mut pinned = stack_volume("pinned", "/w/deleted", 9.0);
    pinned
        .labels
        .insert(LABEL_RETENTION.into(), "pinned".into());
    let mut machine = stack_volume("machine", "/w/deleted", 9.0);
    machine.labels.insert(LABEL_SCOPE.into(), "machine".into());
    let artifacts = vec![
        mounted,
        pinned,
        machine,
        stack_volume("young", "/w/deleted", 0.2),
    ];
    let plan = plan_managed_with(
        &artifacts,
        Some(REGISTRY),
        RetentionPolicy::default(),
        &facts(&["/w/deleted"], &[]),
    );
    assert!(plan.candidates.is_empty(), "{:?}", ids(&plan));
}

#[test]
fn an_eager_candidate_passes_the_recheck_at_the_idle_floor() {
    let recheck = RetentionPolicy {
        idle_volume_budget: None,
        ..RetentionPolicy::default()
    }
    .recheck_gates();
    let old = stack_volume("v", "/w/deleted", 2.0);
    assert_eq!(classify_managed(&old, Some(REGISTRY), recheck).hold, None);
    let young = stack_volume("v", "/w/deleted", 0.5);
    assert_eq!(
        classify_managed(&young, Some(REGISTRY), recheck).hold,
        Some(HoldReason::WithinTtl)
    );
}

#[test]
fn eager_reasons_round_trip() {
    for reason in EagerReason::ALL {
        assert_eq!(EagerReason::parse(reason.as_str()), Some(reason));
    }
    assert_eq!(EagerReason::parse("nope"), None);
}
