use super::super::store::mobile_relay_pairwise_store_path;

#[test]
fn durable_store_uses_the_canonical_pairwise_database_name() {
    let path = mobile_relay_pairwise_store_path().unwrap();
    assert_eq!(
        path.file_name().and_then(|value| value.to_str()),
        Some("pairwise-pqxdh.sqlite3")
    );
}
