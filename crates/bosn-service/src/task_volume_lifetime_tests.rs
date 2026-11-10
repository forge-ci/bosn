//! Failed removal must not admit another task onto the volume.
use super::*;

struct Recorder;
impl ManifestAppTaskSessionRecorder for Recorder {
    fn begin(&self, _: String) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>> {
        Box::pin(async { Ok(()) })
    }
    fn finish(
        &self,
        _: &'static str,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>> {
        Box::pin(async { Ok(()) })
    }
}

#[test]
fn failed_cleanup_keeps_the_run_lease_until_confirmed_removal() {
    let root = tempfile::tempdir().unwrap();
    let name = format!(
        "cleanup-{}",
        root.path().file_name().unwrap().to_string_lossy()
    );
    let volume = ManifestVolumeResource {
        id: format!("manifest-volume:{name}"),
        name: name.clone(),
        stack: "test".into(),
        generation: "sha256:test".into(),
        scope: Scope::Stack,
        workspace: "/test".into(),
        retention: bosn_core::Retention::Warm,
        target: "/state".into(),
        labels: Default::default(),
        seed_name: None,
    };
    let modes = [(name.clone(), bosn_core::manifest::VolumeMode::Exclusive)].into();
    let marker = root.path().join("ready");
    let engine = DockerEngine::synthetic_for_test(
        "/bin/sh",
        ["-c", "test -e \"$BOSN_CLEANUP_READY\"", "fake-docker"],
    )
    .env("BOSN_CLEANUP_READY", marker.as_os_str());
    async_engine::RuntimeBuilder::current_thread()
        .enable_all()
        .build()
        .unwrap()
        .run(async {
            let cancellation = async_engine::CancellationSource::new();
            let lease = crate::volume_seed::locks::destinations(
                root.path(),
                &[volume],
                &modes,
                &async_engine::Deadline::after(Duration::from_secs(5)),
                &cancellation.token(),
                "task holder",
            )
            .await
            .unwrap();
            let lifetime = TaskLifetime {
                container: Some(TaskContainer {
                    name: "task".into(),
                    id: "owned-id".into(),
                }),
                volumes: lease,
                overlay_volumes: Vec::new(),
            };
            assert!(
                finish(&engine, lifetime, &Recorder, Ok("done".into()))
                    .await
                    .is_err()
            );
            let directory = crate::runners::cache_lock_dir();
            assert!(
                crate::volume_seed::locks::try_lock(&directory, &name)
                    .unwrap()
                    .is_none()
            );
            std::fs::write(&marker, "ready").unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                if crate::volume_seed::locks::try_lock(&directory, &name)
                    .unwrap()
                    .is_some()
                {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "cleanup did not release the lease"
                );
                async_engine::sleep(Duration::from_millis(20)).await;
            }
        });
}
