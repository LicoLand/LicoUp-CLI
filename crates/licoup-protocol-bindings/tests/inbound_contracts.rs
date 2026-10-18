//! Client-boundary contracts that do not need the gated authority artifact.
//!
//! The version contract, the blocked-scope report, and the per-call refusal
//! behaviour run on every build. The one case that needs the authorized
//! authority artifact is `#[ignore]`d and reads it from
//! `LICOARC_AUTHORITY_BUNDLE`, so an absent artifact is reported as not run
//! instead of passing silently.

use licoup_protocol_bindings::{
    ACCEPTED_GENERATION, ACCEPTED_PROTECTION_PROFILE_ID, ACCEPTED_PROTOCOL_LINE_ID,
    ACCEPTED_WIRE_ID, AcceptedVersion, AuthorityInput, BLOCKED_SCOPES, EndpointConsumer,
    ProtocolVersion, VersionRefusal,
};

fn restored(
    wire_id: &str,
    generation: u64,
    protocol_line_id: [u8; 32],
    protection_profile_id: [u8; 32],
) -> ProtocolVersion {
    ProtocolVersion::restored(wire_id, generation, protocol_line_id, protection_profile_id)
}

/// A consumer is constructible in a const context, so constructing one performs
/// no runtime initialization: no custody backend is opened, no pairing state is
/// created, and no listener is started. A client without the optional pairing
/// package therefore stays inert.
const INERT_CONSUMER: EndpointConsumer = EndpointConsumer::new();

#[test]
fn a_consumer_is_constructed_without_initializing_anything() {
    assert_eq!(INERT_CONSUMER.accepted(), AcceptedVersion::fixed());
    assert_eq!(
        INERT_CONSUMER.accepted(),
        EndpointConsumer::new().accepted()
    );
}

#[test]
fn blocked_scopes_name_their_branch_and_their_reason() {
    let capabilities: Vec<&str> = BLOCKED_SCOPES
        .iter()
        .map(|scope| scope.capability)
        .collect();
    assert_eq!(capabilities.len(), 1);
    for scope in BLOCKED_SCOPES {
        assert!(!scope.capability.is_empty());
        assert!(
            scope.reason.len() > 32,
            "a blocked scope must name the missing input, not just its name"
        );
    }
    assert!(
        BLOCKED_SCOPES
            .iter()
            .any(|scope| scope.reason.contains("LICOARC_AUTHORITY_BUNDLE")),
        "the missing credential is named by the environment input that supplies it"
    );
    assert!(
        BLOCKED_SCOPES
            .iter()
            .any(|scope| scope.capability.contains("protected-record exchange")),
        "only the communication branch is blocked by the missing artifact"
    );
    assert!(
        !BLOCKED_SCOPES
            .iter()
            .any(|scope| scope.capability.contains("version")),
        "the version contract is local work and stays unblocked"
    );
}

#[test]
fn a_blocked_branch_does_not_block_local_work() {
    let consumer = EndpointConsumer::new();
    assert_eq!(consumer.accepted(), AcceptedVersion::fixed());
    assert_eq!(AcceptedVersion::default(), AcceptedVersion::fixed());

    // Local admission still refuses an artifact that is not the fixed Candidate,
    // and the client is still usable immediately afterwards.
    let refused = AuthorityInput::new(b"{}")
        .admit()
        .expect_err("only the fixed Candidate is admitted");
    assert_eq!(refused.code(), "authorization_required");
    assert_eq!(consumer.accepted(), AcceptedVersion::fixed());
}

#[test]
fn an_unaccepted_version_refuses_that_call_only() {
    let consumer = EndpointConsumer::new();
    let acceptance = consumer.accepted();

    // A persisted session record from another line, another generation, or
    // another profile is refused, one field at a time.
    let mut changed_line = ACCEPTED_PROTOCOL_LINE_ID;
    changed_line[31] ^= 1;
    let mut changed_profile = ACCEPTED_PROTECTION_PROFILE_ID;
    changed_profile[31] ^= 1;
    for version in [
        restored("licoarc.protocol-line.v2", 1, changed_line, changed_profile),
        restored(
            ACCEPTED_WIRE_ID,
            ACCEPTED_GENERATION + 1,
            ACCEPTED_PROTOCOL_LINE_ID,
            ACCEPTED_PROTECTION_PROFILE_ID,
        ),
        restored(
            ACCEPTED_WIRE_ID,
            ACCEPTED_GENERATION,
            changed_line,
            ACCEPTED_PROTECTION_PROFILE_ID,
        ),
        restored(
            ACCEPTED_WIRE_ID,
            ACCEPTED_GENERATION,
            ACCEPTED_PROTOCOL_LINE_ID,
            changed_profile,
        ),
    ] {
        let refusal = acceptance
            .check(&version)
            .expect_err("this client does not accept that version");
        assert!(refusal.code().starts_with("unsupported_"));
        assert!(refusal.to_string().starts_with(refusal.code()));
        // The same client, with the same state, still accepts its own version.
        assert_eq!(
            acceptance.check(&restored(
                ACCEPTED_WIRE_ID,
                ACCEPTED_GENERATION,
                ACCEPTED_PROTOCOL_LINE_ID,
                ACCEPTED_PROTECTION_PROFILE_ID,
            )),
            Ok(())
        );
    }
    assert_eq!(
        acceptance.check(&restored("", 0, [0; 32], [0; 32])),
        Err(VersionRefusal::WireId {
            accepted: ACCEPTED_WIRE_ID
        })
    );
}

