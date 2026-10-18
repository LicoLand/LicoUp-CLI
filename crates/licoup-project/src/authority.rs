//! The authority answer this owner asks for instead of inventing.
//!
//! A registration names an [`AuthorityReference`]; whether that reference is
//! currently authorized is a fact the existing authority owner holds. This
//! crate declares the question so the composition answers it from the owner that
//! already knows, and so a test can answer it without a running process.

use crate::identity::AuthorityReference;

/// Decide whether one authority reference is currently authorized.
///
/// Implementations answer from the authority owner that issued the reference:
/// the verified caller of the request in the running client, and the deployment
/// role or grant registry where one exists. The trait exists so a registration
/// can never be admitted by the module that records it.
pub trait ProjectAuthorityDirectory: Send + Sync {
    fn admits(&self, reference: &AuthorityReference) -> bool;
}

/// An authority owner that admits nothing.
///
/// This is the fail-closed answer for a process that has not composed a real
/// authority owner yet: with it, a registration is refused as unauthorized
/// rather than recorded on trust.
pub struct NoAuthorityDirectory;

impl ProjectAuthorityDirectory for NoAuthorityDirectory {
    fn admits(&self, _reference: &AuthorityReference) -> bool {
        false
    }
}
