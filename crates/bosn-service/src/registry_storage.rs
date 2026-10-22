//! WAL copy/sync owns a separate blocking worker, never the registry actor.
use crate::*;
use bosn_registry::{RegistryCheckpointReport, RegistryCheckpointer};
use kernal_api::async_engine::{CancellationToken, OneshotSender, Task};

const INTERVAL: Duration = Duration::from_secs(15);
type Reply = OneshotSender<Result<RegistryCheckpointReport, bosn_registry::Error>>;

#[cfg(test)]
pub(crate) struct CheckpointGate {
    pub entered: OneshotSender<()>,
    pub release: std::sync::mpsc::Receiver<()>,
}

/// Start the blocking owner before launching the scheduler. The checkpointer
/// stays there even between passes; cancellation closes its command channel,
/// so its connection and writer lease also close on that blocking worker.
pub(crate) fn spawn(
    checkpointer: RegistryCheckpointer,
    stop: CancellationToken,
    #[cfg(test)] gate: Option<CheckpointGate>,
) -> Task<()> {
    let interval = INTERVAL;
    #[cfg(test)]
    let interval = if gate.is_some() {
        Duration::from_millis(1)
    } else {
        interval
    };
    let (send, receive) = std::sync::mpsc::sync_channel::<Reply>(1);
    let worker = async_engine::launch_blocking(move || {
        let mut checkpointer = checkpointer;
        #[cfg(test)]
        let mut gate = gate;
        while let Ok(reply) = receive.recv() {
            #[cfg(test)]
            if let Some(gate) = gate.take() {
                let _ = gate.entered.send(());
                let _ = gate.release.recv();
            }
            let _ = reply.send(checkpointer.run_pass());
        }
        drop(checkpointer);
    });
    async_engine::launch(async move {
        schedule(send, stop, interval).await;
        // No further pass can be submitted. A running copy cannot be forcibly
        // interrupted, so await its completion and connection close at shutdown.
        if worker.await.is_err() {
            daemon_log!("bosn registry: checkpoint worker failed");
        }
    })
}

async fn schedule(
    send: std::sync::mpsc::SyncSender<Reply>,
    stop: CancellationToken,
    interval: Duration,
) {
    let mut previous = None;
    while async_engine::cancellable(&stop, async_engine::sleep(interval))
        .await
        .is_ok()
    {
        let (reply, wait) = async_engine::oneshot_channel();
        if send.try_send(reply).is_err() {
            daemon_log!("bosn registry: checkpoint worker unavailable");
            break;
        }
        let result = match async_engine::cancellable(&stop, wait).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => {
                daemon_log!("bosn registry: checkpoint worker reply unavailable");
                break;
            }
            Err(_) => break,
        };
        // Log state transitions, not one line per frame/byte change.
        let state = match &result {
            Ok(report) if report.usage.partial => "allocation_unknown",
            Ok(report) if report.blocked => "reader_or_writer_held",
            Ok(_) => "ready",
            Err(_) => "failed",
        };
        if previous != Some(state) {
            match result {
                Ok(report) if state != "ready" => daemon_log!(
                    "bosn registry: checkpoint {state}; remaining_frames={} allocated_bytes={:?}",
                    report.remaining_frames,
                    report.usage.allocated_bytes,
                ),
                Ok(_) if previous.is_some() => daemon_log!("bosn registry: checkpoint ready"),
                Err(error) => daemon_log!("bosn registry: checkpoint failed: {error}"),
                _ => {}
            }
            previous = Some(state);
        }
    }
    drop(send);
}
