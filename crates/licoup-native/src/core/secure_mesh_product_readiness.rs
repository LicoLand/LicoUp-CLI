use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Result, anyhow, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub const SECURE_MESH_PRODUCT_READINESS_SCHEMA: &str =
    "licomesh.secure-mesh.mls-product-readiness.v1";

const PRODUCT_MESSAGING_GATES: &[SecureMeshProductEvidenceGate] = &[
    SecureMeshProductEvidenceGate::CryptographicRuntime,
    SecureMeshProductEvidenceGate::NativeProductActions,
    SecureMeshProductEvidenceGate::PolicyCallsiteCoverage,
    SecureMeshProductEvidenceGate::AdversarialPolicyTests,
    SecureMeshProductEvidenceGate::KeyTransparencyAuthorityIntegration,
];
const SELECTED_TARGET_RELEASE_GATES: &[SecureMeshProductEvidenceGate] = &[
    SecureMeshProductEvidenceGate::CanonicalOpaqueRelay,
    SecureMeshProductEvidenceGate::SelectedTargetTopology,
];
const PRODUCTION_CLAIM_GATES: &[SecureMeshProductEvidenceGate] = &[
    SecureMeshProductEvidenceGate::ExternalKeyTransparencyAuthority,
    SecureMeshProductEvidenceGate::IndependentCryptographicAudit,
    SecureMeshProductEvidenceGate::ExternalEvidenceReducer,
];

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SecureMeshProductEvidenceGate {
    CryptographicRuntime,
    NativeProductActions,
    PolicyCallsiteCoverage,
    AdversarialPolicyTests,
    KeyTransparencyAuthorityIntegration,
    CanonicalOpaqueRelay,
    SelectedTargetTopology,
    ExternalKeyTransparencyAuthority,
    IndependentCryptographicAudit,
    ExternalEvidenceReducer,
}

