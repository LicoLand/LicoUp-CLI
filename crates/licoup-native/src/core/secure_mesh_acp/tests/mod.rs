use super::{
    SECURE_MESH_ACP_STATUS, SecureMeshAcpEnvelopeBinding, SecureMeshAcpPayloadClass,
    acp_envelope_aad_digest, classify_acp_payload_class, encode_acp_envelope_aad,
    open_acp_protected_payload, reject_plaintext_acp_protected_payload_relay,
    seal_acp_protected_payload,
};
use crate::core::secure_mesh_crypto::{
    ContentKey, SecureMeshContentContext, SecureMeshPayloadKind, SecureMeshPlaintext,
    open_payload_with_aad_binding, seal_payload_with_aad_binding,
};
use crate::core::secure_mesh_pairwise::{SecureMeshPairwisePrivateKey, SecureMeshPairwiseSession};
use crate::core::secure_mesh_pqxdh::SecureMeshMlKem1024PreKeySeed;
use crate::core::secure_mesh_prekey::{
    SecureMeshPairwisePreKeyBundle, SecureMeshPreKeyKind, SecureMeshPreKeyValidationPolicy,
    authorize_test_pairwise_prekey_bundle, sign_prekey_record,
};
use crate::core::secure_mesh_trust::{DeviceTrustPublicIdentity, DeviceTrustState};
use base64::{Engine, engine::general_purpose};
use ed25519_dalek::SigningKey;
use licoup_protocol_bindings::licoarc_relay::LicoArcRelayEnvelope;
use rand_core::OsRng;
use sha2::{Digest, Sha256};
use time::{Duration, OffsetDateTime, format_description::well_known::Rfc3339};

fn binding_fixture(class: SecureMeshAcpPayloadClass) -> SecureMeshAcpEnvelopeBinding {
    let mut binding = SecureMeshAcpEnvelopeBinding::new(
        class,
        "agent-source",
        "agent-target",
        "acp-session-1",
        "relay-session-1",
        "relay-turn-1",
        7,
    )
    .unwrap();
    binding.operation_id = Some("op-1".to_string());
    binding.tool_call_id = Some("tool-1".to_string());
    binding.permission_request_id = Some("perm-1".to_string());
    binding.idempotency_key = Some("idem-1".to_string());
    binding.retry_counter = Some(0);
    binding.expires_at = Some("2026-01-01T00:10:00.000Z".to_string());
    binding.policy_revision = Some("policy-rev-1".to_string());
    binding.grant_id = Some("grant-1".to_string());
    binding.parent_transcript_digest =
        Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string());
    binding.previous_envelope_digest =
        Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string());
    binding
}

fn context_fixture(session_id: &str) -> SecureMeshContentContext {
    let created_at = OffsetDateTime::now_utc();
    let expires_at = created_at + Duration::minutes(10);
    SecureMeshContentContext::new(
        general_purpose::URL_SAFE_NO_PAD.encode(&Sha256::digest(b"env-acp-1")[..24]),
        "msg-acp-1",
        general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(b"mailbox-acp-1")),
        "agent-source",
        "agent-target",
        session_id,
        created_at.format(&Rfc3339).unwrap(),
        expires_at.format(&Rfc3339).unwrap(),
    )
}

struct EndpointFixture {
    identity: DeviceTrustPublicIdentity,
    identity_secret: SecureMeshPairwisePrivateKey,
    signing_key: SigningKey,
}

fn endpoint(endpoint_id: &str) -> EndpointFixture {
    let identity_secret = SecureMeshPairwisePrivateKey::generate();
    let signing_key = SigningKey::generate(&mut OsRng);
    let identity = DeviceTrustPublicIdentity::new(
        endpoint_id,
        identity_secret.public_key(),
        signing_key.verifying_key().to_bytes(),
        1,
    )
    .unwrap();
    EndpointFixture {
        identity,
        identity_secret,
        signing_key,
    }
}

