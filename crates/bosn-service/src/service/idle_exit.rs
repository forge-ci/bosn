//! Refuse queued requests after the idle decision, without dispatching work.
use super::*;

#[cfg(test)]
pub(crate) struct IdleGate {
    pub(crate) armed: CancellationSource,
    pub(crate) entered: CancellationSource,
    pub(crate) release: CancellationSource,
    pub(crate) unfinished: Arc<std::sync::atomic::AtomicUsize>,
}

pub(super) async fn stop_and_drain(
    stop: &CancellationSource,
    #[cfg(test)] gate: &Option<IdleGate>,
    listener: &AsyncListener,
    actor: &RegistryActor,
    jobs: &JobActor,
    ci: &ci::CiRuntime,
) -> bool {
    let admission = ci.admission();
    let _guard = admission.lock().await;
    #[cfg(test)]
    if let Some(gate) = gate {
        if !gate.armed.is_cancelled() {
            return false;
        }
        gate.entered.cancel();
        async_engine::cancellable(&gate.release.token(), std::future::pending::<()>())
            .await
            .ok();
    }
    if handoff::stop_if_idle(actor, jobs, ci, stop).await.code != 30 {
        return false;
    }
    drain_backlog(listener).await;
    #[cfg(test)]
    if let Some(gate) = gate {
        gate.unfinished.store(
            jobs.unfinished().await.unwrap(),
            std::sync::atomic::Ordering::SeqCst,
        );
    }
    daemon_log!("bosn daemon: idle; exiting (containers and state are kept)");
    true
}

async fn drain_backlog(listener: &AsyncListener) {
    let mut tasks = async_engine::TaskGroup::new();
    let mut count = 0;
    while let Ok(Ok(mut stream)) = async_engine::timeout(Duration::ZERO, listener.accept()).await {
        count += 1;
        tasks.spawn(async move {
            let frame = read_frame_within(&mut stream, Duration::from_secs(1)).await?;
            let reply = ReplyWire {
                code: handoff::CODE_EXITING,
                ..Default::default()
            };
            let mut payload = Vec::new();
            reply
                .encode(&mut payload)
                .map_err(|_| Error::Protocol("reply encode"))?;
            write_frame(&mut stream, DaemonFrame::response_to(&frame, payload)).await
        });
    }
    let _ = async_engine::timeout(Duration::from_secs(2), async {
        while tasks.join_next().await.is_some() {}
    })
    .await;
    daemon_log!("bosn daemon: refused {count} queued connection(s) during idle exit");
}
