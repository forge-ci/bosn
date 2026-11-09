//! Image family retention (#663), using the same observed families as volume retention.
use std::collections::{BTreeMap, BTreeSet};

use super::{
    EagerFacts, EagerReason, HoldReason, IMAGE_FAMILY_KEEP, RetentionPlan, RetentionPolicy,
    classify_managed,
};
use crate::{
    LABEL_GENERATION, LABEL_STACK, LABEL_TASK, LABEL_WORKSPACE, ObservedArtifact, ResourceKind,
};

type Family = (String, String, String);

fn family(image: &ObservedArtifact, facts: &EagerFacts) -> Option<Family> {
    let label = |key| image.labels.get(key).filter(|value| !value.is_empty());
    let workspace = label(LABEL_WORKSPACE)?;
    if !std::path::Path::new(workspace).is_absolute() {
        return None;
    }
    Some((
        facts.families.get(workspace).unwrap_or(workspace).clone(),
        label(LABEL_STACK)?.clone(),
        label(LABEL_TASK)?.clone(),
    ))
}

fn removable_references(
    image: &ObservedArtifact,
    artifacts: &[ObservedArtifact],
    plan: &RetentionPlan,
    facts: &EagerFacts,
) -> bool {
    if !image.signals.in_use {
        return true;
    }
    let Some(references) = facts.image_containers.get(&image.id) else {
        return false;
    };
    !references.is_empty()
        && references.iter().all(|id| {
            artifacts.iter().any(|container| {
                container.kind == ResourceKind::Container
                    && container.id == *id
                    && container
                        .labels
                        .get("com.zackees.bosn.setup-managed")
                        .map(String::as_str)
                        == Some("v1")
                    && container.labels.contains_key(LABEL_GENERATION)
                    && plan.candidates.iter().any(|candidate| {
                        candidate.kind == ResourceKind::Container && candidate.id == *id
                    })
            })
        })
}

/// Promote only age-held images, or images pinned solely by already reclaimable setup containers.
pub(super) fn promote(
    plan: &mut RetentionPlan,
    artifacts: &[ObservedArtifact],
    registry: Option<&str>,
    policy: RetentionPolicy,
    facts: &EagerFacts,
) {
    let mut families: BTreeMap<Family, Vec<(&ObservedArtifact, f64)>> = BTreeMap::new();
    for image in artifacts
        .iter()
        .filter(|item| item.kind == ResourceKind::Image)
    {
        if crate::classify_ownership(&image.labels, registry) != crate::OwnershipClass::Ours {
            continue;
        }
        let (Some(key), Some(mut age)) = (family(image, facts), image.age_seconds) else {
            continue;
        };
        if let Some(references) = facts.image_containers.get(&image.id) {
            for container in artifacts
                .iter()
                .filter(|item| references.contains(&item.id))
            {
                let Some(used) = container.age_seconds else {
                    age = 0.0;
                    break;
                };
                age = age.min(used);
            }
        }
        families.entry(key).or_default().push((image, age));
    }
    let mut selected = BTreeSet::new();
    for images in families.values_mut() {
        images.sort_by(|left, right| left.1.total_cmp(&right.1).then(left.0.id.cmp(&right.0.id)));
        for (image, age) in images.iter().skip(IMAGE_FAMILY_KEEP) {
            if *age < policy.idle_volume_floor.as_secs_f64()
                || !removable_references(image, artifacts, plan, facts)
            {
                continue;
            }
            let mut idle = (*image).clone();
            idle.signals.in_use = false;
            if matches!(
                classify_managed(&idle, registry, policy).hold,
                None | Some(HoldReason::WithinTtl)
            ) {
                selected.insert(image.id.clone());
            }
        }
    }
    let mut index = 0;
    while index < plan.held.len() {
        if plan.held[index].kind == ResourceKind::Image && selected.contains(&plan.held[index].id) {
            let mut verdict = plan.held.remove(index);
            verdict.hold = None;
            verdict.eager = Some(EagerReason::FamilyCap);
            plan.candidates.push(verdict);
        } else {
            index += 1;
        }
    }
}
