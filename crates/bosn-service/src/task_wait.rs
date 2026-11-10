//! Capability guard for the optional task-container wait request.
use crate::Error;

pub(crate) fn require_protocol(wait: bool, protocol: u32) -> Result<(), Error> {
    if wait && protocol < 5 {
        return Err(Error::Protocol(
            "--wait requires daemon protocol 5; stop the old daemon and retry",
        ));
    }
    Ok(())
}
