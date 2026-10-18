use super::support::*;

#[test]
fn secure_mesh_mls_key_package_requires_authenticated_mlkem1024_wrapper() {
    let participant = SecureMeshMlsParticipant::new(b"mobile:key-package".to_vec()).unwrap();
    let key_package = participant.generate_key_package().unwrap();
    assert_eq!(
        key_package.mlkem1024_public_key().len(),
        crate::core::secure_mesh_pqxdh::ML_KEM_1024_PUBLIC_KEY_BYTES
    );
    assert!(SecureMeshMlsKeyPackage::from_public_bytes(key_package.as_public_bytes()).is_ok());
    let unwrapped_openmls_key_package = key_package
        .public_key_package
        .tls_serialize_detached()
        .unwrap();
    assert!(SecureMeshMlsKeyPackage::from_public_bytes(&unwrapped_openmls_key_package).is_err());
    let mut tampered = key_package.as_public_bytes().to_vec();
    tampered[0] ^= 0x01;
    assert!(SecureMeshMlsKeyPackage::from_public_bytes(&tampered).is_err());
}