impl SecureMeshProductEvidenceGate {
    fn as_str(self) -> &'static str {
        match self {
            Self::CryptographicRuntime => "cryptographic_runtime",
            Self::NativeProductActions => "native_product_actions",
            Self::PolicyCallsiteCoverage => "policy_callsite_coverage",
            Self::AdversarialPolicyTests => "adversarial_policy_tests",
            Self::KeyTransparencyAuthorityIntegration => "key_transparency_authority_integration",
            Self::CanonicalOpaqueRelay => "canonical_opaque_relay",
            Self::SelectedTargetTopology => "selected_target_topology",
            Self::ExternalKeyTransparencyAuthority => "external_key_transparency_authority",
            Self::IndependentCryptographicAudit => "independent_cryptographic_audit",
            Self::ExternalEvidenceReducer => "external_evidence_reducer",
        }
    }

    fn blocker(self) -> &'static str {
        match self {
            Self::CryptographicRuntime => "mls_cryptographic_runtime_evidence_missing",
            Self::NativeProductActions => "mls_native_product_action_evidence_missing",
            Self::PolicyCallsiteCoverage => "mls_policy_callsite_evidence_missing",
            Self::AdversarialPolicyTests => "mls_adversarial_policy_test_evidence_missing",
            Self::KeyTransparencyAuthorityIntegration => {
                "mls_key_transparency_authority_evidence_missing"
            }
            Self::CanonicalOpaqueRelay => "mls_canonical_opaque_relay_evidence_missing",
            Self::SelectedTargetTopology => "mls_selected_target_topology_evidence_missing",
            Self::ExternalKeyTransparencyAuthority => {
                "external_key_transparency_authority_evidence_missing"
            }
            Self::IndependentCryptographicAudit => {
                "independent_cryptographic_audit_evidence_missing"
            }
            Self::ExternalEvidenceReducer => "external_evidence_reducer_acceptance_missing",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SecureMeshProductEvidenceProvenance {
    SourceContract,
    NativeTest,
    SelectedTargetReceipt,
    ExternalAuthorityReceipt,
    IndependentAudit,
    ExternalReducer,
}

impl SecureMeshProductEvidenceProvenance {
    fn as_str(self) -> &'static str {
        match self {
            Self::SourceContract => "source_contract",
            Self::NativeTest => "native_test",
            Self::SelectedTargetReceipt => "selected_target_receipt",
            Self::ExternalAuthorityReceipt => "external_authority_receipt",
            Self::IndependentAudit => "independent_audit",
            Self::ExternalReducer => "external_reducer",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SecureMeshProductEvidenceRecord {
    pub gate: SecureMeshProductEvidenceGate,
    pub provenance: SecureMeshProductEvidenceProvenance,
    pub evidence_digest: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SecureMeshProductEvidenceReport {
    pub schema_version: String,
    pub source_state_digest: String,
    pub records: Vec<SecureMeshProductEvidenceRecord>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SecureMeshProductReadiness {
    accepted_gates: BTreeSet<SecureMeshProductEvidenceGate>,
    product_messaging_blockers: Vec<&'static str>,
    selected_target_release_blockers: Vec<&'static str>,
    production_claim_blockers: Vec<&'static str>,
    evidence_digest: Option<String>,
    source_state_digest: Option<String>,
}

impl SecureMeshProductReadiness {
    pub fn missing_evidence() -> Self {
        let accepted_gates = BTreeSet::new();
        let product_messaging_blockers = missing_blockers(&accepted_gates, PRODUCT_MESSAGING_GATES);
        let mut selected_target_release_blockers = product_messaging_blockers.clone();
        selected_target_release_blockers.extend(missing_blockers(
            &accepted_gates,
            SELECTED_TARGET_RELEASE_GATES,
        ));
        let mut production_claim_blockers = selected_target_release_blockers.clone();
        production_claim_blockers.extend(missing_blockers(&accepted_gates, PRODUCTION_CLAIM_GATES));
        Self {
            accepted_gates,
            product_messaging_blockers,
            selected_target_release_blockers,
            production_claim_blockers,
            evidence_digest: None,
            source_state_digest: None,
        }
    }

    pub fn product_messaging_available(&self) -> bool {
        self.product_messaging_blockers.is_empty()
    }

    pub fn selected_target_release_ready(&self) -> bool {
        self.selected_target_release_blockers.is_empty()
    }

    pub fn production_claim_ready(&self) -> bool {
        self.production_claim_blockers.is_empty()
    }

    pub fn evidence_digest(&self) -> Option<&str> {
        self.evidence_digest.as_deref()
    }

    pub fn to_status_json(&self) -> Value {
        json!({
            "schemaVersion": SECURE_MESH_PRODUCT_READINESS_SCHEMA,
            "evidenceDerived": true,
            "sourceStateDigest": self.source_state_digest,
            "evidenceDigest": self.evidence_digest,
            "acceptedGates": self
                .accepted_gates
                .iter()
                .map(|gate| gate.as_str())
                .collect::<Vec<_>>(),
            "productMessagingAvailable": self.product_messaging_available(),
            "productMessagingBlockers": self.product_messaging_blockers,
            "selectedTargetReleaseReady": self.selected_target_release_ready(),
            "selectedTargetReleaseBlockers": self.selected_target_release_blockers,
            "productionClaimReady": self.production_claim_ready(),
            "productionClaimBlockers": self.production_claim_blockers,
        })
    }
}

pub fn evaluate_secure_mesh_product_readiness(
    report: &SecureMeshProductEvidenceReport,
    expected_source_state_digest: &str,
) -> Result<SecureMeshProductReadiness> {
    ensure!(
        report.schema_version == SECURE_MESH_PRODUCT_READINESS_SCHEMA,
        "secure mesh product readiness schema is invalid"
    );
    validate_sha256_digest(&report.source_state_digest)?;
    validate_sha256_digest(expected_source_state_digest)?;
    ensure!(
        report.source_state_digest == expected_source_state_digest,
        "secure mesh product readiness evidence is stale for the current source"
    );
    ensure!(
        report.records.len() <= SecureMeshProductEvidenceGate::all().len(),
        "secure mesh product readiness evidence record count is invalid"
    );

    let mut records = BTreeMap::new();
    for record in &report.records {
        validate_sha256_digest(&record.evidence_digest)?;
        validate_gate_provenance(record.gate, record.provenance)?;
        ensure!(
            records.insert(record.gate, record).is_none(),
            "secure mesh product readiness evidence contains a duplicate gate"
        );
    }
    let accepted_gates = records.keys().copied().collect::<BTreeSet<_>>();
    let product_messaging_blockers = missing_blockers(&accepted_gates, PRODUCT_MESSAGING_GATES);
    let mut selected_target_release_blockers = product_messaging_blockers.clone();
    selected_target_release_blockers.extend(missing_blockers(
        &accepted_gates,
        SELECTED_TARGET_RELEASE_GATES,
    ));
    let mut production_claim_blockers = selected_target_release_blockers.clone();
    production_claim_blockers.extend(missing_blockers(&accepted_gates, PRODUCTION_CLAIM_GATES));

    Ok(SecureMeshProductReadiness {
        accepted_gates,
        product_messaging_blockers,
        selected_target_release_blockers,
        production_claim_blockers,
        evidence_digest: Some(aggregate_evidence_digest(report, &records)?),
        source_state_digest: Some(report.source_state_digest.clone()),
    })
}

pub fn parse_and_evaluate_secure_mesh_product_readiness(
    report_json: &str,
    expected_source_state_digest: &str,
) -> Result<SecureMeshProductReadiness> {
    ensure!(
        report_json.len() <= 256 * 1024,
        "secure mesh product readiness evidence is too large"
    );
    let report = serde_json::from_str::<SecureMeshProductEvidenceReport>(report_json)
        .map_err(|_| anyhow!("secure mesh product readiness evidence is invalid"))?;
    evaluate_secure_mesh_product_readiness(&report, expected_source_state_digest)
}

impl SecureMeshProductEvidenceGate {
    fn all() -> &'static [Self] {
        &[
            Self::CryptographicRuntime,
            Self::NativeProductActions,
            Self::PolicyCallsiteCoverage,
            Self::AdversarialPolicyTests,
            Self::KeyTransparencyAuthorityIntegration,
            Self::CanonicalOpaqueRelay,
            Self::SelectedTargetTopology,
            Self::ExternalKeyTransparencyAuthority,
            Self::IndependentCryptographicAudit,
            Self::ExternalEvidenceReducer,
        ]
    }
}

fn validate_gate_provenance(
    gate: SecureMeshProductEvidenceGate,
    provenance: SecureMeshProductEvidenceProvenance,
) -> Result<()> {
    let accepted = match gate {
        SecureMeshProductEvidenceGate::CryptographicRuntime
        | SecureMeshProductEvidenceGate::NativeProductActions
        | SecureMeshProductEvidenceGate::KeyTransparencyAuthorityIntegration
        | SecureMeshProductEvidenceGate::CanonicalOpaqueRelay => {
            matches!(provenance, SecureMeshProductEvidenceProvenance::NativeTest)
        }
        SecureMeshProductEvidenceGate::PolicyCallsiteCoverage => matches!(
            provenance,
            SecureMeshProductEvidenceProvenance::SourceContract
        ),
        SecureMeshProductEvidenceGate::AdversarialPolicyTests => {
            matches!(provenance, SecureMeshProductEvidenceProvenance::NativeTest)
        }
        SecureMeshProductEvidenceGate::SelectedTargetTopology => matches!(
            provenance,
            SecureMeshProductEvidenceProvenance::SelectedTargetReceipt
        ),
        SecureMeshProductEvidenceGate::ExternalKeyTransparencyAuthority => matches!(
            provenance,
            SecureMeshProductEvidenceProvenance::ExternalAuthorityReceipt
        ),
        SecureMeshProductEvidenceGate::IndependentCryptographicAudit => matches!(
            provenance,
            SecureMeshProductEvidenceProvenance::IndependentAudit
        ),
        SecureMeshProductEvidenceGate::ExternalEvidenceReducer => matches!(
            provenance,
            SecureMeshProductEvidenceProvenance::ExternalReducer
        ),
    };
    ensure!(
        accepted,
        "secure mesh product readiness evidence provenance is invalid for its gate"
    );
    Ok(())
}

fn missing_blockers(
    accepted: &BTreeSet<SecureMeshProductEvidenceGate>,
    required: &[SecureMeshProductEvidenceGate],
) -> Vec<&'static str> {
    required
        .iter()
        .filter(|gate| !accepted.contains(gate))
        .map(|gate| gate.blocker())
        .collect()
}

fn aggregate_evidence_digest(
    report: &SecureMeshProductEvidenceReport,
    records: &BTreeMap<SecureMeshProductEvidenceGate, &SecureMeshProductEvidenceRecord>,
) -> Result<String> {
    let mut hasher = Sha256::new();
    append_digest_field(&mut hasher, b"LICO-SECURE-MESH-MLS-PRODUCT-READINESS-v1")?;
    append_digest_field(&mut hasher, report.schema_version.as_bytes())?;
    append_digest_field(&mut hasher, report.source_state_digest.as_bytes())?;
    for (gate, record) in records {
        append_digest_field(&mut hasher, gate.as_str().as_bytes())?;
        append_digest_field(&mut hasher, record.provenance.as_str().as_bytes())?;
        append_digest_field(&mut hasher, record.evidence_digest.as_bytes())?;
    }
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

fn append_digest_field(hasher: &mut Sha256, value: &[u8]) -> Result<()> {
    let length = u32::try_from(value.len())
        .map_err(|_| anyhow!("secure mesh product readiness digest field is too large"))?;
    hasher.update(length.to_be_bytes());
    hasher.update(value);
    Ok(())
}

fn validate_sha256_digest(value: &str) -> Result<()> {
    ensure!(
        value.len() == 71
            && value.starts_with("sha256:")
            && value[7..]
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "secure mesh product readiness digest is invalid"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCE_DIGEST: &str =
        "sha256:1111111111111111111111111111111111111111111111111111111111111111";

    fn record(
        gate: SecureMeshProductEvidenceGate,
        provenance: SecureMeshProductEvidenceProvenance,
        byte: char,
    ) -> SecureMeshProductEvidenceRecord {
        SecureMeshProductEvidenceRecord {
            gate,
            provenance,
            evidence_digest: format!("sha256:{}", byte.to_string().repeat(64)),
        }
    }

    fn report(records: Vec<SecureMeshProductEvidenceRecord>) -> SecureMeshProductEvidenceReport {
        SecureMeshProductEvidenceReport {
            schema_version: SECURE_MESH_PRODUCT_READINESS_SCHEMA.to_string(),
            source_state_digest: SOURCE_DIGEST.to_string(),
            records,
        }
    }

    fn product_records() -> Vec<SecureMeshProductEvidenceRecord> {
        vec![
            record(
                SecureMeshProductEvidenceGate::CryptographicRuntime,
                SecureMeshProductEvidenceProvenance::NativeTest,
                '2',
            ),
            record(
                SecureMeshProductEvidenceGate::NativeProductActions,
                SecureMeshProductEvidenceProvenance::NativeTest,
                '3',
            ),
            record(
                SecureMeshProductEvidenceGate::PolicyCallsiteCoverage,
                SecureMeshProductEvidenceProvenance::SourceContract,
                '4',
            ),
            record(
                SecureMeshProductEvidenceGate::AdversarialPolicyTests,
                SecureMeshProductEvidenceProvenance::NativeTest,
                '5',
            ),
            record(
                SecureMeshProductEvidenceGate::KeyTransparencyAuthorityIntegration,
                SecureMeshProductEvidenceProvenance::NativeTest,
                '6',
            ),
        ]
    }

    #[test]
    fn missing_evidence_fails_closed_with_explicit_scoped_blockers() {
        let missing = SecureMeshProductReadiness::missing_evidence();
        let missing_status = missing.to_status_json();
        assert!(missing_status["sourceStateDigest"].is_null());
        assert!(missing_status["evidenceDigest"].is_null());
        assert_eq!(missing_status["productMessagingAvailable"], false);
        assert_eq!(
            missing_status["productionClaimBlockers"]
                .as_array()
                .unwrap()
                .len(),
            10
        );

        let readiness =
            evaluate_secure_mesh_product_readiness(&report(Vec::new()), SOURCE_DIGEST).unwrap();
        assert!(!readiness.product_messaging_available());
        assert!(!readiness.selected_target_release_ready());
        assert!(!readiness.production_claim_ready());
        let status = readiness.to_status_json();
        assert_eq!(status["evidenceDerived"], true);
        assert_eq!(
            status["productMessagingBlockers"].as_array().unwrap().len(),
            5
        );
        assert_eq!(
            status["selectedTargetReleaseBlockers"]
                .as_array()
                .unwrap()
                .len(),
            7
        );
        assert_eq!(
            status["productionClaimBlockers"].as_array().unwrap().len(),
            10
        );
    }

    #[test]
    fn product_availability_does_not_overclaim_selected_release_or_a10_claim() {
        let readiness =
            evaluate_secure_mesh_product_readiness(&report(product_records()), SOURCE_DIGEST)
                .unwrap();
        assert!(readiness.product_messaging_available());
        assert!(!readiness.selected_target_release_ready());
        assert!(!readiness.production_claim_ready());
        assert_eq!(
            readiness.to_status_json()["selectedTargetReleaseBlockers"],
            json!([
                "mls_canonical_opaque_relay_evidence_missing",
                "mls_selected_target_topology_evidence_missing"
            ])
        );
    }

    #[test]
    fn selected_client_release_is_independent_from_external_claim_gates() {
        let mut records = product_records();
        records.extend([
            record(
                SecureMeshProductEvidenceGate::CanonicalOpaqueRelay,
                SecureMeshProductEvidenceProvenance::NativeTest,
                '6',
            ),
            record(
                SecureMeshProductEvidenceGate::SelectedTargetTopology,
                SecureMeshProductEvidenceProvenance::SelectedTargetReceipt,
                '7',
            ),
        ]);
        let readiness =
            evaluate_secure_mesh_product_readiness(&report(records), SOURCE_DIGEST).unwrap();
        assert!(readiness.product_messaging_available());
        assert!(readiness.selected_target_release_ready());
        assert!(!readiness.production_claim_ready());
        assert_eq!(
            readiness.to_status_json()["productionClaimBlockers"],
            json!([
                "external_key_transparency_authority_evidence_missing",
                "independent_cryptographic_audit_evidence_missing",
                "external_evidence_reducer_acceptance_missing"
            ])
        );
    }

    #[test]
    fn full_claim_requires_every_external_evidence_authority() {
        let mut records = product_records();
        records.extend([
            record(
                SecureMeshProductEvidenceGate::CanonicalOpaqueRelay,
                SecureMeshProductEvidenceProvenance::NativeTest,
                '6',
            ),
            record(
                SecureMeshProductEvidenceGate::SelectedTargetTopology,
                SecureMeshProductEvidenceProvenance::SelectedTargetReceipt,
                '7',
            ),
            record(
                SecureMeshProductEvidenceGate::ExternalKeyTransparencyAuthority,
                SecureMeshProductEvidenceProvenance::ExternalAuthorityReceipt,
                '8',
            ),
            record(
                SecureMeshProductEvidenceGate::IndependentCryptographicAudit,
                SecureMeshProductEvidenceProvenance::IndependentAudit,
                '9',
            ),
            record(
                SecureMeshProductEvidenceGate::ExternalEvidenceReducer,
                SecureMeshProductEvidenceProvenance::ExternalReducer,
                'a',
            ),
        ]);
        let readiness =
            evaluate_secure_mesh_product_readiness(&report(records), SOURCE_DIGEST).unwrap();
        assert!(readiness.product_messaging_available());
        assert!(readiness.selected_target_release_ready());
        assert!(readiness.production_claim_ready());
    }

    #[test]
    fn duplicate_wrong_provenance_stale_and_unknown_fields_fail_closed() {
        let duplicate = record(
            SecureMeshProductEvidenceGate::CryptographicRuntime,
            SecureMeshProductEvidenceProvenance::NativeTest,
            '2',
        );
        assert!(
            evaluate_secure_mesh_product_readiness(
                &report(vec![duplicate.clone(), duplicate]),
                SOURCE_DIGEST,
            )
            .is_err()
        );
        assert!(
            evaluate_secure_mesh_product_readiness(
                &report(vec![record(
                    SecureMeshProductEvidenceGate::SelectedTargetTopology,
                    SecureMeshProductEvidenceProvenance::SourceContract,
                    '3',
                )]),
                SOURCE_DIGEST,
            )
            .is_err()
        );
        assert!(
            evaluate_secure_mesh_product_readiness(
                &report(Vec::new()),
                "sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
            )
            .is_err()
        );
        let unknown = json!({
            "schemaVersion": SECURE_MESH_PRODUCT_READINESS_SCHEMA,
            "sourceStateDigest": SOURCE_DIGEST,
            "records": [],
            "productionReady": true
        });
        assert!(
            parse_and_evaluate_secure_mesh_product_readiness(&unknown.to_string(), SOURCE_DIGEST,)
                .is_err()
        );
    }

    #[test]
    fn evidence_digest_is_deterministic_and_tamper_evident() {
        let records = product_records();
        let mut reversed = records.clone();
        reversed.reverse();
        let first =
            evaluate_secure_mesh_product_readiness(&report(records), SOURCE_DIGEST).unwrap();
        let second =
            evaluate_secure_mesh_product_readiness(&report(reversed), SOURCE_DIGEST).unwrap();
        assert_eq!(first.evidence_digest(), second.evidence_digest());

        let mut tampered = product_records();
        tampered[0].evidence_digest =
            "sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee".to_string();
        let third =
            evaluate_secure_mesh_product_readiness(&report(tampered), SOURCE_DIGEST).unwrap();
        assert_ne!(first.evidence_digest(), third.evidence_digest());
    }
}
