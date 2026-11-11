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
    assert!(SecurityCapability::from_id("protocol.authenticated_encryption").is_err());
    assert!(SecurityCapability::from_id("custody.unknown").is_err());
}

#[test]
fn identifiers_all_live_in_the_local_custody_namespace() {
    for capability in SecurityCapability::ALL {
        assert!(capability.id().starts_with("custody."));
    }
}
