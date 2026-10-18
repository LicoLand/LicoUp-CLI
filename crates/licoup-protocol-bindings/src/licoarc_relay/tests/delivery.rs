//! Endpoint delivery primitive checks.

use super::support::*;

#[test]
fn delivery_secret_and_channel_binding_keep_key_material_opaque() {
    let secret = SecureMeshDeliverySecret::from_bytes([0x31; DELIVERY_SECRET_BYTES]);
    assert_eq!(secret.as_bytes(), &[0x31; DELIVERY_SECRET_BYTES]);
    assert_eq!(
        format!("{secret:?}"),
        "SecureMeshDeliverySecret([redacted])"
    );

    let binding = SecureMeshRelayChannelBinding::from_bytes([0x32; CHANNEL_BINDING_BYTES]);
    assert_eq!(binding.as_bytes(), &[0x32; CHANNEL_BINDING_BYTES]);
    assert_eq!(
        format!("{binding:?}"),
        "SecureMeshRelayChannelBinding([redacted])"
    );
}
