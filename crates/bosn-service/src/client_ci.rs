//! The client's CI requests: one typed call per `bosn ci` verb, split
//! from `client.rs` by responsibility (the 1,000-line rule).
use super::*;

impl Client {
    /// One CI request; the reply is parsed eagerly into `T`, the request's
    /// typed reply (see [`ci::reply`]).
    pub(crate) async fn ci_call<T: serde::de::DeserializeOwned>(
        &self,
        request: ci::CiRequest,
    ) -> Result<T, Error> {
        let encoded =
            serde_json::to_string(&request).map_err(|_| Error::Protocol("ci request encode"))?;
        let deadline = request.reply_deadline();
        match self
            .call_within(
                Request {
                    ci_request: encoded,
                    ..Request::operation(36)
                },
                deadline,
            )
            .await?
        {
            Reply::Ci(json) => {
                serde_json::from_str(&json).map_err(|_| Error::Protocol("ci reply decode"))
            }
            Reply::CiError(json) => {
                let error: ci::ErrorReply =
                    serde_json::from_str(&json).map_err(|_| Error::Protocol("ci error decode"))?;
                Err(Error::Ci {
                    code: error.code,
                    message: error.message,
                })
            }
            _ => Err(Error::Protocol("unexpected ci response")),
        }
    }
    /// Snapshot `workspace` into the daemon's staging area, then submit it.
    /// The snapshot is taken here (same user) because copying a tree can
    /// take longer than one daemon request may.
    pub async fn ci_submit(&self, options: ci::SubmitOptions) -> Result<ci::SubmitReply, Error> {
        let request = ci::stage_submission(&self.state_dir, options).await?;
        let staging = ci::staging_root(&self.state_dir).join(&request.staging);
        let result = self
            .ci_call(ci::CiRequest::Submit {
                request: Box::new(request),
            })
            .await;
        // Only a refusal proves the daemon will not use the snapshot; after a
        // lost reply it may have queued the run, and its startup sweep drops
        // abandoned staging anyway.
        if daemon_refused(&result) {
            let _ = std::fs::remove_dir_all(staging);
        }
        result
    }
    pub async fn ci_list(
        &self,
        workspace: Option<String>,
        state: Option<ci::RunState>,
        limit: Option<usize>,
    ) -> Result<ci::ListReply, Error> {
        self.ci_call(ci::CiRequest::List {
            workspace,
            state,
            limit,
        })
        .await
    }
    pub async fn ci_show(&self, run: String, tree: bool) -> Result<ci::RunView, Error> {
        self.ci_call(ci::CiRequest::Show {
            run,
            tree: Some(tree),
        })
        .await
    }
    pub async fn ci_logs(&self, query: ci::LogsQuery) -> Result<ci::LogsReply, Error> {
        self.ci_call(ci::CiRequest::Logs {
            run: query.run,
            job: query.job,
            section: query.section,
            since_seq: Some(query.since_seq),
            limit: query.limit,
            max_bytes: query.max_bytes,
        })
        .await
    }
    pub async fn ci_cancel(&self, run: String) -> Result<ci::CancelReply, Error> {
        self.ci_call(ci::CiRequest::Cancel { run }).await
    }
    pub async fn ci_retry(
        &self,
        run: String,
        job: Option<String>,
    ) -> Result<ci::SubmitReply, Error> {
        self.ci_call(ci::CiRequest::Retry { run, job }).await
    }
    pub async fn ci_report(
        &self,
        run: String,
        tail: Option<usize>,
    ) -> Result<ci::RunReport, Error> {
        self.ci_call(ci::CiRequest::Report { run, tail }).await
    }
    /// A widget request (`WidgetHello`, `WidgetPoll`, `WidgetDismiss`,
    /// `WidgetCommand`); every one answers with the widget's state.
    pub async fn ci_widget(&self, request: ci::CiRequest) -> Result<ci::WidgetReply, Error> {
        self.ci_call(request).await
    }
    pub async fn ci_ui_grant(&self, path: Option<String>) -> Result<ci::UiGrantReply, Error> {
        self.ci_call(ci::CiRequest::UiGrant { path }).await
    }
    pub async fn ci_runners(&self, action: ci::RunnerAction) -> Result<ci::RunnersReply, Error> {
        self.ci_call(ci::CiRequest::Runners { action }).await
    }
    pub async fn ci_doctor(&self) -> Result<ci::DoctorReply, Error> {
        self.ci_call(ci::CiRequest::Doctor).await
    }
}
