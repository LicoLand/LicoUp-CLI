//! Minimal endpoint consumer of the fixed Candidate authority artifact.
//!
//! This is the smallest real consumer of the LicoArc SDK endpoint surface: this
//! crate admits an explicitly supplied authority artifact, and the verified
//! line is what an `Endpoint` is constructed from. The consumer compiles with
//! every build and test run. The artifact-dependent cases run only where the
//! authorized bundle is supplied explicitly through `LICOARC_AUTHORITY_BUNDLE`;
//! they are marked `#[ignore]` so an absent artifact is reported as not run
//! instead of passing silently.
//!
//! # Why the doubles here are not the adapter composition
//!
//! `NoKeyCustody` and `SyntheticStore` exist only to reach `Endpoint`
//! construction and fail closed on every operation; they compose nothing and
//! hold no caller-owned port. The production adapters that satisfy these same
//! SDK traits from the caller-owned ports live in `licoup-native`'s
//! `domain::mobile_relay::endpoint_ports`, because this crate may not depend on
//! the platform adapter: the direction is
//! `platform adapter → protocol bindings → fixed SDK`, and
//! `tests/dependency_direction.rs` proves this crate declares no such edge.

use std::{env, fs};

use licoarc::endpoint::{Endpoint, EndpointState, Responder};
use licoarc::error::Stage;
use licoarc::provider::RustCryptoProvider;
use licoarc::state::{
    AtomicState, Commit, CustodyRef, Ed25519Signing, KeyCustody, MlDsa65Signing, MlKem768Private,
    MlKemEncapsulationEntropy, PendingId, Revision, SecretHandle, StagedSecretHandle, Versioned,
    X25519Private,
};
use licoup_protocol_bindings::{AUTHORIZATION_REQUIRED, AuthorityInput, Error, ErrorCode};

/// L03 fixed input identities. They are asserted against the SDK-verified line
/// as evidence that this build integrates the recorded Candidate; the SDK
/// remains the only verifier.
const LINE_ID: &str = "c0b64d71865ce972a944db3d31a18cb03395300f3ed006c21e64429178c23a08";
const PROFILE_ID: &str = "4b7d575f397862f9031e44b716921e86c410b5facf379bde21955922b0d58a17";
const BUNDLE_DIGEST: &str = "b6ceacb359568cb09800317a8d4668442f55a1a66bba04f13c3e865a6cd2f8e0";

/// Custody that holds no key material. The minimal consumer cannot use keys
/// yet, so every operation reports a provider failure instead of inventing
/// material; real custody belongs to the platform and host nodes.
struct NoKeyCustody;

fn custody_unavailable() -> Error {
    Error::terminal(ErrorCode::ProviderFailure, Stage::Provider)
}

impl KeyCustody for NoKeyCustody {
    fn ed25519_public(&self, _handle: &SecretHandle<Ed25519Signing>) -> Result<[u8; 32], Error> {
        Err(custody_unavailable())
    }

    fn ed25519_sign(
        &self,
        _handle: &SecretHandle<Ed25519Signing>,
        _message: &[u8],
    ) -> Result<[u8; 64], Error> {
        Err(custody_unavailable())
    }

    fn ml_dsa_65_public(&self, _handle: &SecretHandle<MlDsa65Signing>) -> Result<Vec<u8>, Error> {
        Err(custody_unavailable())
    }

    fn ml_dsa_65_sign(
        &self,
        _handle: &SecretHandle<MlDsa65Signing>,
        _message: &[u8],
    ) -> Result<Vec<u8>, Error> {
        Err(custody_unavailable())
    }

    fn x25519_public(&self, _handle: CustodyRef<'_, X25519Private>) -> Result<[u8; 32], Error> {
        Err(custody_unavailable())
    }

    fn x25519(
        &self,
        _handle: CustodyRef<'_, X25519Private>,
        _public: &[u8; 32],
    ) -> Result<[u8; 32], Error> {
        Err(custody_unavailable())
    }

    fn ml_kem_768_public(
        &self,
        _handle: CustodyRef<'_, MlKem768Private>,
    ) -> Result<Vec<u8>, Error> {
        Err(custody_unavailable())
    }

    fn ml_kem_768_encapsulate(
        &self,
        _public: &[u8],
        _entropy: &SecretHandle<MlKemEncapsulationEntropy>,
    ) -> Result<(Vec<u8>, [u8; 32]), Error> {
        Err(custody_unavailable())
    }

