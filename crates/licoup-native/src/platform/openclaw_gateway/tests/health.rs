use super::super::health::normalize_remote;

#[test]
fn remote_gateway_requires_tls_unless_it_is_loopback() {
    assert_eq!(
        normalize_remote("wss://gateway.example").unwrap(),
        (
            "https://gateway.example".to_string(),
            "wss://gateway.example".to_string()
        )
    );
    assert!(normalize_remote("ws://gateway.example").is_err());
    assert!(normalize_remote("ws://127.0.0.1:18789").is_ok());
    assert!(normalize_remote("https://token@gateway.example").is_err());
}
