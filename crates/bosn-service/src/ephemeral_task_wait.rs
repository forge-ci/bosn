//! Docker names can be held outside this daemon's admission key; no actor wakeup covers them.
use super::*;

#[derive(Debug, PartialEq)]
pub(super) enum WaitDecision {
    Ready,
    Wait,
    Refuse,
    Expired,
}

pub(super) fn wait_decision(running: bool, owned: bool, expired: bool) -> WaitDecision {
    if !owned {
        WaitDecision::Refuse
    } else if !running {
        WaitDecision::Ready
    } else if expired {
        WaitDecision::Expired
    } else {
        WaitDecision::Wait
    }
}

pub(super) fn suffix(waited: &Option<Duration>) -> String {
    waited.map_or_else(String::new, |d| format!(" (waited {}s)", d.as_secs()))
}

pub(super) fn busy_after(name: &str, elapsed: Duration) -> TaskContainerError {
    TaskContainerError::TaskContainerBusy(name.into(), Some(elapsed))
}

pub(super) async fn before_prepare(
    engine: &DockerEngine,
    plan: &SetupPlan,
    request: &ManifestAppTaskJobRequest,
    session: &dyn ManifestAppTaskSessionRecorder,
    cancellation: &async_engine::CancellationToken,
    remaining: Duration,
) -> Result<Duration, TaskContainerError> {
    let started = Instant::now();
    let workspace = plan.workspace_root.clone();
    let identity = async_engine::launch_blocking(move || workspace_identity::observe(&workspace))
        .await
        .map_err(|_| "task workspace identity worker failed")?
        .map_err(|e| e.to_string())?;
    let name = task_name(&request.task_name, &identity);
    let labels = task_labels(&session.registry_id().await?, plan, request)?;
    let result = wait_for_owned(
        engine,
        &name,
        &request.task_name,
        &labels,
        (session, cancellation, started, remaining),
    )
    .await;
    session.waiting_on(None).await?;
    result?;
    Ok(remaining.saturating_sub(started.elapsed()))
}

type WaitContext<'a> = (
    &'a dyn ManifestAppTaskSessionRecorder,
    &'a async_engine::CancellationToken,
    Instant,
    Duration,
);

async fn wait_for_owned(
    engine: &DockerEngine,
    name: &str,
    task: &str,
    labels: &ResourceLabels,
    context: WaitContext<'_>,
) -> Result<(), TaskContainerError> {
    let (session, cancellation, started, budget) = context;
    loop {
        if cancellation.is_cancelled() {
            return Err("manifest task cancelled while waiting on container".into());
        }
        let remaining = budget.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return Err(busy_after(name, started.elapsed()));
        }
        let observed = async_engine::timeout(remaining, inspect(engine, name))
            .await
            .map_err(|_| busy_after(name, started.elapsed()))??;
        let Some(observed) = observed else {
            return Ok(());
        };
        match wait_decision(
            observed.state.running,
            ours(&observed, labels, name, task),
            started.elapsed() >= budget,
        ) {
            WaitDecision::Ready => return Ok(()),
            WaitDecision::Refuse => {
                return Err(TaskContainerError::TaskContainerForeign(name.into()));
            }
            WaitDecision::Expired => return Err(busy_after(name, started.elapsed())),
            WaitDecision::Wait => {
                session.waiting_on(Some(name.into())).await?;
                async_engine::sleep(
                    Duration::from_millis(250).min(budget.saturating_sub(started.elapsed())),
                )
                .await;
            }
        }
    }
}
