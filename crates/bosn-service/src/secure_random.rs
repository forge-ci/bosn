//! Bounded, shared secure-random admission and secret-free diagnostics.
use kernal_api::random::{RandomError, SecureRandom};
use std::{sync::OnceLock, time::Duration};

// Both pools are shared for the life of this process. A timed-out native worker
// keeps its permit until it ends; creating a fresh pool per call defeats that bound.
const MAX_IN_FLIGHT: usize = 64;
static SHORT: OnceLock<Result<SecureRandom, RandomError>> = OnceLock::new();
static LONG: OnceLock<Result<SecureRandom, RandomError>> = OnceLock::new();

#[derive(Clone, Copy)]
pub(crate) enum RandomBudget {
    Short,
    Long,
}

impl RandomBudget {
    fn seconds(self) -> u64 {
        match self {
            Self::Short => 3,
            Self::Long => 5,
        }
    }
}

pub(crate) fn shared_source(budget: RandomBudget) -> &'static Result<SecureRandom, RandomError> {
    let slot = match budget {
        RandomBudget::Short => &SHORT,
        RandomBudget::Long => &LONG,
    };
    slot.get_or_init(|| SecureRandom::new(MAX_IN_FLIGHT, Duration::from_secs(budget.seconds())))
}

pub(crate) fn diagnostic(
    operation: &'static str,
    budget: RandomBudget,
    error: RandomError,
) -> String {
    let category = match error {
        RandomError::InvalidLimits => "invalid secure-random limits",
        RandomError::TooLarge => "secure-random byte limit exceeded",
        RandomError::AtCapacity => "secure-random capacity exhausted",
        RandomError::RuntimeUnavailable => "secure-random runtime unavailable",
        RandomError::Unavailable => "operating-system secure randomness unavailable",
        RandomError::TimedOut => "secure-random deadline expired (worker may be queued or blocked)",
        RandomError::Cancelled => "secure-random request cancelled",
        RandomError::WorkerFailed => "secure-random worker failed",
    };
    format!(
        "{operation}: {category} (operation limit {}s)",
        budget.seconds()
    )
}

pub(crate) async fn bytes(
    operation: &'static str,
    budget: RandomBudget,
    len: usize,
) -> Result<Vec<u8>, String> {
    let source = shared_source(budget)
        .as_ref()
        .map_err(|error| diagnostic(operation, budget, *error))?;
    map_result(operation, budget, source.bytes(len)).await
}

pub(crate) async fn map_result(
    operation: &'static str,
    budget: RandomBudget,
    random: impl std::future::Future<Output = Result<Vec<u8>, RandomError>>,
) -> Result<Vec<u8>, String> {
    random
        .await
        .map_err(|error| diagnostic(operation, budget, error))
}
