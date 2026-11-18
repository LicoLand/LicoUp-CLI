use super::{
    AbiIdentity, ArenaError, CLIENT_RUNTIME_ABI_VERSION, CLIENT_RUNTIME_OPERATIONS, HandleArena,
};

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
