use std::fs;

use super::super::super::super::registration::AgentDestination;
use super::super::destination_policy::{
    parse_absolute_path, relative_path_text, validate_agent_destinations,
};

#[test]
fn destination_paths_must_be_absolute_and_relative_payloads_cannot_escape() {
    assert_eq!(
        parse_absolute_path("relative/path")
            .unwrap_err()
            .to_string(),
        "collaboration_workflow_destination_must_be_absolute"
    );
    assert_eq!(
        relative_path_text(std::path::Path::new("../escape"))
            .unwrap_err()
            .to_string(),
        "collaboration_workflow_relative_path_invalid"
    );
}

#[test]
fn duplicate_destination_paths_are_rejected_before_staging() {
    let root = std::env::temp_dir().join(format!(
        "lico-collaboration-destination-policy-{}",
        uuid::Uuid::new_v4()
    ));
    fs::create_dir_all(&root).unwrap();
    let shared = root.join("shared").to_string_lossy().into_owned();
    let destinations = vec![
        AgentDestination {
            agent_id: "cursor".to_owned(),
            install_destination: shared.clone(),
        },
        AgentDestination {
            agent_id: "hermes".to_owned(),
            install_destination: shared,
        },
    ];
    assert_eq!(
        validate_agent_destinations(&destinations)
            .unwrap_err()
            .to_string(),
        "collaboration_workflow_destination_overlap"
    );
    fs::remove_dir_all(root).unwrap();
}
