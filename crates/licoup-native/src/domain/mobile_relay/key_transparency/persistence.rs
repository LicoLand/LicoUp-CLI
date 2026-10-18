use crate::platform::client_state::ClientStateStore;
use anyhow::Result;
use licoup_foundation::platform::file_security::{
    create_private_state_marker, read_private_state_marker, remove_private_state_marker,
};
use std::path::PathBuf;

fn authority_challenge_path() -> Result<PathBuf> {
    Ok(ClientStateStore::portable()?
        .root()
        .join("mobile-relay")
        .join("secure-mesh-kt-authority-config.pending"))
}

pub(super) fn read_authority_challenge_marker() -> Result<Option<Vec<u8>>> {
    read_private_state_marker(&authority_challenge_path()?)
}

pub(super) fn create_authority_challenge_marker(value: &[u8]) -> Result<()> {
    create_private_state_marker(&authority_challenge_path()?, value)
}

pub(super) fn remove_authority_challenge_marker() -> Result<bool> {
    remove_private_state_marker(&authority_challenge_path()?)
}
