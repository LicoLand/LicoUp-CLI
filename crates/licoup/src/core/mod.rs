// The full data-root archive owner lives in `licoup-foundation`; `licoup migrate`
// reaches it through this re-export at the path it uses.
pub use licoup_foundation::core::full_data_root_archive;
// The safe-archive extraction owner lives in `licoup-foundation`; the host's
// package restore reaches it at the crate-root path it always used.
pub use licoup_foundation::core::safe_archive;

// Linux currently exposes the portable contract without a native authorized-record backend.
#[cfg_attr(target_os = "linux", allow(dead_code))]
pub mod authorized_secure_record;
pub mod mcp;
pub mod secret_store;
pub mod security_capability;
