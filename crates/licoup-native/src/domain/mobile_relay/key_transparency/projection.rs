use serde_json::{Value, json};

pub(super) fn authority_challenge_response(challenge: &Value, schema_version: u32) -> Value {
    json!({
        "ok": true,
        "schemaVersion": schema_version,
        "status": "confirmation_required",
        "authorityChallengeId": challenge["challengeId"],
        "proposalDigest": challenge["proposalDigest"],
        "expiresAtEpochSeconds": challenge["expiresAtEpochSeconds"],
        "requiresSecurityReset": challenge["requiresSecurityReset"],
        "requiresUserPresence": true,
        "directoryResponseAccepted": false,
    })
}

pub(super) fn authority_configuration_response(
    schema_version: u32,
    provenance: &str,
    mock: bool,
    production_authority: bool,
    authority_changed: bool,
    already_committed: bool,
) -> Value {
    let mut response = json!({
        "ok": true,
        "schemaVersion": schema_version,
        "authorityProvenance": provenance,
        "mock": mock,
        "productionAuthority": production_authority,
        "scopeCommitted": true,
        "authorityChanged": authority_changed,
        "directoryResponseAccepted": false
    });
    if already_committed {
        response["alreadyCommitted"] = json!(true);
    }
    response
}
