use std::fs;
use std::path::PathBuf;

use serde_json::json;
use uuid::Uuid;

use super::super::super::super::package::SelectedPayloadFile;
use super::super::super::super::registration::{AgentDestination, PlannedAgentRegistration};
use super::super::super::model::sha256_hex;
use super::super::staging::{commit_staged_units, stage_mcp_units};

fn fixture_root(label: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "lico-collaboration-staging-{label}-{}",
        Uuid::new_v4()
    ));
    fs::create_dir_all(root.join("outputs")).unwrap();
    fs::create_dir_all(root.join("registrations")).unwrap();
    root
}

fn payload() -> Vec<SelectedPayloadFile> {
    let bytes = b"plugin".to_vec();
    vec![SelectedPayloadFile {
        selection_id: "mcp-alpha".to_owned(),
        source_relative_path: PathBuf::from("payload/mcp-alpha/plugin.json"),
        destination_relative_path: PathBuf::from("mcp-alpha/plugin.json"),
        digest_sha256: sha256_hex(&bytes),
        bytes,
    }]
}

#[test]
fn mcp_staging_commits_payload_and_private_registration_together() {
    let root = fixture_root("commit");
    let install_destination = root.join("outputs/cursor");
    let registration_destination = root.join("registrations/cursor.json");
    let content = "{\"schemaVersion\":\"test\"}\n".to_owned();
    let destinations = vec![AgentDestination {
        agent_id: "cursor".to_owned(),
        install_destination: install_destination.to_string_lossy().into_owned(),
    }];
    let registrations = vec![PlannedAgentRegistration {
        agent_id: "cursor".to_owned(),
        registration_id: Uuid::new_v4().to_string(),
        destination: registration_destination.to_string_lossy().into_owned(),
        digest_sha256: sha256_hex(content.as_bytes()),
        content: content.clone(),
    }];

    let units = stage_mcp_units(&payload(), &registrations, &destinations).unwrap();
    assert!(!commit_staged_units(&units, &json!({})).unwrap());
    assert_eq!(
        fs::read(install_destination.join("mcp-alpha/plugin.json")).unwrap(),
        b"plugin"
    );
    assert_eq!(
        fs::read_to_string(registration_destination).unwrap(),
        content
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn mcp_staging_rejects_registration_digest_drift_before_commit() {
    let root = fixture_root("digest");
    let install_destination = root.join("outputs/cursor");
    let registration_destination = root.join("registrations/cursor.json");
    let destinations = vec![AgentDestination {
        agent_id: "cursor".to_owned(),
        install_destination: install_destination.to_string_lossy().into_owned(),
    }];
    let registrations = vec![PlannedAgentRegistration {
        agent_id: "cursor".to_owned(),
        registration_id: Uuid::new_v4().to_string(),
        destination: registration_destination.to_string_lossy().into_owned(),
        digest_sha256: "0".repeat(64),
        content: "{}\n".to_owned(),
    }];

    let error = match stage_mcp_units(&payload(), &registrations, &destinations) {
        Ok(_) => panic!("registration digest drift must fail before commit"),
        Err(error) => error,
    };
    assert_eq!(
        error.to_string(),
        "collaboration_mcp_registration_digest_mismatch"
    );
    assert!(!install_destination.exists());
    assert!(!registration_destination.exists());
    let _ = fs::remove_dir_all(root);
}
