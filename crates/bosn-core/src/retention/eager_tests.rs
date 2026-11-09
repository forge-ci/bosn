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
        image_containers: BTreeMap::new(),
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

// #663: explicit image labels are the only task-family evidence.
fn image_workspace() -> &'static str {
    if cfg!(windows) { "C:/repo" } else { "/repo" }
}

fn family_images() -> Vec<ObservedArtifact> {
    (0..5)
        .map(|index| {
            let mut image = stack_volume(
                &format!("image-{index}"),
                image_workspace(),
                (index + 2).into(),
            );
            image.kind = ResourceKind::Image;
            image
                .labels
                .insert(crate::LABEL_KIND.into(), "image".into());
            image
                .labels
                .insert(crate::LABEL_TASK.into(), "build".into());
            image
        })
        .collect()
}

#[test]
fn image_family_cap_keeps_three_and_reports_two() {
    let plan = plan_managed_with(
        &family_images(),
        Some(REGISTRY),
        RetentionPolicy::default(),
        &EagerFacts::default(),
    );
    assert_eq!(
        ids(&plan),
        vec![
            ("image-4", Some(EagerReason::FamilyCap)),
            ("image-3", Some(EagerReason::FamilyCap))
        ]
    );
    assert_eq!(plan.eager_counts()[&EagerReason::FamilyCap], 2);
}

#[test]
fn image_family_cap_preserves_live_pinned_young_and_unlabelled_images() {
    for mode in 0..4 {
        let mut images = family_images();
        match mode {
            0 => images[4].signals.in_use = true,
            1 => {
                images[4]
                    .labels
                    .insert(LABEL_RETENTION.into(), "pinned".into());
            }
            2 => images[4].age_seconds = Some(30.0),
            _ => {
                images[4].labels.remove(crate::LABEL_TASK);
            }
        }
        let plan = plan_managed_with(
            &images,
            Some(REGISTRY),
            RetentionPolicy::default(),
            &EagerFacts::default(),
        );
        assert!(!plan.candidates.iter().any(|item| item.id == "image-4"));
    }
}

#[test]
fn image_family_cap_separates_tasks_and_shares_repository_family() {
    let mut images = family_images();
    images.truncate(3);
    let mut other = images.clone();
    for image in &mut other {
        image.id.push_str("-other");
        image.labels.insert(crate::LABEL_TASK.into(), "test".into());
    }
    images.extend(other);
    assert!(
        plan_managed_with(
            &images,
            Some(REGISTRY),
            RetentionPolicy::default(),
            &EagerFacts::default()
        )
        .candidates
        .is_empty()
    );
    images = family_images();
    let worktree = if cfg!(windows) {
        "C:/worktree"
    } else {
        "/worktree"
    };
    images[4]
        .labels
        .insert(LABEL_WORKSPACE.into(), worktree.into());
    let facts = facts(
        &[],
        &[(image_workspace(), "/common"), (worktree, "/common")],
    );
    assert_eq!(
        plan_managed_with(&images, Some(REGISTRY), RetentionPolicy::default(), &facts)
            .candidates
            .len(),
        2
    );
}

#[test]
fn image_family_cap_removes_superseded_setup_container_before_image() {
    let mut images = family_images();
    images[4].signals.in_use = true;
    let mut container = artifact(
        "setup-old",
        ResourceKind::Container,
        Some(30.0 * 86400.0),
        false,
        &[("com.zackees.bosn.setup-managed", "v1")],
    );
    let mut facts = EagerFacts::default();
    facts
        .image_containers
        .insert("image-4".into(), vec![container.id.clone()]);
    images.push(container.clone());
    let plan = plan_managed_with(&images, Some(REGISTRY), RetentionPolicy::default(), &facts);
    assert_eq!(plan.candidates[0].id, "setup-old");
    assert!(plan.candidates.iter().any(|item| item.id == "image-4"));
    container.signals.in_use = true;
    *images.last_mut().unwrap() = container;
    let plan = plan_managed_with(&images, Some(REGISTRY), RetentionPolicy::default(), &facts);
    assert!(!plan.candidates.iter().any(|item| item.id == "image-4"));
}
