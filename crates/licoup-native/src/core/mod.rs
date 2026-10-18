// The full data-root archive owner lives in `licoup-foundation`; `licoup-migrate`
// reaches it through this re-export at the path it uses.
pub use licoup_foundation::core::full_data_root_archive;
// The safe-archive extraction owner lives in `licoup-foundation`; the host's
// package restore reaches it at the crate-root path it always used.
pub use licoup_foundation::core::safe_archive;

// Linux currently exposes the portable contract without a native authorized-record backend.
#[cfg_attr(target_os = "linux", allow(dead_code))]
pub mod authorized_secure_record;
pub mod mcp;
pub mod secure_mesh;
pub mod secure_mesh_acp;
pub mod secure_mesh_approval;
pub mod secure_mesh_capability;
pub mod secure_mesh_capability_proof;
pub mod secure_mesh_command;
pub mod secure_mesh_crypto;
pub mod secure_mesh_directory;
pub mod secure_mesh_file;
pub mod secure_mesh_lifecycle;
pub mod secure_mesh_mlkem_braid;
pub mod secure_mesh_mls;
pub(crate) mod secure_mesh_mls_pq_epoch;
pub mod secure_mesh_mls_product;
pub mod secure_mesh_pairwise;
pub mod secure_mesh_pqxdh;
pub mod secure_mesh_prekey;
pub mod secure_mesh_product_readiness;
pub mod secure_mesh_response;
pub mod secure_mesh_secret_store;
pub mod secure_mesh_session_negotiation;
pub mod secure_mesh_sparse_pq_ratchet;
pub mod secure_mesh_transparency;
pub mod secure_mesh_trust;
