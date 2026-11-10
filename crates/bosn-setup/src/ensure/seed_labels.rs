//! The final Docker ownership boundary also enforces the registry-authorized seed nonce.
//! In particular an ordinary cold plan cannot attach a racing, unfinished seeded volume.
use super::{SetupEnsureError, SetupEnsureVolume};
use bosn_core::{LABEL_SEED_NONCE, LABEL_SEED_REGISTRY, LABEL_SEEDED_FROM};
use std::collections::BTreeMap;
#[derive(serde::Deserialize)]
struct Labels {
    #[serde(rename = "Labels")]
    labels: Option<BTreeMap<String, String>>,
}
pub(super) fn verify(
    expected: &SetupEnsureVolume,
    document: &serde_json::Value,
) -> Result<(), SetupEnsureError> {
    let observed: Labels = serde_json::from_value(document.clone())
        .map_err(|_| SetupEnsureError::OwnershipMismatch)?;
    let labels = observed.labels.unwrap_or_default();
    let keys = [LABEL_SEEDED_FROM, LABEL_SEED_NONCE, LABEL_SEED_REGISTRY];
    if keys.iter().any(|k| labels.contains_key(*k))
        && keys
            .iter()
            .any(|k| labels.get(*k).is_none() || labels.get(*k) != expected.labels.get(*k))
    {
        return Err(SetupEnsureError::OwnershipMismatch);
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unfinished_or_racing_seed_is_refused_without_registry_authority() {
        let mut expected = SetupEnsureVolume {
            name: "volume".into(),
            target: "/state".into(),
            labels: BTreeMap::new(),
        };
        let labels = BTreeMap::from([
            (LABEL_SEEDED_FROM.to_string(), "source".to_string()),
            (LABEL_SEED_NONCE.into(), "nonce".into()),
            (LABEL_SEED_REGISTRY.into(), "owner".into()),
        ]);
        let observed = serde_json::json!({"Labels":labels});
        assert!(verify(&expected, &observed).is_err());
        expected.labels = labels;
        assert!(verify(&expected, &observed).is_ok());
        expected
            .labels
            .insert(LABEL_SEED_NONCE.into(), "another-attempt".into());
        assert!(verify(&expected, &observed).is_err());
        assert!(verify(&expected, &serde_json::json!({"Labels":null})).is_ok());
    }
}
