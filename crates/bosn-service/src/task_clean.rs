//! Explicit task-volume reset selection and peer capability.
use crate::Error;
use bosn_core::Scope;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CleanVolumes {
    #[default]
    None,
    Checkout,
    All,
}

#[derive(Debug, Eq, PartialEq, thiserror::Error)]
pub(crate) enum TaskCleanError {
    #[error("--clean is not supported for kept tasks yet; follow-up")]
    KeptTaskUnsupported,
}

impl CleanVolumes {
    pub(crate) fn validate_task(self, keep: bool) -> Result<(), TaskCleanError> {
        if keep && self != Self::None {
            return Err(TaskCleanError::KeptTaskUnsupported);
        }
        Ok(())
    }

    pub fn selects(self, scope: Scope) -> bool {
        match self {
            Self::None => false,
            Self::Checkout => matches!(scope, Scope::Stack | Scope::Spec),
            Self::All => true,
        }
    }

    pub fn require_protocol(self, protocol: u32) -> Result<(), Error> {
        if self != Self::None && protocol < 4 {
            return Err(Error::Protocol(
                "--clean requires daemon protocol 4; stop the old daemon and retry",
            ));
        }
        Ok(())
    }

    pub(crate) fn wire(self) -> Option<u32> {
        match self {
            Self::None => None,
            Self::Checkout => Some(1),
            Self::All => Some(2),
        }
    }

    pub(crate) fn from_wire(value: Option<u32>) -> Result<Self, Error> {
        match value {
            None => Ok(Self::None),
            Some(1) => Ok(Self::Checkout),
            Some(2) => Ok(Self::All),
            _ => Err(Error::Protocol("invalid task clean mode")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_selects_only_checkout_scopes_unless_all() {
        for scope in [Scope::Stack, Scope::Spec, Scope::Machine] {
            assert!(!CleanVolumes::None.selects(scope));
            assert_eq!(
                CleanVolumes::Checkout.selects(scope),
                scope != Scope::Machine
            );
            assert!(CleanVolumes::All.selects(scope));
        }
    }

    #[test]
    fn kept_tasks_refuse_clean_before_any_actions() {
        for mode in [CleanVolumes::Checkout, CleanVolumes::All] {
            let error = mode.validate_task(true).unwrap_err();
            assert_eq!(error, TaskCleanError::KeptTaskUnsupported);
            assert_eq!(
                error.to_string(),
                "--clean is not supported for kept tasks yet; follow-up"
            );
            assert!(mode.validate_task(false).is_ok());
        }
        assert!(CleanVolumes::None.validate_task(true).is_ok());
    }

    #[test]
    fn old_daemons_must_not_silently_ignore_clean() {
        for mode in [CleanVolumes::Checkout, CleanVolumes::All] {
            for protocol in 0..4 {
                assert!(mode.require_protocol(protocol).is_err());
            }
            assert!(mode.require_protocol(4).is_ok());
            assert_eq!(CleanVolumes::from_wire(mode.wire()).unwrap(), mode);
        }
        assert!(CleanVolumes::None.require_protocol(0).is_ok());
        assert!(CleanVolumes::from_wire(Some(3)).is_err());
    }
}
