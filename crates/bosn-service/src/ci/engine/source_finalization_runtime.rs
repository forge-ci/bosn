//! A replacement daemon journals its own helper with the original frozen source.
use super::{RecoveryPin, require_finalization};
use crate::{
    RegistryActor,
    ci::engine::{
        ActEngineBackend, CONTROL_DEADLINE, DockerActBackend, ENGINE_CACHE, act_artifact,
        act_install::offline_act_script,
        cache_usage::{helper, journal},
        engine_image, owned,
        source_context::SourceContext,
    },
};
use bosn_registry::cache_helper::{CacheHelperIntent, CacheHelperRole};
use std::time::Duration;

const WORK: &str = "/tmp/bosn-recovery";
const DEADLINE: Duration = Duration::from_secs(600);

impl DockerActBackend {
    /// This proves producer finalization and helper teardown. It never invents
    /// a record in the original registry and never physically deletes storage.
    pub(in crate::ci::engine) async fn finalize_retained_source(
        &self,
        registry: &RegistryActor,
        owner: &str,
        context_id: &str,
        context: &SourceContext,
    ) -> Result<(), String> {
        let (intent, program) = self
            .prepare_source_finalizer(owner, context_id, context)
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
            let actual = self
                .verify_source_finalization_authority(context_id, context)
                .await?;
            let Some(CacheHelperRole::ToolSourceFinalizationV1 {
                source_mountpoint, ..
            }) = &intent.role
            else {
                return Err("source finalizer role missing".into());
            };
            if source_mountpoint.is_some() && actual.as_ref() != source_mountpoint.as_ref() {
                return Err("retained source mountpoint changed before helper start".into());
            }
            let output = self
                .run(owned(&["start", "--attach", &id]), DEADLINE)
                .await?;
            if !output.ok() {
                return Err(format!(
                    "source finalizer failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                ));
            }
            String::from_utf8(output.stdout).map_err(|e| format!("source finalizer encoding: {e}"))
        }
        .await;
        // The report is never physical removal authority until PID1's entire
        // mount namespace is absent, including after failed overlay cleanup.
        if self.confirm_measurement_absent(&id).await.is_err() {
            self.recover_measurement(&identity, &intent.volume, None)
                .await
                .map_err(|e| {
                    format!("source finalizer teardown unproven: {e}; outcome: {outcome:?}")
                })?;
        }
        let outcome = match outcome {
            Ok(text) => {
                self.verify_source_finalization_authority(context_id, context)
                    .await?;
                require_finalization(
                    &text,
                    &RecoveryPin::from_intent(&context.recovery)?,
                    &format!("{ENGINE_CACHE}/toolstore-v1"),
                    crate::ci::lifecycle::now_seconds(),
                )
            }
            Err(error) => Err(error),
        };
        tracker.finish(&id).await?;
        outcome
    }

    async fn prepare_source_finalizer(
        &self,
        owner: &str,
        context_id: &str,
        context: &SourceContext,
    ) -> Result<(CacheHelperIntent, String), String> {
        let source = self
            .verify_source_finalization_authority(context_id, context)
            .await?;
        if format!(
            "docker.io/library/docker@{}",
            context.intent.engine_image_digest
        ) != engine_image()
        {
            return Err("retained source engine image is not supported by this helper".into());
        }
        self.ensure_engine_image().await?;
        let arch = self
            .checked(
                "source finalizer image architecture",
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
        let act = act_artifact(&arch).ok_or("unsupported source finalizer image architecture")?;
        let source_mountpoint = context
            .recovery
            .is_live(crate::ci::lifecycle::now_seconds())
            .then_some(source)
            .flatten();
        let program = finalization_program(context, act, source_mountpoint.is_some())?;
        let intent = CacheHelperIntent {
            registry_id: owner.into(),
            nonce: crate::ci::new_uuid().await.map_err(|e| e.to_string())?,
            volume: context.recovery.cache_volume.clone(),
            image: engine_image(),
            created_at: crate::ci::lifecycle::now_seconds(),
            role: Some(CacheHelperRole::ToolSourceFinalizationV1 {
                context: Box::new(context.0.clone()),
                context_id: context_id.into(),
                source_mountpoint,
                command_sha256: helper::command_sha256(
                    &["/bin/sh".into()],
                    &["-ec".into(), program.clone()],
                )?,
            }),
        };
        intent.validate().map_err(|e| e.to_string())?;
        Ok((intent, program))
    }

    pub(in crate::ci::engine) async fn verify_source_finalization_authority(
        &self,
        id: &str,
        context: &SourceContext,
    ) -> Result<Option<String>, String> {
        context.validate()?;
        let (observed_id, observed) = self.observed_source_context(id).await?;
        if observed_id != id || observed != *context {
            return Err("source finalization context changed".into());
        }
        self.confirm_measurement_absent(&context.recovery.engine_id)
            .await?;
        self.confirm_measurement_absent(&context.intent.engine_name())
            .await?;
        self.verify_measured_volume(&context.recovery.cache_volume)
            .await?;
        if !self.volume_exists(&context.recovery.source_volume).await? {
            return Ok(None);
        }
        let document = self
            .checked(
                "retained source ownership",
                owned(&["volume", "inspect", &context.recovery.source_volume]),
                CONTROL_DEADLINE,
            )
            .await?;
        crate::act_engine::retained_source_mountpoint(
            document.as_bytes(),
            &context.intent,
            &context.recovery,
            &context.registry_id,
        )
        .map(Some)
        .map_err(|e| e.to_string())
    }
}

fn finalization_program(
    context: &SourceContext,
    act: crate::ci::engine::ActArtifact,
    mount_source: bool,
) -> Result<String, String> {
    let profile = context
        .intent
        .creation_profile
        .as_ref()
        .ok_or("source profile missing")?;
    let generation = profile
        .tool_generation
        .as_ref()
        .ok_or("source generation missing")?;
    if profile
        .cache_volume
        .as_ref()
        .is_none_or(|v| v.target != ENGINE_CACHE)
    {
        return Err("source finalizer producer binding differs".into());
    }
    let target = crate::ci::engine::toolcache::recovery_target(&generation.overlay_recipe_sha256)?;
    let pin = RecoveryPin::from_intent(&context.recovery)?.encoded()?;
    let install = offline_act_script(act, &format!("{WORK}/bin"));
    let source = if mount_source {
        format!(
            " --source-root /var/lib/docker --toolcache-target {target} --overlay-recipe-sha256 {}",
            generation.overlay_recipe_sha256
        )
    } else {
        String::new()
    };
    Ok(format!(
        "{install}; printf '%s' '{pin}' >{WORK}/intent.json; exec {WORK}/bin/act --cache-server-path {ENGINE_CACHE}/toolstore-v1 cache tool-recovery finalize-source --record {WORK}/intent.json --max-bytes {} --source-quiescent --apply{source}",
        generation.max_payload_bytes
    ))
}

impl DockerActBackend {
    /// Producer finalization is repeated even after a lost volume-removal reply.
    /// Context metadata is removed only after exact-owned source absence.
    pub(in crate::ci::engine) async fn dispose_retained_source(
        &self,
        registry: &RegistryActor,
        owner: &str,
        id: &str,
        context: &SourceContext,
    ) -> Result<(), String> {
        self.finalize_retained_source(registry, owner, id, context)
            .await?;
        let present = self
            .verify_source_finalization_authority(id, context)
            .await?;
        if present.is_some() {
            self.checked(
                "retained source removal",
                owned(&["volume", "rm", &context.recovery.source_volume]),
                crate::act_engine::DELETE,
            )
            .await?;
        }
        if self
            .verify_source_finalization_authority(id, context)
            .await?
            .is_some()
        {
            return Err("retained source absence is unproven".into());
        }
        self.checked(
            "source context removal",
            owned(&["container", "rm", "--volumes", id]),
            CONTROL_DEADLINE,
        )
        .await?;
        self.confirm_measurement_absent(id).await?;
        self.confirm_measurement_absent(&context.name()).await
    }
}
