use super::super::transaction::mobile_relay_pairwise_operation;
use crate::domain::mobile_relay::secret_custody::RuntimeSecretMaterial;
use serde_json::json;

#[test]
fn transaction_open_fails_closed_without_initialized_session() {
    assert!(
        mobile_relay_pairwise_operation(
            &json!({}),
            &RuntimeSecretMaterial::new(),
            "test transaction",
            1,
        )
        .is_err()
    );
}
