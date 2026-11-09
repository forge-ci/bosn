//! Complete, unprotected container reference observations for image family retention.
use bosn_core::{ObservedArtifact, ResourceKind, retention::EagerFacts};
use bosn_engine::{DockerEngine, RunOptions};

pub(super) fn observe(
    engine: &DockerEngine,
    artifacts: &[ObservedArtifact],
    facts: &mut EagerFacts,
    authority: &super::authority::Authority,
) {
    let options = RunOptions::bounded(
        super::RETENTION_READ_DEADLINE,
        super::RETENTION_OUTPUT_LIMIT,
    );
    for image in artifacts.iter().filter(|item| {
        item.kind == ResourceKind::Image
            && item.signals.in_use
            && item.labels.contains_key(bosn_core::LABEL_TASK)
    }) {
        // A registry lease/session is also represented as in-use. Never mistake that protection
        // for a Docker reference we can discharge by removing a stopped setup container.
        let Some(entries) = super::parse_read::<super::ImageDetail>(
            engine.inspect_images(std::slice::from_ref(&image.id), options),
        ) else {
            continue;
        };
        let Some(entry) = entries.first() else {
            continue;
        };
        let Some(age) = entry.created_age(super::now_seconds()) else {
            continue;
        };
        if authority
            .prove_image(&entry.id, &entry.repo_tags, entry.labels(), age, false)
            .in_use
        {
            continue;
        }
        if let Ok(ids) = engine.container_ids_using_image(&image.id, options) {
            facts.image_containers.insert(image.id.clone(), ids);
        }
    }
}
