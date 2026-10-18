//! Endpoint identity and custody boundary for the mobile entry.
//!
//! The mobile client's endpoint identity is custody the *platform* holds: the
//! Android Keystore and the iOS Keychain own the private material, and the
//! pinned SDK owns the protocol. Neither of those is re-declared here. This
//! module reads the caller-owned port values the endpoint core already
//! declares and applies the one admission rule the mobile entry adds: an
//! operation that will write durable protocol state may only proceed from an
//! unlocked session with *adopted* custody of a purpose the mobile entry
//! actually requires.
//!
//! A staged handle is the SDK's tentative custody. Admitting it here would let
//! a tentative key authorise a durable write, so it is refused by name instead
//! of being silently promoted.

use licoup_endpoint_core::{ClientSession, CustodyHandle, CustodyLifecycle, CustodyPurpose};

/// The custody purposes the mobile entry requires.
///
/// Ed25519 is the endpoint signing identity the pairing claim is authenticated
/// with; X25519 is the private half of the durable delivery channel. The
/// post-quantum purposes are part of the same protocol but are not required to
/// answer this surface, so their absence is not a mobile-entry failure.
pub const MOBILE_REQUIRED_CUSTODY: &[CustodyPurpose] = &[
    CustodyPurpose::Ed25519Signing,
    CustodyPurpose::X25519Private,
];

/// Why one custody handle was refused for the mobile entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IdentityRefusal {
    /// The session is locked or terminated, so no durable write may start.
    SessionNotUnlocked,
    /// The handle serves a purpose this surface does not require.
    PurposeNotRequired(CustodyPurpose),
    /// The handle is tentative custody, not adopted custody.
    TentativeCustody,
}

impl IdentityRefusal {
    /// The stable wire label, so a caller reads one code per refusal.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::SessionNotUnlocked => "mobile_endpoint_session_locked",
            Self::PurposeNotRequired(_) => "mobile_endpoint_custody_purpose_unrequired",
            Self::TentativeCustody => "mobile_endpoint_custody_tentative",
        }
    }
}

/// Admit one custody handle for a durable mobile operation.
///
/// The handle is re-checked rather than trusted: purpose and lifecycle travel
/// as values across the port boundary precisely because the port
/// implementation must verify them again, and this is that verification for
/// the mobile surface.
pub fn admit_custody(
    session: ClientSession,
    handle: CustodyHandle,
) -> Result<CustodyPurpose, IdentityRefusal> {
    if !session.allows_ordinary_interaction() {
        return Err(IdentityRefusal::SessionNotUnlocked);
    }
    let purpose = handle.purpose();
    if !MOBILE_REQUIRED_CUSTODY.contains(&purpose) {
        return Err(IdentityRefusal::PurposeNotRequired(purpose));
    }
    if handle.lifecycle() != CustodyLifecycle::Adopted {
        return Err(IdentityRefusal::TentativeCustody);
    }
    Ok(purpose)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adopted_required_custody_is_admitted_only_from_an_unlocked_session() {
        let handle = CustodyHandle::adopted(7, CustodyPurpose::Ed25519Signing);
        assert_eq!(
            admit_custody(ClientSession::device_unlocked(), handle),
            Ok(CustodyPurpose::Ed25519Signing)
        );
        assert_eq!(
            admit_custody(ClientSession::app_locked(), handle),
            Err(IdentityRefusal::SessionNotUnlocked)
        );
        assert_eq!(
            admit_custody(ClientSession::terminated(), handle),
            Err(IdentityRefusal::SessionNotUnlocked)
        );
    }

    #[test]
    fn tentative_custody_and_unrequired_purposes_are_refused_by_name() {
        assert_eq!(
            admit_custody(
                ClientSession::device_unlocked(),
                CustodyHandle::staged(9, CustodyPurpose::X25519Private),
            ),
            Err(IdentityRefusal::TentativeCustody)
        );
        assert_eq!(
            admit_custody(
                ClientSession::device_unlocked(),
                CustodyHandle::adopted(9, CustodyPurpose::MlKem768Private),
            ),
            Err(IdentityRefusal::PurposeNotRequired(
                CustodyPurpose::MlKem768Private
            ))
        );
    }
}
