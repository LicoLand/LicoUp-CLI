use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

use super::taxonomy::SecurityCapability;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretCustodyStrategy {
    MemoryOnlyEphemeral,
    OsSecureStore,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CustodyRestartSemantics {
    RePairRekeyAfterRestart,
    PersistentStateAvailable,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CustodySelection {
    pub strategy: SecretCustodyStrategy,
    pub restart_semantics: CustodyRestartSemantics,
    pub enabled_hardening: BTreeSet<SecurityCapability>,
}

pub(crate) fn custody_selection_from_enabled(
    enabled: &BTreeSet<SecurityCapability>,
) -> Option<CustodySelection> {
    if enabled.contains(&SecurityCapability::OsSecureStore) {
        return Some(CustodySelection {
            strategy: SecretCustodyStrategy::OsSecureStore,
            restart_semantics: CustodyRestartSemantics::PersistentStateAvailable,
            enabled_hardening: enabled
                .iter()
                .copied()
                .filter(|capability| capability.id().starts_with("custody."))
                .collect(),
        });
    }
    enabled
        .contains(&SecurityCapability::MemoryOnlyEphemeral)
        .then(|| CustodySelection {
            strategy: SecretCustodyStrategy::MemoryOnlyEphemeral,
            restart_semantics: CustodyRestartSemantics::RePairRekeyAfterRestart,
            enabled_hardening: BTreeSet::from([SecurityCapability::MemoryOnlyEphemeral]),
        })
}
