//! Independent RFC 9162 inclusion and consistency proof components.

mod consistency;
mod hash;
mod inclusion;

pub(super) use consistency::checkpoint_from_sth;
#[cfg(any(test, feature = "secure-mesh-acceptance-mock-kt"))]
pub(super) use consistency::rfc9162_consistency_path;
pub use consistency::verify_kt_consistency;
pub(crate) use hash::kt_log_leaf_hash;
pub(super) use hash::map_root_log_leaf_hash;
#[cfg(any(test, feature = "secure-mesh-acceptance-mock-kt"))]
pub(super) use hash::merkle_tree_hash;
#[cfg(any(test, feature = "secure-mesh-acceptance-mock-kt"))]
pub(super) use inclusion::rfc9162_inclusion_path;
pub use inclusion::verify_kt_inclusion;
