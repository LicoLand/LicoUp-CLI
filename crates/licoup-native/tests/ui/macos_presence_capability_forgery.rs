#![cfg(target_os = "macos")]

use licoup_native::core::secure_mesh_secret_store::{
    SecretStoreConsumedPresence, SecretStorePresenceGrant,
};
use licoup_native::platform::secure_mesh_secret_store::MacosAuthorizedPresence;

fn consumed_default() -> SecretStoreConsumedPresence {
    SecretStoreConsumedPresence::default()
}

fn consumed_clone(value: SecretStoreConsumedPresence) {
    let _ = value.clone();
}

fn consumed_public_constructor() -> SecretStoreConsumedPresence {
    SecretStoreConsumedPresence::new()
}

fn consumed_private_field() -> SecretStoreConsumedPresence {
    SecretStoreConsumedPresence {
        0: panic!("private consumed-presence field"),
    }
}

fn grant_default() -> SecretStorePresenceGrant {
    SecretStorePresenceGrant::default()
}

fn grant_clone(value: SecretStorePresenceGrant) {
    let _ = value.clone();
}

fn grant_public_constructor() -> SecretStorePresenceGrant {
    SecretStorePresenceGrant::new()
}

fn grant_private_field() -> SecretStorePresenceGrant {
    SecretStorePresenceGrant {
        0: panic!("private presence-grant field"),
    }
}

fn authorized_default() -> MacosAuthorizedPresence {
    MacosAuthorizedPresence::default()
}

fn authorized_clone(value: MacosAuthorizedPresence) {
    let _ = value.clone();
}

fn authorized_public_constructor() -> MacosAuthorizedPresence {
    MacosAuthorizedPresence::new()
}

fn authorized_private_field() -> MacosAuthorizedPresence {
    MacosAuthorizedPresence {
        0: panic!("private authorized-presence field"),
    }
}

fn main() {}
