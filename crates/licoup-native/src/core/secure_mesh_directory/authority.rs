use super::*;

pub struct SecureMeshDirectoryAuthority {
    state: SecureMeshKtClientState,
}

impl SecureMeshDirectoryAuthority {
    pub fn open(
        path: impl AsRef<Path>,
        pin: PinnedKtLogKey,
        freshness_policy: KtFreshnessPolicy,
    ) -> Result<Self> {
        Ok(Self {
            state: SecureMeshKtClientState::open(path, pin, freshness_policy)?,
        })
    }

    pub fn open_in_memory(
        pin: PinnedKtLogKey,
        freshness_policy: KtFreshnessPolicy,
    ) -> Result<Self> {
        Ok(Self {
            state: SecureMeshKtClientState::open_in_memory(pin, freshness_policy)?,
        })
    }

    #[cfg(any(test, feature = "secure-mesh-acceptance-mock-kt"))]
    pub(crate) fn observe_response_gossip_for_test(
        &mut self,
        response: &UntrustedDirectoryResponse,
        now_epoch_seconds: u64,
    ) -> Result<()> {
        self.observe_gossip(
            &SecureMeshKtGossipPayload::from_sth(
                response.inclusion.signed_tree_head.clone(),
                response.consistency.clone(),
            ),
            now_epoch_seconds,
        )?;
        Ok(())
    }

    #[cfg(test)]
    pub fn authorize(
        &mut self,
        response: UntrustedDirectoryResponse,
        purpose: DirectoryAuthorizationPurpose,
        now_epoch_seconds: u64,
    ) -> Result<AuthorizedDirectoryLeaf> {
        self.observe_response_gossip_for_test(&response, now_epoch_seconds)?;
        self.authorize_request(
            response.clone(),
            DirectoryAuthorizationRequest::unbound_for_test(
                purpose,
                &response.claim.endpoint.directory_scope_commitment,
            ),
            now_epoch_seconds,
        )
    }

    pub fn authorize_request(
        &mut self,
        response: UntrustedDirectoryResponse,
        request: DirectoryAuthorizationRequest<'_>,
        now_epoch_seconds: u64,
    ) -> Result<AuthorizedDirectoryLeaf> {
        authorize_claim_state(&response.claim, request.purpose)?;
        validate_authorization_request(&response.claim, &request)?;
        let leaf_hash = response.claim.leaf_hash_hex()?;
        let stable_label = response.claim.stable_label();
        let pin = self.state.pin()?.clone();
        let (_, freshness) = self.state.authorize_hashed_directory_view(
            &stable_label,
            request.purpose.stable_code(),
            response.claim.version(),
            response.claim.revoked(),
            &leaf_hash,
            DirectoryComponentCommitments {
                identity_fingerprint: &response.claim.endpoint.fingerprint,
                identity_rotation_epoch: response.claim.endpoint.rotation_epoch,
                identity_key_digest: &response.claim.identity_key_digest(),
                pairwise_prekey_version: response.claim.key_material.pairwise_prekey_version,
                signed_prekey_digest: &response.claim.key_material.signed_prekey_bundle_digest,
                one_time_prekey_digest: &response.claim.key_material.one_time_prekey_batch_digest,
                mls_key_package_version: response.claim.key_material.mls_key_package_version,
                mls_key_package_digest: &response.claim.key_material.mls_key_package_digest,
            },
            &response.inclusion,
            &response.latest_map,
            response.consistency.as_ref(),
            now_epoch_seconds,
        )?;
        let authorization_digest = authorized_leaf_digest(
            &leaf_hash,
            request.purpose,
            &response.inclusion,
            &response.latest_map,
            response.consistency.as_ref(),
            &freshness,
            &pin,
        );
        let transcript_binding_digest =
            authorized_leaf_transcript_binding_digest(&leaf_hash, request.purpose, &pin);
        Ok(AuthorizedDirectoryLeaf {
            claim: response.claim,
            log_id: pin.log_id().to_string(),
            key_id: pin.key_id().to_string(),
            signed_tree_head: response.inclusion.signed_tree_head.clone(),
            inclusion: response.inclusion,
            latest_map: response.latest_map,
            consistency: response.consistency,
            freshness,
            purpose: request.purpose,
            provenance: pin.provenance().clone(),
            authorization_digest,
            transcript_binding_digest,
        })
    }

    pub fn authorize_absence(
        &mut self,
        response: UntrustedDirectoryAbsenceResponse,
        now_epoch_seconds: u64,
    ) -> Result<AuthorizedDirectoryAbsence> {
        let pin = self.state.pin()?.clone();
        let (_, freshness) = self.state.authorize_absence_view(
            &response.stable_label,
            &response.map_root_inclusion,
            &response.absence_map,
            response.consistency.as_ref(),
            now_epoch_seconds,
        )?;
        let authorization_digest = authorized_absence_digest(
            &response.stable_label,
            &response.absence_map,
            response.consistency.as_ref(),
            &freshness,
            &pin,
        );
        Ok(AuthorizedDirectoryAbsence {
            stable_label: response.stable_label,
            signed_tree_head: response.absence_map.signed_tree_head.clone(),
            map_root_inclusion: response.map_root_inclusion,
            absence_map: response.absence_map,
            consistency: response.consistency,
            freshness,
            provenance: pin.provenance().clone(),
            authorization_digest,
        })
    }

