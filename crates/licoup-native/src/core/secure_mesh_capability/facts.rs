use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

use super::catalog::capability_catalog;
use super::taxonomy::SecurityCapability;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityFactState {
    Supported,
    Unsupported,
    TemporarilyUnavailable,
    Unverified,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityEvidenceKind {
    SourceContract,
    RuntimeOperation,
    GeneratedKeyInspection,
    OsAuthorization,
    TestFixture,
    NotMeasured,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CapabilityFact {
    pub capability: SecurityCapability,
    pub state: CapabilityFactState,
    pub evidence_kind: CapabilityEvidenceKind,
    pub measured_at_unix_seconds: Option<i64>,
    pub reason_code: Option<String>,
}

impl CapabilityFact {
    pub fn supported(
        capability: SecurityCapability,
        evidence_kind: CapabilityEvidenceKind,
    ) -> Self {
        Self {
            capability,
            state: CapabilityFactState::Supported,
            evidence_kind,
            measured_at_unix_seconds: None,
            reason_code: None,
        }
    }

    pub fn unavailable(
        capability: SecurityCapability,
        state: CapabilityFactState,
        evidence_kind: CapabilityEvidenceKind,
        reason_code: impl Into<String>,
    ) -> Result<Self> {
        ensure!(
            matches!(
                state,
                CapabilityFactState::Unsupported
                    | CapabilityFactState::TemporarilyUnavailable
                    | CapabilityFactState::Unverified
            ),
            "unavailable capability fact cannot use the supported state"
        );
        let reason_code = reason_code.into();
        validate_reason_code(Some(&reason_code))?;
        Ok(Self {
            capability,
            state,
            evidence_kind,
            measured_at_unix_seconds: None,
            reason_code: Some(reason_code),
        })
    }
}

pub(super) fn validate_reason_code(reason_code: Option<&str>) -> Result<()> {
    let Some(reason_code) = reason_code else {
        return Ok(());
    };
    ensure!(
        !reason_code.is_empty()
            && reason_code.len() <= 96
            && reason_code.bytes().all(|byte| byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || b"._-".contains(&byte)),
        "secure mesh capability reason code is invalid"
    );
    Ok(())
}

pub fn mandatory_protocol_facts(
    evidence_kind: CapabilityEvidenceKind,
) -> Result<Vec<CapabilityFact>> {
    let catalog = capability_catalog()?;
    Ok(catalog
        .definitions()
        .filter(|definition| definition.mandatory && !definition.derived)
        .map(|definition| CapabilityFact::supported(definition.capability, evidence_kind))
        .collect())
}
