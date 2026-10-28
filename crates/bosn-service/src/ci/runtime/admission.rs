//! Serialize CI admission with daemon idle handoff.
use super::*;

impl CiRuntime {
    pub(crate) fn admission(&self) -> Arc<async_engine::Mutex<()>> {
        Arc::clone(&self.admission)
    }

    pub(crate) fn admission_closed(&self) -> Arc<std::sync::atomic::AtomicBool> {
        Arc::clone(&self.admission_closed)
    }

    pub(super) async fn admission_guard(
        &self,
        request: &CiRequest,
    ) -> Result<Option<async_engine::MutexGuard<'_, ()>>, CiError> {
        if !matches!(request, CiRequest::Submit { .. } | CiRequest::Retry { .. }) {
            return Ok(None);
        }
        let guard = self.admission.lock().await;
        if self
            .admission_closed
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return Err(CiError::new("busy", "daemon handoff in progress"));
        }
        Ok(Some(guard))
    }
}
