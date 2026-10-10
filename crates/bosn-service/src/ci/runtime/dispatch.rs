//! Dispatcher nudges, death detection and recovery (#453): the detached loop
//! that admits queued runs, rebuilt under a gate when its task dies so a
//! closed channel can never strand eligible work.

use super::*;

impl CiRuntime {
    /// Nudge the dispatcher. A full channel is a coalesced nudge (a dispatch
    /// is already queued); a closed channel means the dispatcher task died
    /// with its receiver, so rebuild it and dispatch inline — eligible
    /// queued work must start without a daemon restart (#453).
    pub(super) fn kick(&self) {
        let sent = {
            let kick = self.dispatcher.kick.lock().unwrap_or_else(|e| e.into_inner());
            kick.try_send(())
        };
        match sent {
            Ok(()) | Err(async_engine::TrySendError::Full(())) => {}
            Err(async_engine::TrySendError::Closed(())) => self.recover_dispatcher(),
        }
    }

    /// Rebuild a dispatcher whose task died. The gate serializes recovery;
    /// a nudge that now succeeds means a concurrent kick already rebuilt
    /// the loop. The final inline dispatch covers the state change whose
    /// nudge the dead loop swallowed.
    fn recover_dispatcher(&self) {
        let _gate = self.dispatcher.recover.lock().unwrap_or_else(|e| e.into_inner());
        let receiver = {
            let mut kick = self.dispatcher.kick.lock().unwrap_or_else(|e| e.into_inner());
            match kick.try_send(()) {
                // A concurrent kick already rebuilt the loop.
                Ok(()) | Err(async_engine::TrySendError::Full(())) => return,
                Err(async_engine::TrySendError::Closed(())) => {
                    let (fresh, receiver) = async_engine::channel(64);
                    *kick = fresh;
                    receiver
                }
            }
        };
        self.dispatcher
            .recoveries
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.spawn_dispatcher(receiver);
        self.dispatch();
    }

    /// Own the dispatch loop for as long as this runtime lives. Dropping the
    /// stored task (or the runtime) cancels it; a panicked or cancelled loop
    /// leaves a closed channel that [`Self::kick`] recovers.
    pub(super) fn spawn_dispatcher(&self, mut kicked: async_engine::Receiver<()>) {
        let dispatcher = self.clone();
        let task = async_engine::launch(async move {
            while kicked.recv().await.is_some() {
                dispatcher.dispatch();
            }
        });
        *self.dispatcher.task.lock().unwrap_or_else(|e| e.into_inner()) = Some(task);
    }

    /// Whether the dispatch loop is alive (test and status view).
    pub(super) fn dispatcher_live(&self) -> bool {
        self.dispatcher
            .task
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .is_some_and(|task| !task.is_finished())
    }

    /// Fault injection for #453 tests: kill the dispatch loop the way a
    /// panic would, leaving its receiver dropped and the channel closed.
    #[cfg(test)]
    pub(crate) fn kill_dispatcher_for_test(&self) {
        if let Some(task) = self
            .dispatcher
            .task
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            task.cancel();
        }
        for _ in 0..200 {
            let closed = self
                .dispatcher
                .kick
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_closed();
            if closed {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("dispatcher receiver did not close after cancellation");
    }
}
