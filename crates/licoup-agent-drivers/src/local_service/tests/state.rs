use super::super::state::{self, ServicePaths};
use serde_json::json;

#[test]
fn private_state_and_pid_round_trip_through_bounded_files() {
    let root =
        std::env::temp_dir().join(format!("lico-local-service-state-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let paths = ServicePaths::from_root(root.clone(), "serve.pid").unwrap();
    state::write_json(&paths.state_path, &json!({"status": "running"})).unwrap();
    assert_eq!(
        state::read_json(&paths.state_path, "invalid").unwrap()["status"],
        "running"
    );
    state::write_pid(&paths.pid_path, 42).unwrap();
    assert_eq!(state::read_pid(&paths.pid_path).unwrap(), Some(42));
    state::remove_pid(&paths.pid_path).unwrap();
    assert_eq!(state::read_pid(&paths.pid_path).unwrap(), None);
    let _ = std::fs::remove_dir_all(root);
}