fn paired_sessions() -> (SecureMeshPairwiseSession, SecureMeshPairwiseSession) {
    let alice = endpoint("agent-source");
    let bob = endpoint("agent-target");
    let signed_secret = SecureMeshPairwisePrivateKey::generate();
    let one_time_secret = SecureMeshPairwisePrivateKey::generate();
    let one_time_mlkem1024_prekey_seed = SecureMeshMlKem1024PreKeySeed::generate();
    let signed_prekey = sign_prekey_record(
        &bob.signing_key,
        &bob.identity,
        SecureMeshPreKeyKind::SignedPreKey,
        "spk-acp-1",
        signed_secret.public_key(),
        "2026-01-01T00:00:00.000Z",
        "2026-02-01T00:00:00.000Z",
    )
    .unwrap();
    let one_time_prekey = sign_prekey_record(
        &bob.signing_key,
        &bob.identity,
        SecureMeshPreKeyKind::OneTimePreKey,
        "otpk-acp-1",
        one_time_secret.public_key(),
        "2026-01-01T00:00:00.000Z",
        "2026-02-01T00:00:00.000Z",
    )
    .unwrap();
    let one_time_mlkem1024_prekey = sign_prekey_record(
        &bob.signing_key,
        &bob.identity,
        SecureMeshPreKeyKind::OneTimeMlKem1024PreKey,
        "pqotpk-acp-1",
        one_time_mlkem1024_prekey_seed.public_key(),
        "2026-01-01T00:00:00.000Z",
        "2026-02-01T00:00:00.000Z",
    )
    .unwrap();
    let bundle = SecureMeshPairwisePreKeyBundle {
        endpoint_identity: bob.identity.clone(),
        trust_state: DeviceTrustState::Verified,
        signed_prekey,
        one_time_prekey: Some(one_time_prekey),
        one_time_mlkem1024_prekey,
        prekey_publication_version: 1,
    };
    let directory_authorization = authorize_test_pairwise_prekey_bundle(&bundle);
    let now = OffsetDateTime::parse(
        "2026-01-01T00:00:01.000Z",
        &time::format_description::well_known::Rfc3339,
    )
    .unwrap();
    let (mut alice_session, intro) = SecureMeshPairwiseSession::initiate(
        &alice.identity,
        &alice.identity_secret,
        &alice.signing_key,
        &bundle,
        &directory_authorization,
        &SecureMeshPreKeyValidationPolicy::default(),
        &crate::core::secure_mesh_pairwise::secure_mesh_pairwise_test_capability_evaluation()
            .unwrap(),
        now,
    )
    .unwrap();
    let (mut bob_session, accepted) = SecureMeshPairwiseSession::accept(
        &bob.identity,
        &bob.identity_secret,
        &bob.signing_key,
        &alice.identity,
        &signed_secret,
        Some(&one_time_secret),
        &one_time_mlkem1024_prekey_seed,
        &intro,
        &crate::core::secure_mesh_pairwise::secure_mesh_pairwise_test_capability_evaluation()
            .unwrap(),
        now,
        &mut crate::core::secure_mesh_session_negotiation::CapabilityProofReplayGuard::default(),
    )
    .unwrap();
    let finished = alice_session
        .complete_initiator_handshake(
            &alice.identity,
            &bob.identity,
            &accepted,
            now,
            &mut crate::core::secure_mesh_session_negotiation::CapabilityProofReplayGuard::default(
            ),
        )
        .unwrap();
    bob_session.complete_responder_handshake(&finished).unwrap();
    (alice_session, bob_session)
}

#[test]
fn secure_mesh_acp_envelope_aad_has_stable_digest_vector() {
    let binding = binding_fixture(SecureMeshAcpPayloadClass::Prompt);
    let digest = acp_envelope_aad_digest(&binding).unwrap();
    assert_eq!(digest.len(), 64);
    assert_eq!(
        digest,
        "c5dcd95e8ae40644ea4cad031eca4351ac7e095bd10e5e9a219b3c60c1445534"
    );
}

