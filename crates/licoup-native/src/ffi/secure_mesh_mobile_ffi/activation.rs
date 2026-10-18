//! What `secure_mesh.deviceTrust.rotate|revoke|recover` do with durable evidence.
//!
//! The three lifecycle actions were stateless policy evaluators: they mutated the
//! caller's own JSON and returned a decision, so a caller who said `revoke`
//! received `"status": "revoked"` and nothing was recorded. `super::policy`'s own
//! comment states why it cannot be more than that — it receives caller-controlled
//! JSON and so cannot establish a trust root (`policy.rs:38-40`).
//!
//! This module is the missing half. It writes the durable activation or
//! revocation through [`SecureMeshDeviceActivationStore`], and the policy result
//! the caller sees is then decided *from that ledger* rather than from the
//! request:
//!
//! * a lifecycle request that carries no durable evidence writes nothing and
//!   keeps the policy result it always had, so the existing command surface and
//!   its responses are unchanged;
//! * a lifecycle request that carries durable evidence is written first, and only
//!   a write that really happened turns the destination `active` (a rotation or a
//!   recovery) or `revoked` (a revocation). The reported state is read back from
//!   the ledger, so it cannot disagree with what was persisted;
//! * a refused write is reported with its own stable code and a refusal reason,
//!   and the caller's own request is never echoed back as the answer.
//!
//! # What this does and does not verify
//!
//! The store checks the evidence against the ledger it holds: the epoch only
//! advances, a destination that is already active is never replaced in, an
//! activation never names its own source as its destination, and the state is
//! projection-consistent with the row written beside it. Cryptographic
//! verification of the signed transition and the possession proof belongs to the
//! pinned SDK, reached through
//! `licoup_native::domain::mobile_relay::endpoint_ports::admit_replacement`.
//! A lifecycle call therefore records a decision an authorized flow already
//! reached; it is not itself a verifier, and the record it writes says which
//! authority state it descends from so that verification can be re-driven.

use anyhow::Result;
use serde_json::{Value, json};

#[cfg(test)]
use crate::core::secure_mesh_trust::DeviceActivationLookup;
use crate::core::secure_mesh_trust::{
    DeviceActivationRefusal, DeviceActivationState, SecureMeshDeviceActivationStore,
    device_activation_ledger_path, device_activation_request_from_json,
};
use crate::platform::client_state::ClientStateStore;

/// The lifecycle actions that own a durable write.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeviceTrustLifecycle {
    /// Admit a new destination identity on authority that supersedes the source.
    Rotate,
    /// Withdraw a destination identity from new admission.
    Revoke,
    /// Admit a destination on recovery authority.
    Recover,
}

impl DeviceTrustLifecycle {
    /// The wire name of this lifecycle action.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Rotate => "rotate",
            Self::Revoke => "revoke",
            Self::Recover => "recover",
        }
    }

    /// The state a successful write of this action records.
    #[must_use]
    const fn state(self) -> DeviceActivationState {
        match self {
            Self::Rotate | Self::Recover => DeviceActivationState::Active,
            Self::Revoke => DeviceActivationState::Revoked,
        }
    }
}

impl DeviceActivationRefusal {
    /// The response the caller receives when the ledger refused the write.
    ///
    /// It states plainly that nothing was persisted, so a refused lifecycle call
    /// can never be read as a completed one.
    #[must_use]
    pub fn to_json(self) -> Value {
        json!({
            "ok": false,
            "persisted": false,
            "code": self.code(),
            "reason": self.reason()
        })
    }
}

/// Writes the durable half of one lifecycle action, if it carries evidence.
///
/// `None` means the request carried no durable evidence: nothing is opened,
/// nothing is written, and the caller keeps the policy-only result it always had.
pub fn apply_device_trust_lifecycle(
    params: &Value,
    lifecycle: DeviceTrustLifecycle,
) -> Result<Option<Value>> {
    let Some(evidence) = params.get("durableActivation") else {
        return Ok(None);
    };
    if evidence.is_null() {
        return Ok(None);
    }
    let request = device_activation_request_from_json(evidence)?;
    // The action decides the state; evidence may not disagree with the command
    // that carries it.
    if request.state != lifecycle.state() {
        return Ok(Some(json!({
            "ok": false,
            "persisted": false,
            "code": "lifecycle_state_mismatch",
            "reason": format!(
                "a {} request can only record the {} state",
                lifecycle.as_str(),
                lifecycle.state().as_str()
            )
        })));
    }
    let outcome = write_activation(request)?;
    Ok(Some(outcome))
}

