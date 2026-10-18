use super::{
    AbiIdentity, ArenaError, CLIENT_RUNTIME_ABI_VERSION, CLIENT_RUNTIME_OPERATIONS, HandleArena,
    MOBILE_RUNTIME_ABI_VERSION, MOBILE_RUNTIME_LAYOUT_IDENTITY, MOBILE_RUNTIME_OPERATIONS,
    mobile_abi_identity,
};
use std::collections::BTreeSet;

#[test]
fn abi_identity_matches_canonical_operations() {
    let identity = AbiIdentity::load();
    assert_eq!(identity.abi_version, CLIENT_RUNTIME_ABI_VERSION);
    assert_eq!(
        identity.layout_identity,
        "licoup.client-runtime.abi.v1.generation-index"
    );
    assert_eq!(
        identity.operations,
        CLIENT_RUNTIME_OPERATIONS
            .iter()
            .map(|operation| (*operation).to_string())
            .collect::<Vec<_>>()
    );
}

#[test]
fn mobile_abi_identity_is_the_declared_mobile_surface() {
    let identity = mobile_abi_identity();
    assert_eq!(identity.abi_version, MOBILE_RUNTIME_ABI_VERSION);
    assert_eq!(identity.layout_identity, MOBILE_RUNTIME_LAYOUT_IDENTITY);
    assert_eq!(identity.operations.len(), MOBILE_RUNTIME_OPERATIONS.len());
    assert!(
        identity
            .operations
            .iter()
            .all(|operation| MOBILE_RUNTIME_OPERATIONS.contains(&operation.as_str()))
    );
    let unique: BTreeSet<&String> = identity.operations.iter().collect();
    assert_eq!(
        unique.len(),
        identity.operations.len(),
        "a mobile operation is declared twice"
    );
}

#[test]
fn the_desktop_runtime_and_the_mobile_entry_are_distinct_abi_identities() {
    let desktop = AbiIdentity::load();
    let mobile = mobile_abi_identity();
    assert_ne!(desktop.layout_identity, mobile.layout_identity);
    assert!(
        mobile
            .operations
            .iter()
            .all(|operation| !desktop.operations.contains(operation)),
        "the mobile entry restates a desktop runtime operation; the two surfaces ask different \
         questions and must not be aliases of one another"
    );
}

#[test]
fn arena_rejects_stale_handles_and_respects_capacity() {
    let mut arena = HandleArena::bounded(1);
    let first = arena.insert(7_u32).expect("insert");
    assert_eq!(arena.get(first).copied(), Some(7));
    assert_eq!(arena.insert(8), Err(ArenaError::CapacityExceeded));

    let value = arena.free(first).expect("free");
    assert_eq!(value, 7);
    assert_eq!(arena.get(first), None);
    assert_eq!(arena.free(first), Err(ArenaError::StaleHandle));

    let second = arena.insert(9).expect("reuse");
    assert_ne!(second.generation(), first.generation());
    assert_eq!(arena.get(first), None);
    assert_eq!(arena.get(second).copied(), Some(9));
}
