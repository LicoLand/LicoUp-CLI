use anyhow::{Result, anyhow, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::time::Duration;
use uuid::Uuid;

use crate::core::authorized_secure_record::{
    SecureRecordAuthorizationRequest, SecureRecordLocator, SecureRecordOperation,
    VersionedSecureRecord,
};
use crate::platform::client_state::ClientStateStore;

const AUTHORITY_SCHEMA: &str = "licoup.optional-collaboration-authority.v1";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct CollaborationAuthority {
    pub(super) schema_version: String,
    pub(super) capability_enabled: bool,
    pub(super) trust: Option<AuthorityTrust>,
    pub(super) installed: Option<AuthorityInstalledArtifact>,
    pub(super) assemblies: Vec<AuthorityAssembly>,
    pub(super) registrations: Vec<AuthorityRegistration>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct AuthorityTrust {
    pub(super) key_id: String,
    pub(super) public_key_base64url: String,
    pub(super) fingerprint_sha256: String,
    pub(super) source_repository_url: String,
    pub(super) runner_identity: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct AuthorityInstalledArtifact {
    pub(super) plugin_id: String,
    pub(super) version: String,
    pub(super) source_url: String,
    pub(super) source_commit_oid: String,
    pub(super) package_digest_sha256: String,
    pub(super) signed_package_inventory_digest_sha256: String,
    pub(super) runner_platform: String,
    pub(super) runner_architecture: String,
    pub(super) runner_digest_sha256: String,
    pub(super) runner_contract_version: String,
    pub(super) health_contract_version: String,
    pub(super) capabilities_contract_version: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct AuthorityAssembly {
    pub(super) deployment_id: String,
    pub(super) assembly_manifest_digest_sha256: String,
    pub(super) selected_payload_inventory_digest_sha256: String,
    pub(super) selected_component_ids: Vec<String>,
    pub(super) runner_digest_sha256: String,
    pub(super) destination_digest_sha256: String,
    pub(super) port: u16,
    pub(super) runner_destination_relative_path: String,
    pub(super) sealed_snapshot_digest_sha256: String,
    pub(super) runtime_generation: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct AuthorityRegistration {
    pub(super) registration_id: String,
    pub(super) agent_id: String,
    pub(super) package_digest_sha256: String,
    pub(super) registration_file_digest_sha256: String,
    pub(super) registration_record_digest_sha256: String,
    pub(super) selected_plugin_inventory_digest_sha256: String,
    pub(super) endpoint_scope_digest_sha256: String,
    pub(super) payload_inventory_digest_sha256: String,
    pub(super) agent_destination_digest_sha256: String,
    pub(super) registration_destination_digest_sha256: String,
}

pub(super) struct BoundAuthority {
    pub(super) authority: CollaborationAuthority,
    pub(super) secure_record: VersionedSecureRecord,
}

impl CollaborationAuthority {
    pub(super) fn new(trust: AuthorityTrust, capability_enabled: bool) -> Self {
        Self {
            schema_version: AUTHORITY_SCHEMA.to_owned(),
            capability_enabled,
            trust: Some(trust),
            installed: None,
            assemblies: Vec::new(),
            registrations: Vec::new(),
        }
    }

    pub(super) fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == AUTHORITY_SCHEMA,
            "collaboration_authority_schema_invalid"
        );
        let Some(trust) = &self.trust else {
            ensure!(
                !self.capability_enabled
                    && self.installed.is_none()
                    && self.assemblies.is_empty()
                    && self.registrations.is_empty(),
                "collaboration_authority_orphan_binding_invalid"
            );
            return Ok(());
        };
        ensure!(
                !trust.key_id.is_empty()
                && crate::domain::collaboration_plugin::runner_signature::public_key_fingerprint(
                    &trust.public_key_base64url,
                )? == trust.fingerprint_sha256
                && crate::domain::collaboration_plugin::source::normalized_github_repository_url(
                    &trust.source_repository_url,
                )? == trust.source_repository_url
                && trust.runner_identity
                    == crate::domain::collaboration_plugin::runner_signature::OFFICIAL_SERVER_RUNNER_IDENTITY,
            "collaboration_authority_trust_invalid"
        );
        if let Some(installed) = &self.installed {
            ensure!(
                is_slug(&installed.plugin_id)
                    && installed.source_url == trust.source_repository_url
                    && is_commit(&installed.source_commit_oid)
                    && is_sha256(&installed.package_digest_sha256)
                    && is_sha256(&installed.signed_package_inventory_digest_sha256)
                    && matches!(
                        installed.runner_platform.as_str(),
                        "macos" | "windows" | "ubuntu"
                    )
                    && matches!(installed.runner_architecture.as_str(), "x86_64" | "aarch64")
                    && is_sha256(&installed.runner_digest_sha256)
                    && installed.runner_contract_version == super::manifest::SERVER_RUNNER_CONTRACT
                    && installed.health_contract_version == super::manifest::SERVER_HEALTH_CONTRACT
                    && installed.capabilities_contract_version
                        == super::manifest::SERVER_CAPABILITIES_CONTRACT,
                "collaboration_authority_installed_artifact_invalid"
            );
        } else {
            ensure!(
                self.assemblies.is_empty() && self.registrations.is_empty(),
                "collaboration_authority_orphan_binding_invalid"
            );
        }
        ensure!(
            self.assemblies.len() <= 256
                && self
                    .assemblies
                    .windows(2)
                    .all(|pair| { pair[0].deployment_id < pair[1].deployment_id })
                && self.registrations.len() <= 1024
                && self
                    .registrations
                    .windows(2)
                    .all(|pair| { pair[0].registration_id < pair[1].registration_id }),
            "collaboration_authority_collection_invalid"
        );
        for assembly in &self.assemblies {
            ensure!(
                Uuid::parse_str(&assembly.deployment_id)
                    .is_ok_and(|value| value.to_string() == assembly.deployment_id)
                    && is_sha256(&assembly.assembly_manifest_digest_sha256)
                    && is_sha256(&assembly.selected_payload_inventory_digest_sha256)
                    && is_sha256(&assembly.runner_digest_sha256)
                    && is_sha256(&assembly.destination_digest_sha256)
                    && assembly.port >= 1024
                    && !assembly.runner_destination_relative_path.is_empty()
                    && is_sha256(&assembly.sealed_snapshot_digest_sha256)
                    && assembly.runtime_generation > 0
                    && !assembly.selected_component_ids.is_empty()
                    && assembly
                        .selected_component_ids
                        .windows(2)
                        .all(|pair| pair[0] < pair[1])
                    && assembly
                        .selected_component_ids
                        .iter()
                        .all(|value| is_slug(value))
                    && self.installed.as_ref().is_some_and(|installed| {
                        installed.runner_digest_sha256 == assembly.runner_digest_sha256
                    }),
                "collaboration_authority_assembly_invalid"
            );
        }
        ensure!(
            self.assemblies.iter().enumerate().all(|(index, assembly)| {
                self.assemblies[index.saturating_add(1)..]
                    .iter()
                    .all(|other| {
                        assembly.destination_digest_sha256 != other.destination_digest_sha256
                            && assembly.port != other.port
                    })
            }),
            "collaboration_authority_assembly_conflict"
        );
        for registration in &self.registrations {
            ensure!(
                Uuid::parse_str(&registration.registration_id)
                    .is_ok_and(|value| value.to_string() == registration.registration_id)
                    && super::registration::canonical_agent_id(&registration.agent_id).as_deref()
                        == Some(registration.agent_id.as_str())
                    && is_sha256(&registration.package_digest_sha256)
                    && is_sha256(&registration.registration_file_digest_sha256)
                    && is_sha256(&registration.registration_record_digest_sha256)
                    && is_sha256(&registration.selected_plugin_inventory_digest_sha256)
                    && is_sha256(&registration.endpoint_scope_digest_sha256)
                    && is_sha256(&registration.payload_inventory_digest_sha256)
                    && is_sha256(&registration.agent_destination_digest_sha256)
                    && is_sha256(&registration.registration_destination_digest_sha256)
                    && self.installed.as_ref().is_some_and(|installed| {
                        installed.package_digest_sha256 == registration.package_digest_sha256
                    }),
                "collaboration_authority_registration_invalid"
            );
        }
        ensure!(
            self.registrations
                .iter()
                .enumerate()
                .all(|(index, registration)| {
                    self.registrations[index.saturating_add(1)..]
                        .iter()
                        .all(|other| {
                            registration.agent_destination_digest_sha256
                                != other.agent_destination_digest_sha256
                                && registration.registration_destination_digest_sha256
                                    != other.registration_destination_digest_sha256
                        })
                }),
            "collaboration_authority_registration_conflict"
        );
        Ok(())
    }

    fn payload(&self) -> Result<String> {
        self.validate()?;
        Ok(serde_json::to_string(self)?)
    }

    pub(super) fn add_assembly(
        &mut self,
        record: &super::assembly::LocalAssemblyRecord,
    ) -> Result<()> {
        let assembly = AuthorityAssembly::from_record(record);
        ensure!(
            self.assemblies.iter().all(|existing| {
                existing.deployment_id != assembly.deployment_id
                    && existing.destination_digest_sha256 != assembly.destination_digest_sha256
                    && existing.port != assembly.port
            }),
            "collaboration_authority_assembly_conflict"
        );
        self.assemblies.push(assembly);
        self.assemblies
            .sort_by(|left, right| left.deployment_id.cmp(&right.deployment_id));
        self.validate()
    }

    pub(super) fn remove_assembly(
        &mut self,
        record: &super::assembly::LocalAssemblyRecord,
    ) -> Result<()> {
        self.ensure_assembly(record)?;
        self.assemblies
            .retain(|assembly| assembly.deployment_id != record.deployment_id);
        self.validate()
    }

    pub(super) fn ensure_assembly(
        &self,
        record: &super::assembly::LocalAssemblyRecord,
    ) -> Result<()> {
        ensure!(
            self.assemblies
                .iter()
                .any(|assembly| assembly == &AuthorityAssembly::from_record(record)),
            "collaboration_authority_assembly_binding_mismatch"
        );
        Ok(())
    }

    pub(super) fn contains_assembly(&self, deployment_id: &str) -> bool {
        self.assemblies
            .iter()
            .any(|assembly| assembly.deployment_id == deployment_id)
    }

    pub(super) fn add_registrations(
        &mut self,
        registrations: &[AuthorityRegistration],
    ) -> Result<()> {
        ensure!(
            !registrations.is_empty()
                && registrations
                    .windows(2)
                    .all(|pair| { pair[0].registration_id < pair[1].registration_id }),
            "collaboration_authority_registration_collection_invalid"
        );
        for registration in registrations {
            ensure!(
                self.registrations.iter().all(|existing| {
                    existing.registration_id != registration.registration_id
                        && existing.agent_destination_digest_sha256
                            != registration.agent_destination_digest_sha256
                        && existing.registration_destination_digest_sha256
                            != registration.registration_destination_digest_sha256
                }),
                "collaboration_authority_registration_conflict"
            );
            self.registrations.push(registration.clone());
        }
        self.registrations
            .sort_by(|left, right| left.registration_id.cmp(&right.registration_id));
        self.validate()
    }

    pub(super) fn ensure_registrations(
        &self,
        registrations: &[AuthorityRegistration],
    ) -> Result<()> {
        ensure!(
            !registrations.is_empty()
                && registrations.iter().all(|registration| {
                    self.registrations
                        .iter()
                        .any(|existing| existing == registration)
                }),
            "collaboration_authority_registration_binding_mismatch"
        );
        Ok(())
    }

    pub(super) fn contains_registration(&self, registration_id: &str) -> bool {
        self.registrations
            .iter()
            .any(|registration| registration.registration_id == registration_id)
    }
}

impl AuthorityAssembly {
    fn from_record(record: &super::assembly::LocalAssemblyRecord) -> Self {
        Self {
            deployment_id: record.deployment_id.clone(),
            assembly_manifest_digest_sha256: record.manifest_digest_sha256.clone(),
            selected_payload_inventory_digest_sha256: record
                .selected_payload_inventory_digest_sha256
                .clone(),
            selected_component_ids: record.selected_component_ids.clone(),
            runner_digest_sha256: record.runner_digest_sha256.clone(),
            destination_digest_sha256: record.destination_digest_sha256.clone(),
            port: record.port,
            runner_destination_relative_path: record.runner_destination_relative_path.clone(),
            sealed_snapshot_digest_sha256: record.sealed_snapshot_digest_sha256.clone(),
            runtime_generation: record.runtime_generation,
        }
    }
}

pub(super) fn create(
    store: &ClientStateStore,
    authority: CollaborationAuthority,
    reason: &str,
) -> Result<BoundAuthority> {
    let secure_record = VersionedSecureRecord::new(1, None, authority.payload()?)?;
    let provider = crate::platform::authorized_secure_record::store();
    ensure!(
        provider.user_presence_available(),
        "collaboration_authority_user_presence_unavailable"
    );
    let locator = locator(store)?;
    let grant = provider.authorize(request(
        locator.clone(),
        SecureRecordOperation::Create,
        &secure_record,
        None,
        reason,
    )?)?;
    provider.compare_and_swap(&grant, &locator, None, &secure_record)?;
    Ok(BoundAuthority {
        authority,
        secure_record,
    })
}

pub(super) fn read(
    store: &ClientStateStore,
    expected_version: u64,
    expected_digest_sha256: &str,
    reason: &str,
) -> Result<BoundAuthority> {
    let provider = crate::platform::authorized_secure_record::store();
    ensure!(
        provider.user_presence_available(),
        "collaboration_authority_user_presence_unavailable"
    );
    let locator = locator(store)?;
    let request = SecureRecordAuthorizationRequest::new(
        locator.clone(),
        SecureRecordOperation::Read,
        expected_digest_sha256.to_owned(),
        expected_version,
        Some(expected_digest_sha256.to_owned()),
        Uuid::new_v4().to_string(),
        reason.to_owned(),
        Duration::from_secs(45),
        1,
        authority_scope_bindings(),
    )?;
    let grant = provider.authorize(request)?;
    let secure_record =
        provider.read(&grant, &locator, expected_version, expected_digest_sha256)?;
    decode_bound(secure_record)
}

pub(super) fn recover_current(store: &ClientStateStore, reason: &str) -> Result<BoundAuthority> {
    let provider = crate::platform::authorized_secure_record::store();
    ensure!(
        provider.user_presence_available(),
        "collaboration_authority_user_presence_unavailable"
    );
    let locator = locator(store)?;
    let mut hasher = Sha256::new();
    hasher.update(b"LICOUP-OPTIONAL-COLLABORATION-AUTHORITY-RECOVERY-V1\0");
    hasher.update(locator.namespace().as_bytes());
    hasher.update([0]);
    hasher.update(locator.key().as_bytes());
    let recovery_scope_digest_sha256 = format!("{:x}", hasher.finalize());
    let request = SecureRecordAuthorizationRequest::new(
        locator.clone(),
        SecureRecordOperation::RecoverRead,
        recovery_scope_digest_sha256.clone(),
        0,
        None,
        Uuid::new_v4().to_string(),
        reason.to_owned(),
        Duration::from_secs(45),
        1,
        authority_scope_bindings(),
    )?;
    let grant = provider.authorize(request)?;
    decode_bound(provider.read_current(&grant, &locator, &recovery_scope_digest_sha256)?)
}

pub(super) fn replace(
    store: &ClientStateStore,
    expected: &BoundAuthority,
    replacement: CollaborationAuthority,
    reason: &str,
) -> Result<BoundAuthority> {
    expected.authority.validate()?;
    let secure_record = VersionedSecureRecord::new(
        expected.secure_record.version().saturating_add(1),
        Some(expected.secure_record.record_digest_sha256().to_owned()),
        replacement.payload()?,
    )?;
    let provider = crate::platform::authorized_secure_record::store();
    ensure!(
        provider.user_presence_available(),
        "collaboration_authority_user_presence_unavailable"
    );
    let locator = locator(store)?;
    let grant = provider.authorize(request(
        locator.clone(),
        SecureRecordOperation::Replace,
        &secure_record,
        Some(&expected.secure_record),
        reason,
    )?)?;
    provider.compare_and_swap(
        &grant,
        &locator,
        Some(&expected.secure_record),
        &secure_record,
    )?;
    Ok(BoundAuthority {
        authority: replacement,
        secure_record,
    })
}

fn decode_bound(secure_record: VersionedSecureRecord) -> Result<BoundAuthority> {
    secure_record.validate()?;
    let authority: CollaborationAuthority = serde_json::from_str(secure_record.payload())
        .map_err(|_| anyhow!("collaboration_authority_record_invalid"))?;
    authority.validate()?;
    Ok(BoundAuthority {
        authority,
        secure_record,
    })
}

pub(super) fn decode_projected(secure_record: VersionedSecureRecord) -> Result<BoundAuthority> {
    decode_bound(secure_record)
}

pub(super) fn ensure_projection_matches(
    authority: &CollaborationAuthority,
    state: &super::lifecycle::CapabilityState,
) -> Result<()> {
    let projected_trust = state.runner_trust.as_ref().map(AuthorityTrust::from);
    ensure!(
        authority.capability_enabled == state.capability_enabled
            && authority.trust.as_ref() == projected_trust.as_ref(),
        "collaboration_authority_trust_projection_mismatch"
    );
    let projected_installed = state
        .installed
        .as_ref()
        .map(AuthorityInstalledArtifact::from);
    ensure!(
        authority.installed.as_ref() == projected_installed.as_ref(),
        "collaboration_authority_install_projection_mismatch"
    );
    Ok(())
}

pub(super) fn projected(state: &super::lifecycle::CapabilityState) -> Result<BoundAuthority> {
    let record = state
        .authority_record
        .clone()
        .ok_or_else(|| anyhow!("collaboration_authority_projection_missing"))?;
    let bound = decode_bound(record)?;
    ensure_projection_matches(&bound.authority, state)?;
    Ok(bound)
}

pub(super) fn apply_projection(
    state: &mut super::lifecycle::CapabilityState,
    bound: &BoundAuthority,
) -> Result<()> {
    bound.authority.validate()?;
    ensure_projection_matches(&bound.authority, state)?;
    state.authority_record = Some(bound.secure_record.clone());
    Ok(())
}

impl From<&super::lifecycle::RunnerTrustRecord> for AuthorityTrust {
    fn from(value: &super::lifecycle::RunnerTrustRecord) -> Self {
        Self {
            key_id: value.key_id.clone(),
            public_key_base64url: value.public_key_base64url.clone(),
            fingerprint_sha256: value.fingerprint_sha256.clone(),
            source_repository_url: value.source_repository_url.clone(),
            runner_identity: value.runner_identity.clone(),
        }
    }
}

impl From<&super::lifecycle::InstalledPlugin> for AuthorityInstalledArtifact {
    fn from(value: &super::lifecycle::InstalledPlugin) -> Self {
        Self {
            plugin_id: value.plugin_id.clone(),
            version: value.version.clone(),
            source_url: value.source_url.clone(),
            source_commit_oid: value.source_commit_oid.clone(),
            package_digest_sha256: value.digest_sha256.clone(),
            signed_package_inventory_digest_sha256: value
                .signed_package_inventory_digest_sha256
                .clone(),
            runner_platform: value.runner_platform.clone(),
            runner_architecture: value.runner_architecture.clone(),
            runner_digest_sha256: value.runner_digest_sha256.clone(),
            runner_contract_version: value.runner_contract_version.clone(),
            health_contract_version: value.health_contract_version.clone(),
            capabilities_contract_version: value.capabilities_contract_version.clone(),
        }
    }
}

fn request(
    locator: SecureRecordLocator,
    operation: SecureRecordOperation,
    target: &VersionedSecureRecord,
    expected: Option<&VersionedSecureRecord>,
    reason: &str,
) -> Result<SecureRecordAuthorizationRequest> {
    SecureRecordAuthorizationRequest::new(
        locator,
        operation,
        target.record_digest_sha256().to_owned(),
        expected.map_or(0, VersionedSecureRecord::version),
        expected
            .map(VersionedSecureRecord::record_digest_sha256)
            .map(str::to_owned),
        Uuid::new_v4().to_string(),
        reason.to_owned(),
        Duration::from_secs(45),
        1,
        authority_scope_bindings(),
    )
}

fn authority_scope_bindings() -> BTreeMap<String, String> {
    BTreeMap::from([
        ("consumer".to_owned(), "optional-collaboration".to_owned()),
        ("authority".to_owned(), AUTHORITY_SCHEMA.to_owned()),
    ])
}

fn locator(store: &ClientStateStore) -> Result<SecureRecordLocator> {
    #[cfg(not(test))]
    {
        let _ = store;
        return SecureRecordLocator::new("optional-collaboration", "profile-default");
    }
    #[cfg(test)]
    {
        let root = store
            .root()
            .to_str()
            .ok_or_else(|| anyhow!("collaboration_authority_profile_invalid"))?;
        let profile = format!("{:x}", Sha256::digest(root.as_bytes()));
        SecureRecordLocator::new("optional-collaboration", format!("profile-{profile}"))
    }
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn is_commit(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn is_slug(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 192
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}
