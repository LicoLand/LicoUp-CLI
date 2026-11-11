use std::collections::BTreeSet;

use super::super::custody::custody_selection_from_enabled;
use super::super::{CustodyRestartSemantics, SecretCustodyStrategy, SecurityCapability};

#[test]
fn memory_only_strategy_requires_repair_after_restart() {
    let enabled = BTreeSet::from([SecurityCapability::MemoryOnlyEphemeral]);
    let selection = custody_selection_from_enabled(&enabled).unwrap();
    assert_eq!(
        selection.strategy,
        SecretCustodyStrategy::MemoryOnlyEphemeral
    );
    assert_eq!(
        selection.restart_semantics,
        CustodyRestartSemantics::RePairRekeyAfterRestart
    );
    assert_eq!(selection.enabled_hardening, enabled);
}

#[test]
fn os_store_takes_precedence_and_projects_the_enabled_custody_hardening() {
    let enabled = BTreeSet::from([
        SecurityCapability::MemoryOnlyEphemeral,
        SecurityCapability::OsSecureStore,
        SecurityCapability::SoftwareBacked,
    ]);
    let selection = custody_selection_from_enabled(&enabled).unwrap();
    assert_eq!(selection.strategy, SecretCustodyStrategy::OsSecureStore);
    assert_eq!(
        selection.restart_semantics,
        CustodyRestartSemantics::PersistentStateAvailable
    );
    assert_eq!(selection.enabled_hardening, enabled);
}

#[test]
fn absent_custody_capability_produces_no_strategy() {
    assert!(custody_selection_from_enabled(&BTreeSet::new()).is_none());
}
