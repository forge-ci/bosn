//! Run inputs into the engine: the frozen source, bosn's overlay and the event
//! payload, streamed over `docker exec`'s stdin.

use std::path::Path;

use bosn_engine::RunOptions;
use kernal_api::async_engine;

use super::{ActInvocation, CONTROL_OUTPUT, DockerActBackend, ENGINE_WORK, PULL_DEADLINE};

impl DockerActBackend {
    /// Stream `file` into the engine on `docker exec`'s stdin. The engine's
    /// root is read-only and its storage is a private tmpfs, which `docker
    /// cp` cannot reach, so inputs travel the way the owned runtime's do.
    pub(super) async fn stream_in(
        &self,
        what: &str,
        engine: &str,
        file: &Path,
        script: &str,
    ) -> Result<(), String> {
        let input = std::fs::File::open(file).map_err(|e| format!("{what}: {e}"))?;
        let size = input.metadata().map_err(|e| format!("{what}: {e}"))?.len();
        let result = self
            .docker
            .with_args(Self::exec_stdin(engine, script))
            .capture_with_stdin_file_async(
                input,
                size,
                RunOptions::bounded(PULL_DEADLINE, CONTROL_OUTPUT),
                None,
            )
            .await
            .map_err(|e| format!("{what}: {e}"))?;
        if !result.ok() {
            return Err(format!(
                "{what} failed: {}",
                String::from_utf8_lossy(&result.stderr).trim()
            ));
        }
        Ok(())
    }

    /// Stream the frozen source and bosn's overlay (each as a tar) and the
    /// event payload into the engine's work directory.
    pub(super) async fn copy_inputs(
        &self,
        engine: &str,
        invocation: &ActInvocation,
        source: &Path,
        event: &Path,
    ) -> Result<(), String> {
        let root = invocation
            .scope
            .as_ref()
            .map_or_else(String::new, |scope| format!("{}/", scope.work_relative()));
        self.copy_tree(engine, source, &format!("{root}src"))
            .await?;
        let overlay = super::super::store::overlay_beside(source);
        if overlay.is_dir() {
            self.copy_tree(engine, &overlay, &format!("{root}overlay"))
                .await?;
        }
        self.stream_in(
            "event copy",
            engine,
            event,
            &format!("cat > {}/event.json", invocation.work()),
        )
        .await
    }

    /// Stream the host directory `tree` into `{ENGINE_WORK}/<into>`.
    pub(super) async fn copy_tree(
        &self,
        engine: &str,
        tree: &Path,
        into: &str,
    ) -> Result<(), String> {
        let archive = tree.with_extension("engine.tar");
        let (from, to) = (tree.to_path_buf(), archive.clone());
        let tar = async_engine::launch_blocking(move || {
            kernal_api::run_bounded_command(
                kernal_api::SpawnSpec::new("tar")
                    .arg("-C")
                    .arg(&from)
                    .arg("-cf")
                    .arg(&to)
                    .arg(".")
                    .stdin(kernal_api::StreamMode::Null)
                    .stdout(kernal_api::StreamMode::Piped)
                    .stderr(kernal_api::StreamMode::Piped),
                PULL_DEADLINE,
                CONTROL_OUTPUT,
            )
        })
        .await
        .map_err(|e| format!("{into} archive: {e}"))?
        .map_err(|e| format!("{into} archive: {e}"))?;
        if tar.exit.raw_code() != 0 {
            let _ = std::fs::remove_file(&archive);
            return Err(format!(
                "{into} archive failed: {}",
                String::from_utf8_lossy(&tar.stderr).trim()
            ));
        }
        let copied = self
            .stream_in(
                &format!("{into} copy"),
                engine,
                &archive,
                &format!("tar -xf - -C {ENGINE_WORK}/{into}"),
            )
            .await;
        let _ = std::fs::remove_file(&archive);
        copied
    }
}
