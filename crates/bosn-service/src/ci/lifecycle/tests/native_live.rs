//! Real admission on an explicitly provisioned, isolated Docker daemon.
use super::*;
use crate::ci::{engine::DockerActBackend, shared_engine::SharedEngine, spare::SparePlan};
use bosn_engine::{DockerEngine, RunOptions};

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Fixture {
    pub(crate) docker_id: String,
    pub(crate) act_version: String,
    pub(crate) report: String,
}

async fn native_policy(backend: &DockerActBackend) -> SparePlan {
    let id = crate::ci::new_uuid().await.unwrap();
    let mut plan = shared_plan(&id);
    plan.cache = CacheVolume::machine(OWNER, now_seconds()).unwrap();
    plan.intent.act_version = crate::ci::engine::ACT_VERSION.into();
    plan.intent.act_image_digest = format!("sha256:{}", plan.act.sha256);
    plan.intent.runner_image_digest = crate::ci::pins::runner_manifest().into();
    plan.intent.created_at = now_seconds();
    let limits = crate::ci::limits::size_engine(
        backend.host_resources().await.unwrap(),
        crate::ci::limits::EngineConfig {
            memory_gib: Some(4),
            storage_gib: Some(24),
            cpus: Some(2),
            ..Default::default()
        },
    )
    .unwrap();
    plan.intent.creation_profile = Some(
        crate::act_engine::creation_profile_with_cache(limits, Some(plan.cache.mount())).unwrap(),
    );
    plan.native_tools = Some(64 << 20);
    plan
}

pub(crate) async fn require_protection(registry: &RegistryActor, engine_name: &str) {
    let run = engine_name.strip_prefix("bosn-act-").unwrap();
    let record = super::super::record(registry, run).await.unwrap().unwrap();
    assert!(record.tool_recovery.is_some());
    assert!(record.tool_recovery_reserved_at.is_some());
}

pub(crate) async fn await_claim_release(shared: &SharedEngine, backend: &DockerActBackend) {
    async_engine::timeout(Duration::from_secs(180), async {
        loop {
            shared.tend_machine(Duration::ZERO).await;
            if backend.machine().unwrap().claim().await.unwrap().is_none() {
                return;
            }
            async_engine::sleep(Duration::from_secs(2)).await;
        }
    })
    .await
    .expect("production recovery and cleanup must release the retired claim");
}

#[test]
#[ignore = "requires an isolated Docker daemon with pinned image/archive and exact cleanup"]
fn actual_docker_native_policy_admits_and_reuses_warm_state_after_retirement() {
    actual_native_policy_fixture(false);
}

#[test]
#[ignore = "requires an isolated Docker daemon with pinned image/archive and exact cleanup"]
fn actual_docker_native_policy_recovers_completed_update_after_retirement() {
    actual_native_policy_fixture(true);
}