    fn ml_kem_768_decapsulate(
        &self,
        _handle: &SecretHandle<MlKem768Private>,
        _ciphertext: &[u8],
    ) -> Result<[u8; 32], Error> {
        Err(custody_unavailable())
    }

    fn abort_x25519(&mut self, _staged: &StagedSecretHandle<X25519Private>) {}

    fn abort_ml_kem_768(&mut self, _staged: &StagedSecretHandle<MlKem768Private>) {}
}

/// One-snapshot store for the minimal consumer. It never holds pending work, so
/// settling an item is an invalid transition rather than a silent success.
struct SyntheticStore {
    current: Versioned<EndpointState>,
}

impl SyntheticStore {
    fn responder() -> Self {
        Self {
            current: Versioned::initial(EndpointState::responder()),
        }
    }
}

impl AtomicState<EndpointState> for SyntheticStore {
    fn load(&self) -> Result<Versioned<EndpointState>, Error> {
        Ok(self.current.clone())
    }

    fn compare_and_swap(
        &mut self,
        expected: Revision,
        commit: Commit<EndpointState>,
    ) -> Result<Revision, Error> {
        if expected != self.current.revision() {
            return Err(Error::terminal(ErrorCode::Conflict, Stage::Commit));
        }
        self.current = commit.into_versioned(expected)?;
        Ok(self.current.revision())
    }

    fn settle(&mut self, _revision: Revision, _pending: PendingId) -> Result<Revision, Error> {
        Err(Error::terminal(ErrorCode::InvalidTransition, Stage::Commit))
    }
}

fn authority_bytes() -> Vec<u8> {
    let path = env::var_os("LICOARC_AUTHORITY_BUNDLE")
        .expect("LICOARC_AUTHORITY_BUNDLE must name the explicit read-only authority artifact");
    fs::read(path).expect("the explicit authority artifact must be readable")
}

fn decode_hex(value: &str) -> [u8; 32] {
    std::array::from_fn(|index| {
        u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).expect("fixed identity is hex")
    })
}

#[test]
fn unknown_artifacts_are_refused() {
    for input in [
        b"".as_slice(),
        b"{}".as_slice(),
        br#"{"artifactVersion":"licoarc.bundle.v1"}"#.as_slice(),
        b"licoarc.protocol-line.v1".as_slice(),
    ] {
        let refused = AuthorityInput::new(input)
            .admit()
            .expect_err("only the fixed Candidate is admitted");
        assert_eq!(refused.code(), AUTHORIZATION_REQUIRED);
    }
}

#[test]
#[ignore = "requires the authorized LicoArc v1 authority artifact through LICOARC_AUTHORITY_BUNDLE"]
fn fixed_candidate_artifact_input_reaches_a_minimal_endpoint() {
    let bytes = authority_bytes();
    let line = AuthorityInput::new(&bytes)
        .admit()
        .expect("the fixed Candidate artifact is admitted");

    assert_eq!(line.wire_id(), "licoarc.protocol-line.v1");
    assert_eq!(line.generation(), 1);
    assert_eq!(line.protocol_line_id(), &decode_hex(LINE_ID));
    assert_eq!(line.protection_profile_id(), &decode_hex(PROFILE_ID));
    assert_eq!(
        line.snapshot_digest().as_bytes(),
        &decode_hex(BUNDLE_DIGEST)
    );
    assert_eq!(line.conformance_case_count(), 212);
    assert_eq!(line.operation_ids().len(), 29);

    let endpoint =
        Endpoint::<Responder, RustCryptoProvider, NoKeyCustody, SyntheticStore>::responder(
            line,
            RustCryptoProvider,
            NoKeyCustody,
            SyntheticStore::responder(),
        )
        .expect("the verified line constructs a responder endpoint");
    assert!(
        endpoint
            .pending_items()
            .expect("pending work is readable")
            .is_empty()
    );
}

#[test]
#[ignore = "requires the authorized LicoArc v1 authority artifact through LICOARC_AUTHORITY_BUNDLE"]
fn a_mutated_candidate_artifact_is_refused() {
    let bytes = authority_bytes();
    let text = String::from_utf8(bytes).expect("the authority artifact is JSON text");
    let mutated = text.replacen(LINE_ID, &"1".repeat(64), 1);
    assert_ne!(
        mutated, text,
        "the fixed line identity must appear verbatim"
    );

    let refused = AuthorityInput::new(mutated.as_bytes())
        .admit()
        .expect_err("a mutated artifact is not the fixed Candidate");
    assert_eq!(refused.code(), AUTHORIZATION_REQUIRED);
    assert_eq!(refused.cause().code, ErrorCode::DigestMismatch);
}