#[test]
#[ignore = "requires the authorized LicoArc v1 authority artifact through LICOARC_AUTHORITY_BUNDLE"]
fn the_fixed_candidate_line_is_the_accepted_version_and_its_identity_is_trusted() {
    use std::env;
    use std::fs;

    use licoarc::endpoint::IdentityPublic;
    use licoarc::error::{Error, ErrorCode, Stage};
    use licoarc::provider::{RustCryptoProvider, SignatureProvider};
    use licoarc::state::TrustFacts as TrustFactsPort;

    /// Records the identity keys this client already holds for one device. A real
    /// platform reads them from OS custody; the shape is the same.
    struct RecordedIdentity {
        identity: IdentityPublic,
        profile: [u8; 32],
    }

    impl TrustFactsPort for RecordedIdentity {
        fn identity_key(
            &self,
            identity_state_digest: &[u8; 32],
            purpose: &'static str,
            profile: &[u8; 32],
        ) -> Result<Vec<u8>, Error> {
            if *identity_state_digest != self.identity.state_digest || *profile != self.profile {
                return Err(Error::terminal(
                    ErrorCode::AuthenticationFailed,
                    Stage::Validation,
                ));
            }
            match purpose {
                "ed25519-key-id" => Ok(self.identity.ed25519_key_id.to_vec()),
                "ed25519-public" => Ok(self.identity.ed25519_public.to_vec()),
                "ml-dsa-65-key-id" => Ok(self.identity.ml_dsa_65_key_id.to_vec()),
                "ml-dsa-65-public" => Ok(self.identity.ml_dsa_65_public.clone()),
                _ => Err(Error::terminal(
                    ErrorCode::AuthenticationFailed,
                    Stage::Validation,
                )),
            }
        }
    }

    let provider = RustCryptoProvider;
    let path = env::var_os("LICOARC_AUTHORITY_BUNDLE")
        .expect("LICOARC_AUTHORITY_BUNDLE must name the explicit read-only authority artifact");
    let bytes = fs::read(path).expect("the explicit authority artifact must be readable");
    let line = AuthorityInput::new(&bytes)
        .admit()
        .expect("the fixed Candidate artifact is admitted");

    let consumer = EndpointConsumer::new();
    let version = consumer
        .check_line(&line)
        .expect("the fixed Candidate is this client's accepted version");
    assert_eq!(version.wire_id(), ACCEPTED_WIRE_ID);
    assert_eq!(version.generation(), ACCEPTED_GENERATION);
    assert_eq!(version.protocol_line_id(), ACCEPTED_PROTOCOL_LINE_ID);
    assert_eq!(
        version.protection_profile_id(),
        ACCEPTED_PROTECTION_PROFILE_ID
    );

    // A caller that restores a version record from another generation is refused
    // for that call only; the admitted line keeps working.
    assert_eq!(
        AcceptedVersion::fixed().check(&restored(
            ACCEPTED_WIRE_ID,
            ACCEPTED_GENERATION + 1,
            ACCEPTED_PROTOCOL_LINE_ID,
            ACCEPTED_PROTECTION_PROFILE_ID,
        )),
        Err(VersionRefusal::Generation {
            accepted: ACCEPTED_GENERATION
        })
    );
    assert!(consumer.check_line(&line).is_ok());

    let seed = [3_u8; 32];
    let identity = IdentityPublic {
        state_digest: [4; 32],
        ed25519_key_id: [5; 32],
        ed25519_public: provider.ed25519_public(&seed),
        ml_dsa_65_key_id: [6; 32],
        ml_dsa_65_public: provider.ml_dsa_65_public(&seed),
    };
    let trust = RecordedIdentity {
        identity: identity.clone(),
        profile: *line.protection_profile_id(),
    };
    assert!(
        consumer.trust_peer(&line, &trust, &identity).is_ok(),
        "an identity whose recorded keys match is trusted through the SDK entry"
    );

    // The caller presents a different identity than the keys it recorded. The
    // SDK compares every recorded key against the presented identity itself, so
    // the mismatch is refused rather than trusted.
    let mut other = identity;
    other.ed25519_key_id[0] ^= 1;
    let refusal = consumer
        .trust_peer(&line, &trust, &other)
        .expect_err("an identity whose recorded keys disagree is refused");
    assert_eq!(refusal.code(), "protocol_refusal");
    assert_eq!(refusal.cause().unwrap().code, ErrorCode::HandshakeRejected);
}
