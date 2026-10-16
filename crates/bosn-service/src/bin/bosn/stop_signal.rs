//! A following client's first line of defense against orphaned work: an
//! interrupt (Ctrl-C) or a termination request (SIGTERM; on Windows Ctrl-Break,
//! console close and shutdown) makes the client stop the work it owns before it
//! exits. The daemon's owner sweep is the second line, for a client killed
//! outright (SIGKILL, a crash) that never got the chance.

use std::time::Duration;

use kernal_api::async_engine::{self, InterruptSignal, Runtime, TerminationSignal};

/// What a wait for a stop request saw.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StopRequest {
    /// Nothing within the period.
    None,
    /// The first request: stop the owned work and keep following it down.
    First,
    /// A later request: exit now.
    Again,
}

pub(crate) struct StopSignals<'runtime> {
    interrupt: InterruptSignal<'runtime>,
    /// `None` where the host cannot subscribe; Ctrl-C still works.
    termination: Option<TerminationSignal>,
    requests: u32,
}

impl<'runtime> StopSignals<'runtime> {
    pub(crate) fn new(runtime: &'runtime Runtime) -> Result<Self, String> {
        let interrupt = runtime
            .interrupt_signal()
            .map_err(|_| "cannot observe Ctrl-C".to_owned())?;
        let termination = runtime.run(async { TerminationSignal::new().ok() });
        Ok(Self {
            interrupt,
            termination,
            requests: 0,
        })
    }

    /// Wait up to `period` for a stop request.
    pub(crate) fn wait(&mut self, runtime: &Runtime, period: Duration) -> StopRequest {
        let Self {
            interrupt,
            termination,
            ..
        } = self;
        let requested = runtime.run(async {
            let interrupted = async_engine::timeout(period, interrupt.wait())
                .await
                .is_ok();
            let terminated = match termination {
                Some(termination) => async_engine::timeout(Duration::ZERO, termination.recv())
                    .await
                    .is_ok_and(|received| received.is_some()),
                None => false,
            };
            interrupted || terminated
        });
        if !requested {
            return StopRequest::None;
        }
        self.requests += 1;
        if self.requests == 1 {
            StopRequest::First
        } else {
            StopRequest::Again
        }
    }
}