#[test]
fn secure_mesh_acp_envelope_aad_field_mutation_fails_open() {
    let binding = binding_fixture(SecureMeshAcpPayloadClass::ToolArguments);
    let mut tampered = binding.clone();
    tampered.relay_turn_id = "relay-turn-tampered".to_string();
    let key = ContentKey::from_bytes([41u8; 32]);
    let context = context_fixture("session-acp-aad");
    let plaintext = SecureMeshPlaintext::new(
        SecureMeshPayloadKind::ServiceAction,
        br#"{"tool":"fs.write"}"#,
    )
    .with_content_type(binding.payload_class.content_type());
    let sealed = seal_payload_with_aad_binding(
        &key,
        &context,
        &plaintext,
        &encode_acp_envelope_aad(&binding).unwrap(),
    )
    .unwrap();
    let opened = open_payload_with_aad_binding(
        &key,
        &context,
        &sealed,
        SecureMeshPayloadKind::ServiceAction,
        &encode_acp_envelope_aad(&binding).unwrap(),
    )
    .unwrap();
    assert_eq!(opened.body, br#"{"tool":"fs.write"}"#);
    let err = open_payload_with_aad_binding(
        &key,
        &context,
        &sealed,
        SecureMeshPayloadKind::ServiceAction,
        &encode_acp_envelope_aad(&tampered).unwrap(),
    )
    .unwrap_err();
    let message = format!("{err:#}");
    assert!(
        message.contains("authentication")
            || message.contains("decrypt")
            || message.contains("aead")
            || message.contains("secure mesh"),
        "expected AAD mismatch failure, got {message}"
    );
}

#[test]
fn secure_mesh_acp_pairwise_protected_payload_round_trip() {
    let (mut alice, mut bob) = paired_sessions();
    let binding = binding_fixture(SecureMeshAcpPayloadClass::Prompt);
    let context = context_fixture(&alice.session_id);
    let sealed =
        seal_acp_protected_payload(&mut alice, &context, &binding, b"prompt-body").unwrap();
    let opened = open_acp_protected_payload(&mut bob, &context, &sealed, &binding).unwrap();
    assert_eq!(opened.body, b"prompt-body");
    assert_eq!(
        opened.content_type.as_deref(),
        Some(SecureMeshAcpPayloadClass::Prompt.content_type())
    );
}

#[test]
fn secure_mesh_acp_plaintext_protected_payload_relay_is_blocked() {
    let err = reject_plaintext_acp_protected_payload_relay(SecureMeshAcpPayloadClass::Prompt)
        .unwrap_err();
    assert!(format!("{err:#}").contains("plaintext protected-payload relay"));
    assert!(format!("{err:#}").contains("not a production path"));
}

fn acp_canary_variants(canary: &str) -> Vec<String> {
    let utf8 = canary.as_bytes();
    let base64 = general_purpose::STANDARD.encode(utf8);
    let base64url = general_purpose::URL_SAFE_NO_PAD.encode(utf8);
    let hex = utf8
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    vec![
        canary.to_string(),
        base64,
        base64url,
        hex,
        utf8.iter()
            .map(|byte| format!("\\u{byte:04x}"))
            .collect::<String>(),
    ]
}

fn relay_visible_envelope_text(envelope: &LicoArcRelayEnvelope) -> String {
    format!(
        "{}|{}|{}|{}|{}",
        envelope.contract_version(),
        envelope.envelope_id(),
        envelope.mailbox_id(),
        envelope.ciphertext(),
        envelope.expires_at()
    )
}

#[test]
fn secure_mesh_acp_sealed_payloads_hide_raw_and_encoded_canaries() {
    let protected_classes = [
        SecureMeshAcpPayloadClass::Prompt,
        SecureMeshAcpPayloadClass::Update,
        SecureMeshAcpPayloadClass::Reasoning,
        SecureMeshAcpPayloadClass::ToolArguments,
        SecureMeshAcpPayloadClass::ToolResult,
        SecureMeshAcpPayloadClass::FilesystemContent,
        SecureMeshAcpPayloadClass::TerminalContent,
        SecureMeshAcpPayloadClass::PermissionPayload,
        SecureMeshAcpPayloadClass::ApprovalDecision,
        SecureMeshAcpPayloadClass::Artifact,
        SecureMeshAcpPayloadClass::ArchiveLayer,
    ];
    for class in protected_classes {
        let (mut alice, mut bob) = paired_sessions();
        let binding = binding_fixture(class);
        let context = context_fixture(&alice.session_id);
        let canary = format!(
            "acp-{}-hostile-canary-{}-must-not-relay",
            class.as_str(),
            "PRIVATE"
        );
        let sealed =
            seal_acp_protected_payload(&mut alice, &context, &binding, canary.as_bytes()).unwrap();
        let opened = open_acp_protected_payload(&mut bob, &context, &sealed, &binding).unwrap();
        assert_eq!(opened.body, canary.as_bytes());
        let visible = relay_visible_envelope_text(&sealed);
        for variant in acp_canary_variants(&canary) {
            assert!(
                !visible.contains(&variant),
                "ACP {} relay-visible envelope leaked canary variant",
                class.as_str()
            );
        }
        for marker in ["rootKey", "chainKey", "messageKey", "sending_chain_key"] {
            assert!(
                !visible.contains(marker),
                "ACP {} relay-visible envelope leaked ratchet marker {marker}",
                class.as_str()
            );
        }
    }
}

#[test]
fn secure_mesh_acp_payload_classes_cover_protected_taxonomy() {
    for class in [
        SecureMeshAcpPayloadClass::Prompt,
        SecureMeshAcpPayloadClass::Update,
        SecureMeshAcpPayloadClass::Reasoning,
        SecureMeshAcpPayloadClass::ToolArguments,
        SecureMeshAcpPayloadClass::ToolResult,
        SecureMeshAcpPayloadClass::FilesystemContent,
        SecureMeshAcpPayloadClass::TerminalContent,
        SecureMeshAcpPayloadClass::PermissionPayload,
        SecureMeshAcpPayloadClass::ApprovalDecision,
        SecureMeshAcpPayloadClass::Artifact,
        SecureMeshAcpPayloadClass::ArchiveLayer,
    ] {
        assert!(class.is_protected());
        assert!(
            classify_acp_payload_class(class.as_str())
                .unwrap()
                .is_protected()
        );
    }
    assert!(!SecureMeshAcpPayloadClass::Receipt.is_protected());
}

#[test]
fn secure_mesh_acp_status_remains_independent_review_blocked() {
    assert!(SECURE_MESH_ACP_STATUS.contains("independent_review_pending"));
    assert!(SECURE_MESH_ACP_STATUS.contains("pqxdh_mlkem1024_triple_ratchet"));
    assert!(SECURE_MESH_ACP_STATUS.contains("plaintext_protected_payload_relay_blocked"));
    assert!(SECURE_MESH_ACP_STATUS.contains("acp_protected_envelope_aad_available"));
}
