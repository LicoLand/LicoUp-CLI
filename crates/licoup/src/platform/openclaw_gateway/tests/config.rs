use licoup_foundation::platform::file_security::read_private_text_bounded;

use super::super::config::ensure_minimal;

#[test]
fn generated_config_is_private_bounded_and_loopback_only() {
    let root = std::env::temp_dir().join(format!("lico-openclaw-config-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("config.json");
    ensure_minimal(&path, 24189).unwrap();
    let body = read_private_text_bounded(&path, 16 * 1024)
        .unwrap()
        .unwrap();
    assert!(body.contains("\"bind\": \"loopback\""));
    assert!(!body.contains("token"));
    assert!(!body.contains("password"));
    let _ = std::fs::remove_dir_all(root);
}
