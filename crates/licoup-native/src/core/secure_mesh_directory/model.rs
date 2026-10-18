use super::*;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PinnedKtLogConfiguration {
    pub log_id: String,
    pub key_id: String,
    pub public_key_hex: String,
    pub provenance: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SecureMeshKtVerifierConfiguration {
    pub pin: PinnedKtLogConfiguration,
    pub max_sth_age_seconds: u64,
    pub max_future_skew_seconds: u64,
}

impl SecureMeshKtVerifierConfiguration {
    pub fn validate(&self) -> Result<()> {
        self.pin.clone().into_pin()?;
        KtFreshnessPolicy::strict(self.max_sth_age_seconds, self.max_future_skew_seconds)?;
        Ok(())
    }
}

impl PinnedKtLogConfiguration {
    pub fn into_pin(self) -> Result<PinnedKtLogKey> {
        let public_key = parse_digest(&self.public_key_hex)?;
        match self.provenance.as_str() {
            "user-configured-external" => PinnedKtLogKey::from_user_configured_ed25519_bytes(
                self.log_id,
                self.key_id,
                public_key,
            ),
            #[cfg(any(test, feature = "secure-mesh-acceptance-mock-kt"))]
            "local-acceptance-mock" => PinnedKtLogKey::from_acceptance_mock_ed25519_bytes(
                self.log_id,
                self.key_id,
                public_key,
            ),
            _ => Err(anyhow!(
                "secure mesh KT pin provenance is unsupported or not available in this build"
            )),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SecureMeshDirectoryKeyMaterialCommitment {
    /// Digest of the signed Pairwise prekey record and its endpoint signature.
    pub signed_prekey_bundle_digest: String,
    /// Digest of the published one-time-prekey batch, including identifiers and public keys.
    pub one_time_prekey_batch_digest: String,
    /// Monotonic version of the Pairwise prekey publication.
    pub pairwise_prekey_version: u64,
    /// Digest of the exact MLS KeyPackage bytes and credential binding.
    pub mls_key_package_digest: String,
    /// Monotonic version of the MLS KeyPackage publication.
    pub mls_key_package_version: u64,
}

impl SecureMeshDirectoryKeyMaterialCommitment {
    pub(super) fn validate(&self) -> Result<()> {
        validate_digest(
            "signed prekey bundle digest",
            &self.signed_prekey_bundle_digest,
        )?;
        validate_digest(
            "one-time prekey batch digest",
            &self.one_time_prekey_batch_digest,
        )?;
        validate_digest("MLS KeyPackage digest", &self.mls_key_package_digest)?;
        ensure!(
            self.pairwise_prekey_version <= KT_JSON_SAFE_INTEGER_MAX
                && self.mls_key_package_version <= KT_JSON_SAFE_INTEGER_MAX,
            "secure mesh directory component version exceeds the cross-language safe range"
        );
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SecureMeshDirectoryLeafClaim {
    pub endpoint: SecureMeshTransparencyLeafBody,
    pub key_material: SecureMeshDirectoryKeyMaterialCommitment,
    /// Monotonic version of the combined identity/prekey/KeyPackage directory publication.
    pub directory_version: u64,
}

impl SecureMeshDirectoryLeafClaim {
    pub fn stable_label(&self) -> String {
        self.endpoint.directory_key()
    }

    pub fn version(&self) -> u64 {
        self.directory_version
    }

    pub fn revoked(&self) -> bool {
        self.endpoint.is_revoked()
    }

    pub fn leaf_hash(&self) -> Result<[u8; 32]> {
        self.key_material.validate()?;
        ensure!(
            self.directory_version <= KT_JSON_SAFE_INTEGER_MAX,
            "secure mesh directory version exceeds the cross-language safe range"
        );
        let endpoint_hash = self.endpoint.leaf_hash()?;
        let mut claim = Vec::new();
        claim.extend_from_slice(DIRECTORY_CLAIM_DOMAIN);
        append_len_prefixed(&mut claim, SECURE_MESH_DIRECTORY_CLAIM_VERSION.as_bytes());
        append_len_prefixed(&mut claim, self.stable_label().as_bytes());
        claim.extend_from_slice(&endpoint_hash);
        claim.extend_from_slice(&self.directory_version.to_be_bytes());
        claim.extend_from_slice(&parse_digest(
            &self.key_material.signed_prekey_bundle_digest,
        )?);
        claim.extend_from_slice(&parse_digest(
            &self.key_material.one_time_prekey_batch_digest,
        )?);
        claim.extend_from_slice(&self.key_material.pairwise_prekey_version.to_be_bytes());
        claim.extend_from_slice(&parse_digest(&self.key_material.mls_key_package_digest)?);
        claim.extend_from_slice(&self.key_material.mls_key_package_version.to_be_bytes());
        claim.push(u8::from(self.revoked()));
        Ok(kt_log_leaf_hash(&claim))
    }

    pub fn leaf_hash_hex(&self) -> Result<String> {
        Ok(hex_encode(&self.leaf_hash()?))
    }

    pub fn identity_key_digest(&self) -> String {
        let mut transcript = Vec::new();
        transcript.extend_from_slice(DIRECTORY_IDENTITY_COMMITMENT_DOMAIN);
        append_len_prefixed(&mut transcript, self.endpoint.endpoint_id.as_bytes());
        append_len_prefixed(
            &mut transcript,
            self.endpoint.identity_public_key.as_bytes(),
        );
        append_len_prefixed(&mut transcript, self.endpoint.signing_public_key.as_bytes());
        append_len_prefixed(&mut transcript, self.endpoint.fingerprint.as_bytes());
        hex_encode(&Sha256::digest(transcript))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DirectoryAuthorizationPurpose {
    Pairing,
    PairwiseSignedPrekey,
    PairwiseOneTimePrekey,
    PairwiseSessionBootstrap,
    IdentityKeyChange,
    Revocation,
    SelfMonitor,
    MlsKeyPackage,
    MlsMemberAdd,
}

#[derive(Clone, Copy)]
pub struct DirectoryAuthorizationRequest<'a> {
    pub(super) purpose: DirectoryAuthorizationPurpose,
    pub(super) expected_directory_scope_commitment: &'a str,
    pub(super) expected_identity: Option<&'a DeviceTrustPublicIdentity>,
    pub(super) expected_directory_version: Option<u64>,
    pub(super) expected_signed_prekey: Option<(&'a str, u64)>,
    pub(super) expected_one_time_prekey: Option<(&'a str, u64)>,
    pub(super) expected_mls_key_package: Option<(&'a str, u64)>,
    pub(super) expected_exact_claim: Option<&'a SecureMeshDirectoryLeafClaim>,
}

impl<'a> DirectoryAuthorizationRequest<'a> {
    pub fn for_pairwise(
        purpose: DirectoryAuthorizationPurpose,
        directory_scope_commitment: &'a str,
        identity: &'a DeviceTrustPublicIdentity,
        signed_prekey_digest: &'a str,
        one_time_prekey_digest: &'a str,
        prekey_version: u64,
    ) -> Self {
        Self {
            purpose,
            expected_directory_scope_commitment: directory_scope_commitment,
            expected_identity: Some(identity),
            expected_directory_version: None,
            expected_signed_prekey: Some((signed_prekey_digest, prekey_version)),
            expected_one_time_prekey: Some((one_time_prekey_digest, prekey_version)),
            expected_mls_key_package: None,
            expected_exact_claim: None,
        }
    }

    pub fn for_mls(
        purpose: DirectoryAuthorizationPurpose,
        directory_scope_commitment: &'a str,
        identity: &'a DeviceTrustPublicIdentity,
        directory_version: u64,
        key_package_digest: &'a str,
        key_package_version: u64,
    ) -> Self {
        Self {
            purpose,
            expected_directory_scope_commitment: directory_scope_commitment,
            expected_identity: Some(identity),
            expected_directory_version: Some(directory_version),
            expected_signed_prekey: None,
            expected_one_time_prekey: None,
            expected_mls_key_package: Some((key_package_digest, key_package_version)),
            expected_exact_claim: None,
        }
    }

    pub fn for_full_subject(
        purpose: DirectoryAuthorizationPurpose,
        directory_scope_commitment: &'a str,
        identity: &'a DeviceTrustPublicIdentity,
        directory_version: u64,
        signed_prekey_digest: &'a str,
        one_time_prekey_digest: &'a str,
        pairwise_prekey_version: u64,
        mls_key_package_digest: &'a str,
        mls_key_package_version: u64,
    ) -> Self {
        Self {
            purpose,
            expected_directory_scope_commitment: directory_scope_commitment,
            expected_identity: Some(identity),
            expected_directory_version: Some(directory_version),
            expected_signed_prekey: Some((signed_prekey_digest, pairwise_prekey_version)),
            expected_one_time_prekey: Some((one_time_prekey_digest, pairwise_prekey_version)),
            expected_mls_key_package: Some((mls_key_package_digest, mls_key_package_version)),
            expected_exact_claim: None,
        }
    }

    pub fn for_exact_claim(
        purpose: DirectoryAuthorizationPurpose,
        directory_scope_commitment: &'a str,
        claim: &'a SecureMeshDirectoryLeafClaim,
    ) -> Self {
        Self {
            purpose,
            expected_directory_scope_commitment: directory_scope_commitment,
            expected_identity: None,
            expected_directory_version: None,
            expected_signed_prekey: None,
            expected_one_time_prekey: None,
            expected_mls_key_package: None,
            expected_exact_claim: Some(claim),
        }
    }

    #[cfg(test)]
    pub(super) fn unbound_for_test(
        purpose: DirectoryAuthorizationPurpose,
        directory_scope_commitment: &'a str,
    ) -> Self {
        Self {
            purpose,
            expected_directory_scope_commitment: directory_scope_commitment,
            expected_identity: None,
            expected_directory_version: None,
            expected_signed_prekey: None,
            expected_one_time_prekey: None,
            expected_mls_key_package: None,
            expected_exact_claim: None,
        }
    }
}

impl DirectoryAuthorizationPurpose {
    pub fn stable_code(self) -> &'static str {
        match self {
            Self::Pairing => "pairing",
            Self::PairwiseSignedPrekey => "pairwise-signed-prekey",
            Self::PairwiseOneTimePrekey => "pairwise-one-time-prekey",
            Self::PairwiseSessionBootstrap => "pairwise-session-bootstrap",
            Self::IdentityKeyChange => "identity-key-change",
            Self::Revocation => "revocation",
            Self::SelfMonitor => "self-monitor",
            Self::MlsKeyPackage => "mls-key-package",
            Self::MlsMemberAdd => "mls-member-add",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct UntrustedDirectoryResponse {
    pub claim: SecureMeshDirectoryLeafClaim,
    pub inclusion: SecureMeshKtInclusionProof,
    pub latest_map: SecureMeshKtMapProof,
    pub consistency: Option<SecureMeshKtConsistencyProof>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthorizedDirectoryLeaf {
    pub(super) claim: SecureMeshDirectoryLeafClaim,
    pub(super) log_id: String,
    pub(super) key_id: String,
    pub(super) signed_tree_head: SecureMeshSignedTreeHead,
    pub(super) inclusion: SecureMeshKtInclusionProof,
    pub(super) latest_map: SecureMeshKtMapProof,
    pub(super) consistency: Option<SecureMeshKtConsistencyProof>,
    pub(super) freshness: VerifiedKtFreshness,
    pub(super) purpose: DirectoryAuthorizationPurpose,
    pub(super) provenance: KtAuthorityProvenance,
    pub(super) authorization_digest: String,
    pub(super) transcript_binding_digest: String,
}

impl AuthorizedDirectoryLeaf {
    pub fn claim(&self) -> &SecureMeshDirectoryLeafClaim {
        &self.claim
    }

    pub fn log_id(&self) -> &str {
        &self.log_id
    }

    pub fn key_id(&self) -> &str {
        &self.key_id
    }

    pub fn signed_tree_head(&self) -> &SecureMeshSignedTreeHead {
        &self.signed_tree_head
    }

    pub fn inclusion(&self) -> &SecureMeshKtInclusionProof {
        &self.inclusion
    }

    pub fn latest_map(&self) -> &SecureMeshKtMapProof {
        &self.latest_map
    }

    pub fn consistency(&self) -> Option<&SecureMeshKtConsistencyProof> {
        self.consistency.as_ref()
    }

    pub fn freshness(&self) -> &VerifiedKtFreshness {
        &self.freshness
    }

    pub fn purpose(&self) -> DirectoryAuthorizationPurpose {
        self.purpose
    }

    pub fn provenance(&self) -> &KtAuthorityProvenance {
        &self.provenance
    }

    pub fn authorization_digest(&self) -> &str {
        &self.authorization_digest
    }

    /// Stable digest for binding the authorized directory subject into a peer transcript.
    ///
    /// Unlike `authorization_digest`, this intentionally excludes the verifier's current STH,
    /// consistency path, and observation time. Two clients can therefore bind the same latest
    /// claim while independently verifying fresh, append-consistent views of the pinned log.
    pub fn transcript_binding_digest(&self) -> &str {
        &self.transcript_binding_digest
    }

    pub fn require_purpose(&self, expected: DirectoryAuthorizationPurpose) -> Result<()> {
        ensure!(
            self.purpose == expected,
            "secure mesh directory authorization purpose mismatch"
        );
        Ok(())
    }

    pub fn require_device_identity(&self, identity: &DeviceTrustPublicIdentity) -> Result<()> {
        require_claim_device_identity(&self.claim, identity)
    }

    pub fn require_signed_prekey_digest(&self, digest: &str, version: u64) -> Result<()> {
        validate_digest("signed prekey bundle digest", digest)?;
        ensure!(
            self.claim.key_material.signed_prekey_bundle_digest == digest
                && self.claim.key_material.pairwise_prekey_version == version,
            "secure mesh directory signed prekey commitment mismatch"
        );
        Ok(())
    }

    pub fn require_one_time_prekey_batch_digest(&self, digest: &str, version: u64) -> Result<()> {
        validate_digest("one-time prekey batch digest", digest)?;
        ensure!(
            self.claim.key_material.one_time_prekey_batch_digest == digest
                && self.claim.key_material.pairwise_prekey_version == version,
            "secure mesh directory one-time prekey commitment mismatch"
        );
        Ok(())
    }

    pub fn require_mls_key_package_digest(&self, digest: &str, version: u64) -> Result<()> {
        validate_digest("MLS KeyPackage digest", digest)?;
        ensure!(
            self.claim.key_material.mls_key_package_digest == digest
                && self.claim.key_material.mls_key_package_version == version,
            "secure mesh directory MLS KeyPackage commitment mismatch"
        );
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct UntrustedDirectoryAbsenceResponse {
    pub stable_label: String,
    pub map_root_inclusion: SecureMeshKtInclusionProof,
    pub absence_map: SecureMeshKtMapProof,
    pub consistency: Option<SecureMeshKtConsistencyProof>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthorizedDirectoryAbsence {
    pub(super) stable_label: String,
    pub(super) signed_tree_head: SecureMeshSignedTreeHead,
    pub(super) map_root_inclusion: SecureMeshKtInclusionProof,
    pub(super) absence_map: SecureMeshKtMapProof,
    pub(super) consistency: Option<SecureMeshKtConsistencyProof>,
    pub(super) freshness: VerifiedKtFreshness,
    pub(super) provenance: KtAuthorityProvenance,
    pub(super) authorization_digest: String,
}

impl AuthorizedDirectoryAbsence {
    pub fn stable_label(&self) -> &str {
        &self.stable_label
    }

    pub fn signed_tree_head(&self) -> &SecureMeshSignedTreeHead {
        &self.signed_tree_head
    }

    pub fn absence_map(&self) -> &SecureMeshKtMapProof {
        &self.absence_map
    }

    pub fn map_root_inclusion(&self) -> &SecureMeshKtInclusionProof {
        &self.map_root_inclusion
    }

    pub fn consistency(&self) -> Option<&SecureMeshKtConsistencyProof> {
        self.consistency.as_ref()
    }

    pub fn freshness(&self) -> &VerifiedKtFreshness {
        &self.freshness
    }

    pub fn provenance(&self) -> &KtAuthorityProvenance {
        &self.provenance
    }

    pub fn authorization_digest(&self) -> &str {
        &self.authorization_digest
    }
}