async fn fixture_engine_exec(engine: &str, script: &str) -> String {
    let output = DockerEngine::docker()
        .with_args(["exec", engine, "sh", "-ec", script])
        .capture_async(RunOptions::bounded(Duration::from_secs(30), 4096))
        .await
        .unwrap();
    assert!(output.ok(), "{}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout).unwrap()
}

const FIXTURE_CACHE: &str = "/var/lib/docker/volumes/act-toolcache/_data";

async fn write_fixture_update(engine: &str) {
    // Controlled source fixture, not a workflow-execution claim. The producer
    // has mounted the real writable overlay before admission.
    fixture_engine_exec(
        engine,
        &format!(
            "root={FIXTURE_CACHE}; \
             mkdir -p $root/Completed/2/x64 $root/Incomplete/2/x64; \
             printf completed-update >$root/Completed/2/x64/tool; \
             chmod 751 $root/Completed/2/x64/tool; \
             touch $root/Completed/2/x64.complete; \
             printf unfinished >$root/Incomplete/2/x64/tool"
        ),
    )
    .await;
}

async fn require_fixture_update(engine: &str) {
    assert_eq!(
        fixture_engine_exec(
            engine,
            &format!(
                "root={FIXTURE_CACHE}; \
                 test ! -e $root/Incomplete; \
                 test \"$(stat -c %a $root/Completed/2/x64/tool)\" = 751; \
                 cat $root/Completed/2/x64/tool"
            ),
        )
        .await,
        "completed-update"
    );
}

pub(crate) async fn fixture_backend(fixture: &Fixture) -> Arc<DockerActBackend> {
    let docker = DockerEngine::docker();
    let identity = docker
        .with_args(["info", "--format", "{{.ID}}"])
        .capture_async(RunOptions::bounded(Duration::from_secs(30), 4096))
        .await
        .unwrap();
    assert!(identity.ok());
    assert_eq!(
        String::from_utf8(identity.stdout).unwrap().trim(),
        fixture.docker_id
    );
    let backend = Arc::new(DockerActBackend::new(docker));
    let machine = backend.machine().unwrap();
    assert!(
        machine.claim().await.unwrap().is_none(),
        "fixture must have no claim"
    );
    assert!(
        machine.engines().await.unwrap().is_empty(),
        "fixture must have no engines"
    );
    backend
}

async fn native_cache_sample(
    backend: &DockerActBackend,
    registry: &RegistryActor,
) -> crate::ci::CacheUsage {
    let usage = backend
        .tracked_cache_usage("bosn-ci-cache-v1", registry, OWNER)
        .await
        .unwrap();
    assert!(
        usage.bytes.is_some() && usage.allocated_bytes.is_some() && !usage.partial,
        "{usage:?}"
    );
    assert!(usage.components.iter().any(|component| {
        component.class == crate::ci::CacheClass::Toolcache
            && component.bytes.is_some_and(|bytes| bytes > 0)
            && component.allocated_bytes.is_some_and(|bytes| bytes > 0)
    }));
    usage
}

fn actual_native_policy_fixture(install_update: bool) {
    let path = std::env::var("BOSN_NATIVE_ADMISSION_FIXTURE").unwrap();
    let fixture: Fixture = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert!(!fixture.docker_id.is_empty());
    assert_eq!(crate::ci::engine::ACT_VERSION, fixture.act_version);
    with_registry(|registry, _dir| async move {
        let backend = fixture_backend(&fixture).await;
        let machine = backend.machine().unwrap();
        let maintenance_cancel = CancellationSource::new();
        let maintenance_stop = maintenance_cancel.token();
        let maintenance_backend = backend.clone();
        let maintenance_registry = registry.clone();
        let maintenance = async_engine::launch(async move {
            maintenance_backend
                .maintain_existing_cohort(&maintenance_registry, OWNER, &maintenance_stop)
                .await;
        });
        let shared = SharedEngine::new(registry.clone(), backend.clone());
        let first_plan = native_policy(&backend).await;
        let prep_name = first_plan.intent.engine_name();
        let cancel = CancellationSource::new().token();
        let first = shared
            .lease(
                &crate::ci::new_uuid().await.unwrap(),
                first_plan,
                &cancel,
                &mut |_| {},
            )
            .await
            .unwrap();
        let binding = first
            .tool_generation
            .clone()
            .expect("ordinary admission must be native");
        require_protection(&registry, &first.engine_name).await;
        assert!(machine.inspect(&prep_name).await.unwrap().is_none());
        let first_id = first.engine_id.clone();
        if install_update {
            write_fixture_update(&first_id).await;
        }
        shared.release(first).await;
        assert!(shared.retire_idle(Duration::ZERO).await);
        await_claim_release(&shared, &backend).await;
        assert!(machine.claim().await.unwrap().is_none());
        let second = shared
            .lease(
                &crate::ci::new_uuid().await.unwrap(),
                native_policy(&backend).await,
                &cancel,
                &mut |_| {},
            )
            .await
            .unwrap();
        assert_ne!(second.engine_id, first_id);
        let replacement_binding = second.tool_generation.clone().unwrap();
        if install_update {
            assert_ne!(replacement_binding.id, binding.id);
            require_fixture_update(&second.engine_id).await;
        } else {
            assert_eq!(replacement_binding, binding);
        }
        require_protection(&registry, &second.engine_name).await;
        let second_id = second.engine_id.clone();
        shared.release(second).await;
        assert!(shared.retire_idle(Duration::ZERO).await);
        await_claim_release(&shared, &backend).await;
        assert!(machine.claim().await.unwrap().is_none());
        maintenance_cancel.cancel();
        let _ = maintenance.await;
        let cache_usage = native_cache_sample(&backend, &registry).await;
        std::fs::write(
            fixture.report,
            serde_json::to_vec(&serde_json::json!({
                "passed":true, "first_engine":first_id, "second_engine":second_id,
                "binding":replacement_binding, "initial_binding":binding,
                "completed_update":install_update, "protected_before_admission":true,
                "original_preparation_absent":true, "claims_released":true,
                "cache_usage":cache_usage,
            }))
            .unwrap(),
        )
        .unwrap();
    });
}
