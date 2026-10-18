//! The selection command surface: the projection and the policy transitions.
//!
//! This leaf routes the `selection.*` methods onto the owners that already hold
//! the facts. It owns no selection state and re-implements no transition:
//!
//! * `selection.matrix` renders the document
//!   [`licoup_native::domain::targets::model_selection_matrix_document`]
//!   produces — the same document the desktop `ModelSelectionProjection`
//!   parses — from the observed catalogue and the composed scope admission.
//! * `selection.policy.get` reports the binding
//!   [`licoup_native::domain::client_conversation::current_binding`] captures at
//!   the admission boundary.
//! * `selection.policy.adopt`, `selection.policy.supersede` and
//!   `selection.policy.revoke` are the three transitions over the one durable
//!   register owned by `domain::client_conversation::selection_policy`. Each one
//!   delegates to that owner's function, so the client calls the transition and
//!   never becomes a second implementer of it. No second durable store is opened
//!   here: the register keeps living in the existing client-state `settings`
//!   entry, read and written by that owner's atomic write.
//!
//! A request naming a parameter its operation does not define is refused rather
//! than partially obeyed. The named refusal a transition returns is carried
//! through unchanged as its own error code, so a client tells "nothing is
//! adopted yet" from "your revision is stale" without reading a message.

use super::*;
use licoup_native::domain::client_conversation::{
    SelectionPolicyBinding, SelectionPolicyFailure, SelectionPolicyRevision, adopt_policy,
    current_binding, revoke_policy, supersede_policy,
};
use licoup_native::ffi::generated::client_error::ClientError;

/// The parameter naming the Agent whose matrix is asked for.
const AGENT_FIELD: &str = "agent";
/// The parameter carrying the Agent inventory's own probe parameters.
const PROBE_FIELD: &str = "params";
/// The parameter carrying one revision to adopt or supersede.
const REVISION_FIELD: &str = "revision";
/// The parameter naming the revision in force to revoke.
const REVISION_ID_FIELD: &str = "revisionId";

/// The code a request this surface refuses is reported under.
const INVALID_PARAMS: &str = "invalid_params";

/// One admitted `selection.*` request.
///
/// The routed method selects the operation; parameters are validated here, so a
/// malformed frame is refused before any owner is called.
#[derive(Debug)]
pub(crate) enum SelectionRequest {
    Matrix { agent: String, params: Value },
    PolicyGet,
    PolicyAdopt { revision: SelectionPolicyRevision },
    PolicySupersede { revision: SelectionPolicyRevision },
    PolicyRevoke { revision_id: String },
}

impl SelectionRequest {
    /// Validate one routed request against the operation the method names.
    pub(crate) fn parse(operation: &str, params: Value) -> std::result::Result<Self, &'static str> {
        match operation {
            "matrix" => {
                let object = admitted_params(&params)?;
                reject_undefined_params(object, &[AGENT_FIELD, PROBE_FIELD])?;
                Ok(Self::Matrix {
                    agent: required_value(object, AGENT_FIELD)?,
                    params: optional_object(object, PROBE_FIELD),
                })
            }
            "policy.get" => {
                reject_undefined_params(admitted_params(&params)?, &[])?;
                Ok(Self::PolicyGet)
            }
            "policy.adopt" | "policy.supersede" => {
                let object = admitted_params(&params)?;
                reject_undefined_params(object, &[REVISION_FIELD])?;
                let revision = required_revision(object)?;
                Ok(if operation == "policy.adopt" {
                    Self::PolicyAdopt { revision }
                } else {
                    Self::PolicySupersede { revision }
                })
            }
            "policy.revoke" => {
                let object = admitted_params(&params)?;
                reject_undefined_params(object, &[REVISION_ID_FIELD])?;
                Ok(Self::PolicyRevoke {
                    revision_id: required_value(object, REVISION_ID_FIELD)?,
                })
            }
            _ => Err(INVALID_PARAMS),
        }
    }

    /// Execute the operation through the owner that holds the facts.
    pub(crate) fn dispatch(self) -> std::result::Result<Value, ClientError> {
        match self {
            Self::Matrix { agent, params } => {
                let matrix = licoup_native::domain::targets::model_selection_matrix_document(
                    &agent, &params,
                )
                .map_err(|_| stdio_rpc_client_error("selection_matrix_unavailable"))?;
                Ok(json!({"matrix": matrix}))
            }
            Self::PolicyGet => Ok(json!({"policy": policy_document(&current_binding())})),
            Self::PolicyAdopt { revision } => {
                let binding = adopt_policy(revision).map_err(policy_failure)?;
                Ok(json!({"policy": policy_document(&binding)}))
            }
            Self::PolicySupersede { revision } => {
                let binding = supersede_policy(revision).map_err(policy_failure)?;
                Ok(json!({"policy": policy_document(&binding)}))
            }
            Self::PolicyRevoke { revision_id } => {
                let binding = revoke_policy(&revision_id).map_err(policy_failure)?;
                Ok(json!({"policy": policy_document(&binding)}))
            }
        }
    }
}

/// The current binding as the document a client reads.
///
/// The binding is reported exactly as the owner captured it — its revision and
/// its preferences are that owner's facts, not recomputed here — and the name of
/// the revision in force is added beside them, so "nothing is adopted" is stated
/// as the `unadopted` revision instead of being an absent field a client has to
/// interpret.
pub(crate) fn policy_document(binding: &SelectionPolicyBinding) -> Value {
    let mut document =
        serde_json::to_value(binding).expect("a captured binding always serializes to an object");
    if let Some(object) = document.as_object_mut() {
        object.insert(
            "revisionName".to_owned(),
            Value::String(binding.revision_name().to_owned()),
        );
    }
    document
}

/// Carry a named transition refusal as its own error code.
fn policy_failure(failure: SelectionPolicyFailure) -> ClientError {
    stdio_rpc_client_error(failure.as_str())
}

/// The request parameters as an object.
fn admitted_params(
    params: &Value,
) -> std::result::Result<&serde_json::Map<String, Value>, &'static str> {
    params.as_object().ok_or(INVALID_PARAMS)
}

/// Refuse a parameter this operation does not define instead of ignoring it.
fn reject_undefined_params(
    params: &serde_json::Map<String, Value>,
    defined: &[&str],
) -> std::result::Result<(), &'static str> {
    if params.keys().any(|key| !defined.contains(&key.as_str())) {
        return Err(INVALID_PARAMS);
    }
    Ok(())
}

/// The non-empty identity a named parameter must carry.
fn required_value(
    params: &serde_json::Map<String, Value>,
    field: &'static str,
) -> std::result::Result<String, &'static str> {
    params
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or(INVALID_PARAMS)
}

/// The probe parameters this host passes to the Agent inventory, or none.
fn optional_object(params: &serde_json::Map<String, Value>, field: &'static str) -> Value {
    match params.get(field) {
        Some(value) if value.is_object() => value.clone(),
        _ => json!({}),
    }
}

/// The revision a transition is asked to adopt or supersede.
///
/// The owner's own type parses it, so the field names, the unknown-field refusal
/// and the identity rules the register enforces are the ones applied here too.
fn required_revision(
    params: &serde_json::Map<String, Value>,
) -> std::result::Result<SelectionPolicyRevision, &'static str> {
    let revision = params
        .get(REVISION_FIELD)
        .filter(|value| value.is_object())
        .ok_or(INVALID_PARAMS)?;
    serde_json::from_value(revision.clone()).map_err(|_| INVALID_PARAMS)
}
