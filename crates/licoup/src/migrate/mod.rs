//! Standalone migration tool for LicoUp local data.
//!
//! This tool is packaged and shipped on its own; a user needs no Node.js runtime and no
//! client installation to run it. It holds no schema authority of its own: the domain
//! catalog, the target versions and the observed state all come from the client's native
//! owners, and everything this crate does is read those facts, plan against them, and
//! report the result.

pub mod archive;
pub mod cli;
pub mod convert;
pub mod converter;
pub mod converter_process;
pub mod error;
pub mod inspect;
pub mod inventory;
pub mod journal;
pub mod package_conversion;
pub mod package_journal;
pub mod plan;
pub mod rehearse;
pub mod resume;