fn write_activation(
    request: crate::core::secure_mesh_trust::DeviceActivationRequest,
) -> Result<Value> {
    let mut store = open_activation_store()?;
    // What the ledger holds now, read before the write, so the response can say
    // whether this call was the one that admitted the destination.
    let before = store
        .lookup(
            &request.subject_identity_ref,
            &request.endpoint_identity_ref,
        )?
        .is_present();
    match store.apply(&request) {
        Ok(record) => Ok(json!({
            "ok": true,
            "persisted": true,
            "code": "recorded",
            "reason": "the durable device ledger recorded this decision",
            "record": record.to_json(),
            "newlyAdmitted": !before && record.admits_new_sessions()
        })),
        Err(refusal) => Ok(refusal.to_json()),
    }
}

/// What the ledger says about one destination, without writing anything.
///
/// It is how a later read reports authority a previous lifecycle call recorded,
/// so a caller never has to trust its own request as the answer.
#[cfg(test)]
pub fn device_activation_state_json(subject: &str, endpoint: &str) -> Result<Value> {
    let store = open_activation_store()?;
    let lookup = store.lookup(subject, endpoint)?;
    Ok(match &lookup {
        DeviceActivationLookup::Found(record) => json!({
            "present": true,
            "admitsNewSessions": record.admits_new_sessions(),
            "record": record.to_json()
        }),
        DeviceActivationLookup::Absent => json!({
            "present": false,
            "admitsNewSessions": lookup.admits_new_sessions(),
            "reason": "the subject's device ledger holds no record for this destination"
        }),
        DeviceActivationLookup::EmptyLedger => json!({
            "present": false,
            "admitsNewSessions": lookup.admits_new_sessions(),
            "reason": "the subject has recorded no device activation at all"
        }),
    })
}

fn open_activation_store() -> Result<SecureMeshDeviceActivationStore> {
    let root = ClientStateStore::portable()?.root().to_path_buf();
    SecureMeshDeviceActivationStore::open(device_activation_ledger_path(&root))
}

#[cfg(test)]
mod tests {
    use super::{DeviceTrustLifecycle, apply_device_trust_lifecycle, device_activation_state_json};
    use serde_json::{Value, json};
    use std::path::PathBuf;

    /// One disposable portable data root, so the ledger is a real file the test
    /// can read back through the same code path production uses.
    struct DataRoot {
        previous: Option<PathBuf>,
        root: PathBuf,
    }

