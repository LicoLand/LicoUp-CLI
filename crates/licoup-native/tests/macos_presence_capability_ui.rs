use licoup_native::core::secure_mesh_secret_store::{
    SecretStoreConsumedPresence, SecretStoreHandle, SecretStorePresenceGrant, SecureMeshSecretStore,
};
#[cfg(target_os = "macos")]
use licoup_native::platform::secure_mesh_secret_store::MacosAuthorizedPresence;
use licoup_native::platform::secure_mesh_secret_store::PlatformSecretStore;

#[test]
fn sealed_presence_types_and_default_platform_dispatch_compile_as_an_ordinary_dependency() {
    fn public_type<T>() {}

    public_type::<SecretStoreConsumedPresence>();
    public_type::<SecretStorePresenceGrant>();
    #[cfg(target_os = "macos")]
    public_type::<MacosAuthorizedPresence>();

    let store = PlatformSecretStore::new("app.licomesh.ui-acceptance", "ordinaryDependency");
    let handle = SecretStoreHandle::new("ordinary-dependency", "fail-closed").unwrap();
    assert!(store.get_secret(&handle).is_err());
}

#[cfg(target_os = "macos")]
#[test]
fn presence_capabilities_cannot_be_forged_outside_the_library() {
    let cases = trybuild::TestCases::new();
    cases.compile_fail("tests/ui/macos_presence_capability_forgery.rs");
}
