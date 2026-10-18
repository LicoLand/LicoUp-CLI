use super::super::mailbox::canonical_mailbox_token;
use crate::domain::mobile_relay::secret_custody::RuntimeSecretMaterial;

#[test]
fn mailbox_token_requires_local_delivery_secret() {
    let error = canonical_mailbox_token(&RuntimeSecretMaterial::new(), "endpoint", "mobile", 1)
        .err()
        .expect("missing delivery secret must be rejected");
    assert!(error.to_string().contains("delivery secret is missing"));
}
