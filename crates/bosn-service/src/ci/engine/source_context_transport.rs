//! Deterministic context publication and bounded machine lookup.
use super::{
    ActEngineRecord, CONTROL_DEADLINE, DockerActBackend, owned,
    source_context::{self, SourceContext},
};

impl DockerActBackend {
    pub(super) async fn observed_source_context(
        &self,
        name: &str,
    ) -> Result<(String, SourceContext), String> {
        let document = self
            .checked(
                "source context inspection",
                owned(&["container", "inspect", "--format", "{{json .}}", name]),
                CONTROL_DEADLINE,
            )
            .await?;
        source_context::parse(&document)
    }

    pub(super) async fn ensure_native_source_context(
        &self,
        record: &ActEngineRecord,
    ) -> Result<(), String> {
        let context = SourceContext::from_record(record)?;
        let created = self.run(context.create_args()?, CONTROL_DEADLINE).await;
        // A lost create reply is reconciled only through the deterministic name
        // and the complete frozen profile; never trust an unverified returned ID.
        let (id, observed) = self.observed_source_context(&context.name()).await?;
        if observed != context {
            return Err("source context conflicts with frozen intent".into());
        }
        if let Ok(result) = created
            && result.ok()
            && String::from_utf8_lossy(&result.stdout).trim() != id
        {
            return Err("source context create returned another identity".into());
        }
        Ok(())
    }

    pub(super) async fn native_source_context(
        &self,
        engine: &str,
    ) -> Result<SourceContext, String> {
        let output = self
            .checked(
                "source context lookup",
                owned(&[
                    "ps",
                    "-a",
                    "--no-trunc",
                    "--last",
                    "2",
                    "--filter",
                    &format!("label={}={engine}", source_context::ENGINE),
                    "--format",
                    "{{.ID}}",
                ]),
                CONTROL_DEADLINE,
            )
            .await?;
        if output.len() > 130 {
            return Err("source context lookup exceeds bound".into());
        }
        let ids: Vec<_> = output.split_whitespace().collect();
        let [id] = ids.as_slice() else {
            return Err("native source context missing or ambiguous".into());
        };
        if id.len() != 64
            || !id
                .bytes()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        {
            return Err("source context lookup identity invalid".into());
        }
        let (observed_id, context) = self.observed_source_context(id).await?;
        if observed_id != *id || context.recovery.engine_id != engine {
            return Err("source context lookup differs from engine".into());
        }
        Ok(context)
    }
}