    impl DataRoot {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "lico-device-trust-{name}-{}-{}",
                std::process::id(),
                time::OffsetDateTime::now_utc().unix_timestamp_nanos()
            ));
            std::fs::create_dir_all(&root).expect("data root creates");
            let previous = licoup_foundation::platform::paths::set_portable_data_dir_override(
                Some(root.clone()),
            );
            Self { previous, root }
        }
    }

    impl Drop for DataRoot {
        fn drop(&mut self) {
            licoup_foundation::platform::paths::set_portable_data_dir_override(
                self.previous.take(),
            );
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn evidence(
        endpoint: &str,
        source: Option<&str>,
        state: &str,
        accepted_epoch: u64,
        expected_epoch: Option<u64>,
    ) -> Value {
        let authority_state = json!({
            "recordType": "userAuthorityState",
            "authorityEpoch": accepted_epoch,
            "synthetic": true
        });
        json!({
            "subjectIdentityRef": "subject-a",
            "endpointIdentityRef": endpoint,
            "sourceEndpointIdentityRef": source,
            "state": state,
            "acceptedAuthorityEpoch": accepted_epoch,
            "supersededAuthorityEpoch": expected_epoch,
            "authorityState": authority_state,
            "expectedAuthorityEpoch": expected_epoch
        })
    }

    #[test]
    fn a_lifecycle_call_without_evidence_writes_nothing() {
        let _root = DataRoot::new("no-evidence");
        let outcome = apply_device_trust_lifecycle(
            &json!({"identity": {"endpointId": "device-a"}}),
            DeviceTrustLifecycle::Revoke,
        )
        .expect("a policy-only call still succeeds");
        assert!(
            outcome.is_none(),
            "without durable evidence the command keeps its policy-only result"
        );
        // And nothing at all was recorded, which the later read confirms.
        let read = device_activation_state_json("subject-a", "device-a").expect("read runs");
        assert_eq!(read["present"], json!(false));
        assert_eq!(read["admitsNewSessions"], json!(false));
    }

    #[test]
    fn a_rotation_records_the_new_destination_and_reports_it_from_the_ledger() {
        let _root = DataRoot::new("rotate");
        let outcome = apply_device_trust_lifecycle(
            &json!({"durableActivation": evidence(
                "device-new", Some("device-old"), "active", 1, None
            )}),
            DeviceTrustLifecycle::Rotate,
        )
        .expect("the rotation succeeds")
        .expect("durable evidence is present");

        assert_eq!(outcome["ok"], json!(true));
        assert_eq!(outcome["persisted"], json!(true));
        assert_eq!(outcome["newlyAdmitted"], json!(true));
        assert_eq!(outcome["record"]["state"], json!("active"));
        assert_eq!(outcome["record"]["acceptedAuthorityEpoch"], json!(1));

        let read = device_activation_state_json("subject-a", "device-new").expect("read runs");
        assert_eq!(read["present"], json!(true));
        assert_eq!(read["admitsNewSessions"], json!(true));
    }

    #[test]
    fn a_revocation_records_the_withdrawal_and_not_the_caller_s_request() {
        let _root = DataRoot::new("revoke");
        apply_device_trust_lifecycle(
            &json!({"durableActivation": evidence(
                "device-new", Some("device-old"), "active", 1, None
            )}),
            DeviceTrustLifecycle::Rotate,
        )
        .expect("the rotation succeeds");

        let mut revoke = evidence("device-new", None, "revoked", 2, Some(1));
        revoke["cleanup"] = json!({"requested": true});
        let outcome = apply_device_trust_lifecycle(
            &json!({"durableActivation": revoke}),
            DeviceTrustLifecycle::Revoke,
        )
        .expect("the revocation succeeds")
        .expect("durable evidence is present");

        assert_eq!(outcome["ok"], json!(true));
        assert_eq!(outcome["record"]["state"], json!("revoked"));
        assert_eq!(outcome["record"]["cleanup"]["requested"], json!(true));
        assert_eq!(outcome["record"]["cleanup"]["outstanding"], json!(true));

        let read = device_activation_state_json("subject-a", "device-new").expect("read runs");
        assert_eq!(read["admitsNewSessions"], json!(false));
    }

    #[test]
    fn a_command_may_not_record_a_state_other_than_its_own() {
        let _root = DataRoot::new("mismatch");
        let outcome = apply_device_trust_lifecycle(
            &json!({"durableActivation": evidence(
                "device-new", None, "revoked", 1, None
            )}),
            DeviceTrustLifecycle::Rotate,
        )
        .expect("the call is answered")
        .expect("durable evidence is present");
        assert_eq!(outcome["ok"], json!(false));
        assert_eq!(outcome["persisted"], json!(false));
        assert_eq!(outcome["code"], json!("lifecycle_state_mismatch"));
        assert_eq!(
            device_activation_state_json("subject-a", "device-new").expect("read runs")["present"],
            json!(false)
        );
    }

    #[test]
    fn replacing_an_already_active_destination_is_refused_and_reported() {
        let _root = DataRoot::new("already-active");
        apply_device_trust_lifecycle(
            &json!({"durableActivation": evidence(
                "device-new", Some("device-old"), "active", 1, None
            )}),
            DeviceTrustLifecycle::Rotate,
        )
        .expect("the first rotation succeeds");

        let outcome = apply_device_trust_lifecycle(
            &json!({"durableActivation": evidence(
                "device-new", Some("device-other"), "active", 2, Some(1)
            )}),
            DeviceTrustLifecycle::Rotate,
        )
        .expect("the call is answered")
        .expect("durable evidence is present");
        assert_eq!(outcome["ok"], json!(false));
        assert_eq!(outcome["persisted"], json!(false));
        assert_eq!(outcome["code"], json!("destination_already_active"));

        // The first record still stands, unchanged.
        let read = device_activation_state_json("subject-a", "device-new").expect("read runs");
        assert_eq!(read["record"]["acceptedAuthorityEpoch"], json!(1));
        assert_eq!(read["record"]["stateVersion"], json!(1));
    }
}