    pub fn observe_gossip(
        &mut self,
        gossip: &SecureMeshKtGossipPayload,
        now_epoch_seconds: u64,
    ) -> Result<SecureMeshKtCachedCheckpoint> {
        self.state
            .observe_peer_gossip_sth(gossip, now_epoch_seconds)
    }

    pub fn validate_outgoing_gossip(
        &mut self,
        gossip: &SecureMeshKtGossipPayload,
        now_epoch_seconds: u64,
    ) -> Result<SecureMeshKtCachedCheckpoint> {
        self.state
            .validate_outgoing_gossip_sth(gossip, now_epoch_seconds)
    }

    pub fn latest_checkpoint(&self) -> Result<Option<SecureMeshKtCachedCheckpoint>> {
        self.state.latest_checkpoint()
    }

    pub fn require_current_authorization(
        &mut self,
        stable_label: &str,
        purpose: DirectoryAuthorizationPurpose,
        now_epoch_seconds: u64,
    ) -> Result<SecureMeshKtAuthorizationReceipt> {
        self.state.require_current_directory_authorization(
            stable_label,
            purpose.stable_code(),
            now_epoch_seconds,
        )
    }

    pub fn security_blocked(&self) -> Result<bool> {
        self.state.equivocation_detected()
    }
}

fn authorize_claim_state(
    claim: &SecureMeshDirectoryLeafClaim,
    purpose: DirectoryAuthorizationPurpose,
) -> Result<()> {
    claim.key_material.validate()?;
    let state = claim.endpoint.directory_state.trim().to_ascii_lowercase();
    ensure!(
        matches!(state.as_str(), "active" | "revoked"),
        "secure mesh directory state must be active or revoked and cannot assert local trust"
    );
    match purpose {
        DirectoryAuthorizationPurpose::Revocation => ensure!(
            claim.revoked(),
            "secure mesh directory revocation purpose requires a revoked latest claim"
        ),
        DirectoryAuthorizationPurpose::SelfMonitor => {}
        DirectoryAuthorizationPurpose::IdentityKeyChange => ensure!(
            state == "active",
            "secure mesh directory identity-change claim must be active"
        ),
        _ => ensure!(
            !claim.revoked() && state == "active",
            "secure mesh directory operational authorization requires an active latest claim"
        ),
    }
    Ok(())
}

fn validate_authorization_request(
    claim: &SecureMeshDirectoryLeafClaim,
    request: &DirectoryAuthorizationRequest<'_>,
) -> Result<()> {
    validate_digest(
        "directory scope commitment",
        request.expected_directory_scope_commitment,
    )?;
    ensure!(
        claim.endpoint.directory_scope_commitment == request.expected_directory_scope_commitment,
        "secure mesh directory scope commitment mismatch"
    );
    if let Some(expected) = request.expected_exact_claim {
        ensure!(
            claim == expected,
            "secure mesh directory response does not match the exact locally prepared claim"
        );
    }
    if let Some(identity) = request.expected_identity {
        require_claim_device_identity(claim, identity)?;
    }
    if let Some(version) = request.expected_directory_version {
        ensure!(
            claim.directory_version == version,
            "secure mesh directory publication version mismatch"
        );
    }
    if let Some((digest, version)) = request.expected_signed_prekey {
        validate_digest("signed prekey bundle digest", digest)?;
        ensure!(
            claim.key_material.signed_prekey_bundle_digest == digest
                && claim.key_material.pairwise_prekey_version == version,
            "secure mesh directory signed prekey commitment mismatch"
        );
    }
    if let Some((digest, version)) = request.expected_one_time_prekey {
        validate_digest("one-time prekey batch digest", digest)?;
        ensure!(
            claim.key_material.one_time_prekey_batch_digest == digest
                && claim.key_material.pairwise_prekey_version == version,
            "secure mesh directory one-time prekey commitment mismatch"
        );
    }
    if let Some((digest, version)) = request.expected_mls_key_package {
        validate_digest("MLS KeyPackage digest", digest)?;
        ensure!(
            claim.key_material.mls_key_package_digest == digest
                && claim.key_material.mls_key_package_version == version,
            "secure mesh directory MLS KeyPackage commitment mismatch"
        );
    }
    Ok(())
}

pub(super) fn require_claim_device_identity(
    claim: &SecureMeshDirectoryLeafClaim,
    identity: &DeviceTrustPublicIdentity,
) -> Result<()> {
    ensure!(
        claim.endpoint.endpoint_id == identity.endpoint_id
            && claim.endpoint.identity_public_key == hex_encode(&identity.identity_public_key)
            && claim.endpoint.signing_public_key == hex_encode(&identity.signing_public_key)
            && claim.endpoint.fingerprint == identity.fingerprint()?,
        "secure mesh directory endpoint identity commitment mismatch"
    );
    Ok(())
}
