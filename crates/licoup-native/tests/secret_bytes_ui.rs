use licoup_native::core::secure_mesh_secret_store::{
    SecretBytes, SecretStoreHandle, SecureMeshSecretStore,
};

#[test]
fn owned_secret_port_signature_compiles_for_an_ordinary_dependency() {
    fn write_owned(
        store: &dyn SecureMeshSecretStore,
        handle: &SecretStoreHandle,
        secret: SecretBytes,
    ) {
        let _ = store.set_secret(handle, secret);
    }

    fn read_owned(
        store: &dyn SecureMeshSecretStore,
        handle: &SecretStoreHandle,
    ) -> anyhow::Result<Option<SecretBytes>> {
        store.get_secret(handle)
    }

    let _ = write_owned;
    let _ = read_owned;
}

#[test]
fn secret_bytes_cannot_be_cloned_defaulted_or_forged_outside_the_library() {
    let cases = trybuild::TestCases::new();
    cases.compile_fail("tests/ui/secret_bytes_forgery.rs");
}
