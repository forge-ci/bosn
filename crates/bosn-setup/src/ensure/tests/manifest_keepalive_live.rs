//! Opt-in test against a selected Docker endpoint, without a Bosn daemon.
use super::*;
use std::process::{Command, Output};

fn docker(args: &[&str]) -> Output {
    Command::new("docker")
        .args(args)
        .output()
        .expect("docker command")
}

/// Remove only this test's exact resources after checking their immutable
/// image identity and unique labels. Unverifiable resources are left alone.
struct Cleanup {
    hash: String,
    image_id: Option<String>,
    container: Option<(String, String)>,
}
impl Drop for Cleanup {
    fn drop(&mut self) {
        if let (Some((name, id)), Some(image_id)) = (&self.container, &self.image_id) {
            let inspect = Command::new("docker").args([
                "container", "inspect", "--format",
                "{{.Id}} {{.Image}} {{index .Config.Labels \"com.zackees.bosn.setup-content-sha256\"}} {{index .Config.Labels \"com.zackees.bosn.setup-container\"}}",
                id,
            ]).output();
            if let Ok(inspect) = inspect
                && inspect.status.success()
            {
                let text = String::from_utf8_lossy(&inspect.stdout);
                let fields: Vec<_> = text.split_whitespace().collect();
                if fields.len() == 4
                    && fields[0] == id
                    && fields[1] == image_id
                    && fields[2] == self.hash
                    && fields[3] == name
                {
                    let _ = Command::new("docker")
                        .args(["container", "rm", "-f", id])
                        .output();
                }
            }
        }
        if let Some(image_id) = &self.image_id {
            let inspect = Command::new("docker")
                .args([
                    "image",
                    "inspect",
                    "--format",
                    "{{.Id}} {{index .Config.Labels \"io.bosn.issue793-test\"}}",
                    image_id,
                ])
                .output();
            if let Ok(inspect) = inspect
                && inspect.status.success()
                && String::from_utf8_lossy(&inspect.stdout).trim()
                    == format!("{} {}", image_id, self.hash)
            {
                // No force: Docker refuses if other references keep this image.
                let _ = Command::new("docker")
                    .args(["image", "rm", image_id])
                    .output();
            }
        }
    }
}

#[test]
#[ignore = "creates exact test-owned Docker images and containers"]
fn live_low_level_entrypoints_are_never_invoked() {
    for forwarding in [false, true] {
        let workspace = tempfile::tempdir().unwrap();
        std::fs::create_dir(workspace.path().join("src")).unwrap();
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let hash = format!("{stamp:064x}");
        let tag = format!("bosn-793-test:{hash}");
        assert!(
            !docker(&["image", "inspect", &tag]).status.success(),
            "test tag already exists"
        );
        let mut cleanup = Cleanup {
            hash: hash.clone(),
            image_id: None,
            container: None,
        };
        let image_id = build_image(&workspace, &tag, &hash, forwarding, &mut cleanup);
        let mut p = plan(workspace.path());
        p.content_sha256 = hash.clone();
        p.app.mounts.clear();
        p.app.workdir = None;
        p.app.command = Some(crate::MANIFEST_LINUX_IDLE_COMMAND.into());
        p.manifest_keepalive = true;
        let reference = format!("example.test/bosn-793@sha256:{hash}");
        p.app.source = bosn_core::SetupSource::PinnedImage(reference.clone());
        p.app_source = crate::SetupPlanAppSource::PinnedImage {
            image: reference.clone(),
        };
        let receipt = crate::PreparedImage {
            setup_content_sha256: hash,
            kind: crate::PreparedImageKind::PinnedImage {
                image: reference.clone(),
            },
            reference,
            observed_identity: image_id,
        };
        let name = setup_container_name(&p, workspace.path(), &receipt).unwrap();
        assert!(
            !docker(&["container", "inspect", &name]).status.success(),
            "test name already in use"
        );
        let cancellation = CancellationSource::new();
        let (events, _receiver) = channel(64);
        let result = runtime()
            .run(ensure_setup_app(
                &DockerEngine::docker(),
                SetupEnsureRequest {
                    plan: &p,
                    workspace_root: workspace.path().to_path_buf(),
                    prepared_image: &receipt,
                    options: RunOptions::streaming(Duration::from_secs(30), 8192),
                    cancellation: &cancellation.token(),
                    events: &events,
                },
            ))
            .expect("ensure unique test container");
        assert!(result.created, "refusing a reused container");
        cleanup.container = Some((name, result.container_id.clone()));
        let probe = docker(&[
            "exec",
            &result.container_id,
            "sh",
            "-c",
            "test ! -e /tmp/entrypoint-ran && echo keepalive-ok",
        ]);
        assert!(
            probe.status.success(),
            "entrypoint ran unexpectedly: {}",
            String::from_utf8_lossy(&probe.stderr)
        );
        assert!(String::from_utf8_lossy(&probe.stdout).contains("keepalive-ok"));
    }
}

fn build_image(
    workspace: &tempfile::TempDir,
    tag: &str,
    hash: &str,
    forwarding: bool,
    cleanup: &mut Cleanup,
) -> String {
    let entry = if forwarding {
        "touch /tmp/entrypoint-ran; exec \"$@\""
    } else {
        "touch /tmp/entrypoint-ran; exec sleep 3600"
    };
    let dockerfile = format!(
        "FROM alpine@sha256:28bd5fe8b56d1bd048e5babf5b10710ebe0bae67db86916198a6eec434943f8b\nRUN printf '%s\\n' '#!/bin/sh' '{entry}' > /entrypoint.sh && chmod +x /entrypoint.sh\nENTRYPOINT [\"/entrypoint.sh\"]\n"
    );
    std::fs::write(workspace.path().join("Dockerfile"), dockerfile).unwrap();
    let label = format!("io.bosn.issue793-test={hash}");
    let build = docker(&[
        "build",
        "--network=none",
        "--label",
        &label,
        "--tag",
        tag,
        workspace.path().to_str().unwrap(),
    ]);
    let image = docker(&["image", "inspect", "--format", "{{.Id}}", tag]);
    if image.status.success() {
        cleanup.image_id = Some(String::from_utf8_lossy(&image.stdout).trim().to_owned());
    }
    assert!(
        build.status.success(),
        "image build failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );
    cleanup.image_id.clone().expect("inspect test image")
}
