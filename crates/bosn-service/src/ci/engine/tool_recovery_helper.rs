//! Dedicated PID1 recovery: journal before create, prove teardown before acknowledgement.
use super::{RecoveryPin, require_publication};
use crate::{
    RegistryActor,
    act_registry::{ActRegistryCommand, ActRegistryReply},
    ci::engine::{
        ActEngineBackend, ActEngineRecord, CONTROL_DEADLINE, DockerActBackend, ENGINE_CACHE,
        act_artifact,
        act_install::offline_act_script,
        cache_usage::{helper, journal},
        engine_image, owned,
        toolcache::OVERLAY_RECIPE_SHA256,
    },
};
use bosn_registry::{
    act::ActToolRecoveryPublicationProof,
    cache_helper::{CacheHelperIntent, CacheHelperRole},
};
use std::time::Duration;

const WORK: &str = "/tmp/bosn-recovery";
const RECOVERY_DEADLINE: Duration = Duration::from_secs(600);

impl DockerActBackend {
    /// Recover only an already-reserved, stopped native source. This method
    /// does not enroll native engines or release the reference/source volume.
    pub async fn recover_tool_with_helper(
        &self,
        registry: &RegistryActor,
        run: &str,
        owner: &str,
    ) -> Result<(), String> {
        let record = match registry
            .act_registry(ActRegistryCommand::Get { run: run.into() })
            .await
            .map_err(|e| e.to_string())?
        {
            ActRegistryReply::Record(Some(record)) => record,
            _ => return Err("tool recovery engine record missing".into()),
        };
        if record.registry_id != owner {
            return Err("tool recovery registry differs".into());
        }
        if record.tool_recovery_completion.is_some() {
            // A lost database reply must never start a second publication.
            return Ok(());
        }
        let frozen = record
            .tool_recovery
            .as_ref()
            .ok_or("tool recovery intent missing")?;
        frozen.validate_record(&record).map_err(|e| e.to_string())?;
        let (intent, program) = self.prepare_recovery_helper(&record, owner).await?;
        let identity = helper::Identity::from_intent(&intent)?;
        let _active = journal::ActiveHelper::claim(self, &intent.nonce);
        let tracker = journal::Tracker::new(registry, &intent.nonce);
        tracker.begin(intent.clone()).await?;
        let id = self
            .create_recovery_helper(&identity, &intent, &program, &tracker)
            .await?;
        let publication = async {
            tracker.register(&id).await?;
            // Docker may recreate a missing named volume during create.
            // Check exact ownership and both source identities again before start.
            self.verify_recovery_source(&record, owner).await?;
            let output = self
                .run(owned(&["start", "--attach", &id]), RECOVERY_DEADLINE)
                .await?;
            if !output.ok() {
                return Err(format!(
                    "tool recovery publication failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                ));
            }
            let receipt = std::str::from_utf8(&output.stdout)
                .map_err(|e| format!("tool recovery publication encoding: {e}"))?;
            let pin = RecoveryPin::from_intent(frozen)?;
            require_publication(
                receipt,
                &pin,
                &format!("{ENGINE_CACHE}/toolstore-v1"),
                record.updated_at,
                crate::ci::lifecycle::now_seconds(),
            )
        }
        .await;
        // A CLI report can precede failed overlay cleanup. PID1 teardown and
        // explicit Docker absence must both precede publication authority.
        let cleanup = if self.confirm_measurement_absent(&id).await.is_ok() {
            Ok(())
        } else {
            self.recover_measurement(&identity, &intent.volume, None)
                .await
        };
        cleanup.map_err(|e| {
            format!("recovery namespace teardown unproven: {e}; publication: {publication:?}")
        })?;
        match publication {
            Ok((generation, at)) => {
                registry
                    .act_registry(ActRegistryCommand::ToolRecoveryPublished {
                        run: run.into(),
                        proof: ActToolRecoveryPublicationProof {
                            intent: frozen.clone(),
                            generation,
                        },
                        at,
                    })
                    .await
                    .map_err(|e| {
                        format!("recovery publication acknowledgement needs reconciliation: {e}")
                    })?;
                // Keep the durable disposal fence until completion commits.
                tracker.finish(&id).await
            }
            Err(error) => {
                tracker.finish(&id).await?;
                Err(error)
            }
        }
    }

    async fn create_recovery_helper(
        &self,
        identity: &helper::Identity,
        intent: &CacheHelperIntent,
        program: &str,
        tracker: &journal::Tracker<'_>,
    ) -> Result<String, String> {
        let created = self
            .checked(
                "recovery helper create",
                create_args(identity, intent, program)?,
                CONTROL_DEADLINE,
            )
            .await;
        // Canonical-name inspection independently proves the immutable ID.
        // Never register an unverified create acknowledgement: a wrong ID's
        // later absence would orphan the real helper and release its fence.
        let observed = self
            .checked(
                "recovery helper identity",
                owned(&["container", "inspect", &identity.name]),
                CONTROL_DEADLINE,
            )
            .await
            .and_then(|document| identity.verify(&document, &intent.volume));
        let id = match (created, observed) {
            (Ok(created), Ok(id)) if created == id => id,
            (created, observed) => {
                let cleanup = self
                    .recover_measurement(identity, &intent.volume, Some(tracker))
                    .await;
                return Err(format!(
                    "recovery create unacknowledged: {created:?}, {observed:?}; cleanup: {cleanup:?}"
                ));
            }
        };
        Ok(id)
    }

    async fn prepare_recovery_helper(
        &self,
        record: &ActEngineRecord,
        owner: &str,
    ) -> Result<(CacheHelperIntent, String), String> {
        let frozen = record
            .tool_recovery
            .as_ref()
            .ok_or("tool recovery intent missing")?;
        self.verify_recovery_source(record, owner).await?;
        self.ensure_engine_image().await?;
        let arch = self
            .checked(
                "recovery image architecture",
                owned(&[
                    "image",
                    "inspect",
                    "--format",
                    "{{.Architecture}}",
                    &engine_image(),
                ]),
                CONTROL_DEADLINE,
            )
            .await?;
        let act = act_artifact(&arch).ok_or("unsupported recovery image architecture")?;
        let program = recovery_program(record, act)?;
        let arguments = vec!["-ec".into(), program.clone()];
        let intent = CacheHelperIntent {
            registry_id: owner.into(),
            nonce: crate::ci::new_uuid().await.map_err(|e| e.to_string())?,
            image: engine_image(),
            volume: frozen.cache_volume.clone(),
            created_at: crate::ci::lifecycle::now_seconds(),
            role: Some(CacheHelperRole::ToolRecoveryV1 {
                run_id: record.intent.run_id.clone(),
                intent: frozen.clone(),
                command_sha256: Some(helper::command_sha256(&["/bin/sh".into()], &arguments)?),
            }),
        };
        Ok((intent, program))
    }

    async fn verify_recovery_source(
        &self,
        record: &ActEngineRecord,
        owner: &str,
    ) -> Result<(), String> {
        let frozen = record
            .tool_recovery
            .as_ref()
            .ok_or("tool recovery intent missing")?;
        self.confirm_measurement_absent(&frozen.engine_id).await?;
        self.confirm_measurement_absent(&record.intent.engine_name())
            .await?;
        self.verify_measured_volume(&frozen.cache_volume).await?;
        let document = self
            .checked(
                "recovery source ownership",
                owned(&["volume", "inspect", &frozen.source_volume]),
                CONTROL_DEADLINE,
            )
            .await?;
        crate::act_engine::verify_retained_source(
            document.as_bytes(),
            &record.intent,
            frozen,
            owner,
        )
        .map_err(|e| e.to_string())
    }
}

fn recovery_program(
    record: &ActEngineRecord,
    act: crate::ci::engine::ActArtifact,
) -> Result<String, String> {
    let frozen = record
        .tool_recovery
        .as_ref()
        .ok_or("tool recovery intent missing")?;
    let profile = record
        .intent
        .creation_profile
        .as_ref()
        .ok_or("tool recovery profile missing")?;
    let generation = profile
        .tool_generation
        .as_ref()
        .ok_or("tool recovery generation missing")?;
    if generation.overlay_recipe_sha256 != OVERLAY_RECIPE_SHA256
        || profile
            .cache_volume
            .as_ref()
            .is_none_or(|v| v.target != ENGINE_CACHE)
    {
        return Err("tool recovery producer recipe or cache target differs".into());
    }
    let encoded = RecoveryPin::from_intent(frozen)?.encoded()?;
    let install = offline_act_script(act, &format!("{WORK}/bin"));
    Ok(format!(
        "{install}; printf '%s' '{encoded}' >{WORK}/intent.json; exec {WORK}/bin/act --cache-server-path {ENGINE_CACHE}/toolstore-v1 cache tool-recovery recover --record {WORK}/intent.json --source-root /var/lib/docker --toolcache-target /opt/hostedtoolcache --overlay-recipe-sha256 {OVERLAY_RECIPE_SHA256} --max-bytes {} --source-quiescent --apply",
        generation.max_payload_bytes
    ))
}

fn create_args(
    identity: &helper::Identity,
    intent: &CacheHelperIntent,
    program: &str,
) -> Result<Vec<String>, String> {
    let Some(CacheHelperRole::ToolRecoveryV1 { intent: frozen, .. }) = &intent.role else {
        return Err("recovery helper role missing".into());
    };
    let mut args = owned(&[
        "create",
        "--rm",
        bosn_engine::LOW_CPU_PRIORITY_ARG,
        "--name",
        &identity.name,
        "--label",
        &identity.label(),
        "--pull",
        "never",
        "--network",
        "none",
        "--read-only",
        "--privileged",
        "--init=false",
        "--restart=no",
        "--memory",
        "512m",
        "--cpus",
        "2",
        "--pids-limit",
        "128",
        "--tmpfs",
        "/tmp:exec,size=64m",
        "--mount",
        &format!("type=volume,source={},target={ENGINE_CACHE}", intent.volume),
        "--mount",
        &format!(
            "type=volume,source={},target=/var/lib/docker",
            frozen.source_volume
        ),
        "--entrypoint",
        "/bin/sh",
        &intent.image,
        "-ec",
        program,
    ]);
    args.splice(2..2, identity.ownership_args());
    Ok(args)
}

#[cfg(all(test, unix))]
#[path = "tool_recovery_helper_tests.rs"]
mod tests;
