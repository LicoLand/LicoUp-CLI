//! Same-process ABI identity and generation-index handle arena stubs.

mod abi;
mod handle_arena;

pub use abi::{
    AbiIdentity, CLIENT_RUNTIME_ABI_JSON, CLIENT_RUNTIME_ABI_VERSION, CLIENT_RUNTIME_OPERATIONS,
};
pub use handle_arena::{ArenaError, Handle, HandleArena};

#[cfg(test)]
mod tests;
