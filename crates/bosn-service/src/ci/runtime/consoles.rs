//! Console containers share the daemon's registry identity.
use super::CiRuntime;

impl CiRuntime {
    pub(crate) fn console_activity(
        &self,
    ) -> Option<std::sync::Arc<std::sync::atomic::AtomicUsize>> {
        self.ui
            .get()
            .map(|ui| std::sync::Arc::clone(&ui.console_activity))
    }

    pub(crate) fn active_consoles(&self) -> usize {
        self.console_activity().map_or(0, |activity| {
            activity.load(std::sync::atomic::Ordering::Acquire)
        })
    }

    pub(crate) async fn console_registry_id(&self) -> Result<String, String> {
        self.registry
            .status()
            .await
            .map(|status| status.registry_id)
            .map_err(|error| format!("registry: {error}"))
    }
}
