use super::store::mobile_relay_pairwise_store;
use crate::domain::mobile_relay::endpoint_trust::{local_endpoint_state, session_id};
use crate::domain::mobile_relay::secret_custody::RuntimeSecretContext;
use serde_json::Value;

pub(in crate::domain::mobile_relay) struct AuthorizedPairwiseSessionStatus {
    pub(in crate::domain::mobile_relay) established: bool,
    pub(in crate::domain::mobile_relay) blocker: Option<&'static str>,
    pub(in crate::domain::mobile_relay) capability_projection: Option<Value>,
}

impl AuthorizedPairwiseSessionStatus {
    pub(in crate::domain::mobile_relay) fn blocked(blocker: &'static str) -> Self {
        Self {
            established: false,
            blocker: Some(blocker),
            capability_projection: None,
        }
    }
}

pub(in crate::domain::mobile_relay) fn authorized_pairwise_session_status(
    config: &Value,
    secret_context: &mut RuntimeSecretContext,
) -> AuthorizedPairwiseSessionStatus {
    let Ok(endpoint_id) =
        local_endpoint_state(config, &secret_context.material).map(|endpoint| endpoint.endpoint_id)
    else {
        return AuthorizedPairwiseSessionStatus::blocked("pairwise_session_missing");
    };
    let Ok(session_id) = session_id(config) else {
        return AuthorizedPairwiseSessionStatus::blocked("pairwise_session_missing");
    };
    let Ok(store) = mobile_relay_pairwise_store() else {
        return AuthorizedPairwiseSessionStatus::blocked("pairwise_session_unavailable");
    };
    let Ok(Some(_record)) = store.read_record(&session_id, &endpoint_id) else {
        return AuthorizedPairwiseSessionStatus::blocked("pairwise_session_missing");
    };
    let Ok(Some(authorization_session)) = secret_context.shared_authorization_session() else {
        return AuthorizedPairwiseSessionStatus::blocked("pairwise_session_custody_mismatch");
    };
    if authorization_session.backend() != store.secret_store_backend() {
        return AuthorizedPairwiseSessionStatus::blocked("pairwise_session_custody_mismatch");
    }
    let Ok(Some(session)) = store.load_session_with_authorized_session(
        &session_id,
        &endpoint_id,
        &authorization_session,
    ) else {
        return AuthorizedPairwiseSessionStatus::blocked("pairwise_session_unavailable");
    };
    secret_context
        .overrides
        .mark_secret_store_authorization(&authorization_session);
    if !session.handshake_confirmed() {
        return AuthorizedPairwiseSessionStatus::blocked("pairwise_handshake_incomplete");
    }
    let Some(projection) = session.capability_projection() else {
        return AuthorizedPairwiseSessionStatus::blocked("pairwise_capability_negotiation_missing");
    };
    let Ok(capability_projection) = serde_json::to_value(projection) else {
        return AuthorizedPairwiseSessionStatus::blocked("pairwise_session_unavailable");
    };
    AuthorizedPairwiseSessionStatus {
        established: true,
        blocker: None,
        capability_projection: Some(capability_projection),
    }
}
