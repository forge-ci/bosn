//! Dedicated PID1 recovery: journal before create, prove teardown before acknowledgement.
use super::{
    RecoveryPin,
    operation::{self, Operation},
};
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
use bosn_registry::cache_helper::{CacheHelperIntent, CacheHelperRole};
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
        self.run_tool_helper(registry, run, owner, Operation::Publish)
            .await
    }

    /// Release an exact reference only after durable publication. This helper
    /// never mounts the source and remains usable after expiry or disposal.
    pub async fn release_tool_with_helper(
        &self,
        registry: &RegistryActor,
        run: &str,
        owner: &str,
    ) -> Result<(), String> {
        self.run_tool_helper(registry, run, owner, Operation::Release)
            .await
    }

    /// Re-run producer exclusion on every retry, including after a lost
    /// physical removal reply. This mounts no source and proves no deletion.
    pub async fn fence_tool_source_with_helper(
        &self,
        registry: &RegistryActor,
        run: &str,
        owner: &str,
    ) -> Result<(), String> {
        self.run_tool_helper(registry, run, owner, Operation::Dispose)
            .await
    }

    async fn run_tool_helper(
        &self,
        registry: &RegistryActor,
        run: &str,
        owner: &str,
        operation: Operation,
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
        if operation::already_completed(operation, &record)? {
            // A lost database reply must never start a second publication.
            return Ok(());
        }
        let frozen = record
            .tool_recovery
            .as_ref()
            .ok_or("tool recovery intent missing")?;
        frozen.validate_record(&record).map_err(|e| e.to_string())?;
        let (intent, program) = self
            .prepare_recovery_helper(&record, owner, operation)
            .await?;
        let identity = helper::Identity::from_intent(&intent)?;
        let _active = journal::ActiveHelper::claim(self, &intent.nonce);
        let tracker = journal::Tracker::new(registry, &intent.nonce);
        tracker.begin(intent.clone()).await?;
        let id = self
            .create_recovery_helper(&identity, &intent, &program, &tracker)
            .await?;
        let outcome = async {
            tracker.register(&id).await?;
            self.verify_prepared_helper_storage(&record, owner, operation, &intent)
                .await?;
            let output = self
                .run(owned(&["start", "--attach", &id]), RECOVERY_DEADLINE)
                .await?;
            if !output.ok() {
                return Err(format!(
                    "tool recovery helper failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                ));
            }
            let receipt = std::str::from_utf8(&output.stdout)
                .map_err(|e| format!("tool recovery helper encoding: {e}"))?;
            operation::receipt(operation, receipt, &record)
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
            format!("recovery namespace teardown unproven: {e}; outcome: {outcome:?}")
        })?;
        match outcome {
            Ok(receipt) => {
                if operation == Operation::Dispose {
                    self.verify_helper_storage(&record, owner, operation)
                        .await?;
                }
                receipt.commit(registry, &record).await?;
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
        operation: Operation,
    ) -> Result<(CacheHelperIntent, String), String> {
        let frozen = record
            .tool_recovery
            .as_ref()
            .ok_or("tool recovery intent missing")?;
        if record
            .intent
            .creation_profile
            .as_ref()
            .and_then(|profile| profile.cache_volume.as_ref())
            .is_none_or(|volume| volume.target != ENGINE_CACHE)
        {
            return Err("tool recovery cache target differs from frozen store".into());
        }
        let source_mountpoint = self.verify_helper_storage(record, owner, operation).await?;
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
        let program = match operation {
            Operation::Publish => recovery_program(record, act)?,
            Operation::Release => release_program(record, act)?,
            Operation::Dispose => source_free_program(record, act, "dispose-source")?,
        };
        let arguments = vec!["-ec".into(), program.clone()];
        let intent = CacheHelperIntent {
            registry_id: owner.into(),
            nonce: crate::ci::new_uuid().await.map_err(|e| e.to_string())?,
            image: engine_image(),
            volume: frozen.cache_volume.clone(),
            created_at: crate::ci::lifecycle::now_seconds(),
            role: Some(operation::role(
                operation,
                record,
                helper::command_sha256(&["/bin/sh".into()], &arguments)?,
                source_mountpoint,
            )?),
        };
        Ok((intent, program))
    }

    async fn verify_prepared_helper_storage(
        &self,
        record: &ActEngineRecord,
        owner: &str,
        operation: Operation,
        intent: &CacheHelperIntent,
    ) -> Result<(), String> {
        // Revalidate exact source ownership and frozen path before starting PID1.
        let observed = self.verify_helper_storage(record, owner, operation).await?;
        let expected = intent
            .role
            .as_ref()
            .and_then(CacheHelperRole::recovery_binding)
            .and_then(|binding| binding.source_mountpoint);
        if observed.as_deref() != expected {
            return Err("recovery source mountpoint changed before start".into());
        }
        Ok(())
    }

    async fn verify_helper_storage(
        &self,
        record: &ActEngineRecord,
        owner: &str,
        operation: Operation,
    ) -> Result<Option<String>, String> {
        match operation {
            Operation::Publish => self.verify_recovery_source(record, owner).await.map(Some),
            Operation::Release => self
                .verify_measured_volume(
                    &record
                        .tool_recovery
                        .as_ref()
                        .ok_or("tool recovery intent missing")?
                        .cache_volume,
                )
                .await
                .map(|_| None),
            Operation::Dispose => {
                let frozen = record
                    .tool_recovery
                    .as_ref()
                    .ok_or("tool recovery intent missing")?;
                self.confirm_measurement_absent(&frozen.engine_id).await?;
                self.confirm_measurement_absent(&record.intent.engine_name())
                    .await?;
                self.verify_measured_volume(&frozen.cache_volume).await?;
                Ok(None)
            }
        }
    }

    async fn verify_recovery_source(
        &self,
        record: &ActEngineRecord,
        owner: &str,
    ) -> Result<String, String> {
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
        crate::act_engine::retained_source_mountpoint(
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
    if generation.overlay_recipe_sha256 != OVERLAY_RECIPE_SHA256 {
        return Err("tool recovery producer recipe differs".into());
    }
    let encoded = RecoveryPin::from_intent(frozen)?.encoded()?;
    let install = offline_act_script(act, &format!("{WORK}/bin"));
    Ok(format!(
        "{install}; printf '%s' '{encoded}' >{WORK}/intent.json; exec {WORK}/bin/act --cache-server-path {ENGINE_CACHE}/toolstore-v1 cache tool-recovery recover --record {WORK}/intent.json --source-root /var/lib/docker --toolcache-target /opt/hostedtoolcache --overlay-recipe-sha256 {OVERLAY_RECIPE_SHA256} --max-bytes {} --source-quiescent --apply",
        generation.max_payload_bytes
    ))
}

fn release_program(
    record: &ActEngineRecord,
    act: crate::ci::engine::ActArtifact,
) -> Result<String, String> {
    source_free_program(record, act, "release")
}

fn source_free_program(
    record: &ActEngineRecord,
    act: crate::ci::engine::ActArtifact,
    operation: &str,
) -> Result<String, String> {
    let frozen = record
        .tool_recovery
        .as_ref()
        .ok_or("tool recovery intent missing")?;
    let encoded = RecoveryPin::from_intent(frozen)?.encoded()?;
    let install = offline_act_script(act, &format!("{WORK}/bin"));
    Ok(format!(
        "{install}; printf '%s' '{encoded}' >{WORK}/intent.json; exec {WORK}/bin/act --cache-server-path {ENGINE_CACHE}/toolstore-v1 cache tool-recovery {operation} --record {WORK}/intent.json --apply"
    ))
}

fn create_args(
    identity: &helper::Identity,
    intent: &CacheHelperIntent,
    program: &str,
) -> Result<Vec<String>, String> {
    let binding = intent
        .role
        .as_ref()
        .and_then(CacheHelperRole::recovery_binding)
        .ok_or("recovery helper role missing")?;
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
        "--entrypoint",
        "/bin/sh",
        &intent.image,
        "-ec",
        program,
    ]);
    let mut phase_args = match binding.phase {
        Operation::Publish => owned(&[
            "--privileged",
            "--mount",
            &format!(
                "type=bind,source={},target=/var/lib/docker",
                binding
                    .source_mountpoint
                    .ok_or("recovery source mountpoint missing")?
            ),
        ]),
        Operation::Release | Operation::Dispose => owned(&[
            "--cap-drop",
            "ALL",
            "--tmpfs",
            "/var/lib/docker:ro,noexec,size=1m",
        ]),
    };
    phase_args.extend(identity.ownership_args());
    args.splice(2..2, phase_args);
    Ok(args)
}

#[cfg(all(test, unix))]
#[path = "tool_recovery_helper_tests.rs"]
mod tests;
