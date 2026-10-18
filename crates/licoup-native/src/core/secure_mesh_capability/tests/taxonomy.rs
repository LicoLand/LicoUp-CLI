use std::collections::BTreeSet;

use super::super::SecurityCapability;

#[test]
fn identifiers_are_unique_exhaustive_and_round_trip_without_linear_lookup() {
    let identifiers = SecurityCapability::ALL
        .iter()
        .map(|capability| capability.id())
        .collect::<BTreeSet<_>>();
    assert_eq!(identifiers.len(), SecurityCapability::COUNT);

    for capability in SecurityCapability::ALL {
        assert_eq!(
            SecurityCapability::from_id(capability.id()).unwrap(),
            capability
        );
        let encoded = serde_json::to_string(&capability).unwrap();
        assert_eq!(
            serde_json::from_str::<SecurityCapability>(&encoded).unwrap(),
            capability
        );
    }
    assert!(SecurityCapability::from_id("protocol.unknown").is_err());
}

#[test]
fn identifiers_remain_partitioned_between_protocol_and_local_custody() {
    for capability in SecurityCapability::ALL {
        assert!(
            capability.id().starts_with("protocol.") || capability.id().starts_with("custody.")
        );
    }
}
