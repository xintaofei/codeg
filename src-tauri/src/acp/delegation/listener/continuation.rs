use super::*;

impl DelegationListener {
    /// Resolve the trusted (parent connection, parent conversation) pair.
    async fn continuation_target(
        &self,
        token: &str,
    ) -> Result<
        (String, crate::acp::delegation::continuation::VerifiedParent),
        crate::acp::delegation::continuation::ContinuationError,
    > {
        use crate::acp::delegation::continuation::{
            ContinuationError, ContinuationErrorCode, VerifiedParent,
        };
        let Some(entry) = self.tokens.lookup(token).await else {
            return Err(ContinuationError::new(
                ContinuationErrorCode::NotFoundOrForbidden,
                "no continuable source under this session",
            ));
        };
        let Some(conversation_id) = self
            .parent_lookup
            .current_conversation_id(&entry.parent_connection_id)
            .await
        else {
            return Err(ContinuationError::new(
                ContinuationErrorCode::NotFoundOrForbidden,
                "no continuable source under this session",
            ));
        };
        Ok((
            entry.parent_connection_id,
            VerifiedParent { conversation_id },
        ))
    }

    fn coordinator_or_unavailable(
        &self,
    ) -> Result<
        Arc<crate::acp::delegation::continuation::ContinuationCoordinator>,
        crate::acp::delegation::continuation::ContinuationError,
    > {
        use crate::acp::delegation::continuation::{ContinuationError, ContinuationErrorCode};
        self.collaboration.clone().ok_or_else(|| {
            ContinuationError::new(
                ContinuationErrorCode::NotFoundOrForbidden,
                "no continuable source under this session",
            )
        })
    }

    pub(super) async fn process_continue_with_session(
        &self,
        req: BrokerContinueWithSessionRequest,
    ) -> Result<serde_json::Value, crate::acp::delegation::continuation::ContinuationError> {
        use crate::acp::delegation::continuation::ContinuationError;
        let coordinator = self.coordinator_or_unavailable()?;
        let (parent_conn, parent) = self.continuation_target(&req.token).await?;
        let ack = coordinator
            .continue_turn(
                parent,
                &parent_conn,
                &req.source_task_id,
                &req.request_id,
                &req.message,
                None,
            )
            .await?;
        serde_json::to_value(ack).map_err(|e| {
            ContinuationError::new(
                crate::acp::delegation::continuation::ContinuationErrorCode::StorageUnavailable,
                format!("could not serialize ack: {e}"),
            )
        })
    }

    pub(super) async fn process_get_session_turn_status(
        &self,
        req: BrokerGetSessionTurnStatusRequest,
    ) -> Result<serde_json::Value, crate::acp::delegation::continuation::ContinuationError> {
        let coordinator = self.coordinator_or_unavailable()?;
        let (_, parent) = self.continuation_target(&req.token).await?;
        let report = coordinator
            .get_turn(parent, &req.turn_id, req.wait_ms)
            .await?;
        serde_json::to_value(report).map_err(|e| {
            crate::acp::delegation::continuation::ContinuationError::new(
                crate::acp::delegation::continuation::ContinuationErrorCode::StorageUnavailable,
                format!("could not serialize report: {e}"),
            )
        })
    }

    pub(super) async fn process_cancel_session_turn(
        &self,
        req: BrokerCancelSessionTurnRequest,
    ) -> Result<serde_json::Value, crate::acp::delegation::continuation::ContinuationError> {
        let coordinator = self.coordinator_or_unavailable()?;
        let (_, parent) = self.continuation_target(&req.token).await?;
        let report = coordinator.cancel_turn(parent, &req.turn_id).await?;
        serde_json::to_value(report).map_err(|e| {
            crate::acp::delegation::continuation::ContinuationError::new(
                crate::acp::delegation::continuation::ContinuationErrorCode::StorageUnavailable,
                format!("could not serialize report: {e}"),
            )
        })
    }

    pub(super) async fn process_close_session(
        &self,
        req: BrokerCloseSessionRequest,
    ) -> Result<serde_json::Value, crate::acp::delegation::continuation::ContinuationError> {
        let coordinator = self.coordinator_or_unavailable()?;
        let (_, parent) = self.continuation_target(&req.token).await?;
        let summary = coordinator.close_session(parent, &req.session_id).await?;
        serde_json::to_value(summary).map_err(|e| {
            crate::acp::delegation::continuation::ContinuationError::new(
                crate::acp::delegation::continuation::ContinuationErrorCode::StorageUnavailable,
                format!("could not serialize summary: {e}"),
            )
        })
    }
}

/// Serialize a continuation arm result: `Ok(dto)` renders as the schema_version=1
/// wire DTO; `Err(ContinuationError)` renders as `{ "error": ... }` — both are
/// SUCCESSFUL tool results (`isError` flags the latter), keeping business
/// rejections out of the JSON-RPC error channel.
pub(super) fn continuation_response(
    result: Result<serde_json::Value, crate::acp::delegation::continuation::ContinuationError>,
) -> std::io::Result<BrokerResponse> {
    let outcome = match result {
        Ok(dto) => dto,
        Err(e) => serde_json::json!({ "error": e }),
    };
    serde_json::to_vec(&outcome)
        .map(|_| BrokerResponse { outcome })
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, format!("encode: {e}")))
}
