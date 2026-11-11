use std::marker::PhantomData;

use super::*;
use zeroize::Zeroize;

const FIRST_CANARY: &[u8] = b"synthetic-zeroize-canary-alpha";
const SECOND_CANARY: &[u8] = b"synthetic-zeroize-canary-beta";

struct TraitProbe<T: ?Sized>(PhantomData<T>);

trait AmbiguousIfClone<A> {
    fn marker() {}
}
impl<T: ?Sized> AmbiguousIfClone<()> for TraitProbe<T> {}
impl<T: Clone> AmbiguousIfClone<u8> for TraitProbe<T> {}

trait AmbiguousIfCopy<A> {
    fn marker() {}
}
impl<T: ?Sized> AmbiguousIfCopy<()> for TraitProbe<T> {}
impl<T: Copy> AmbiguousIfCopy<u8> for TraitProbe<T> {}

trait AmbiguousIfDefault<A> {
    fn marker() {}
}
impl<T: ?Sized> AmbiguousIfDefault<()> for TraitProbe<T> {}
impl<T: Default> AmbiguousIfDefault<u8> for TraitProbe<T> {}

trait AmbiguousIfSerialize<A> {
    fn marker() {}
}
impl<T: ?Sized> AmbiguousIfSerialize<()> for TraitProbe<T> {}
impl<T: ?Sized + serde::Serialize> AmbiguousIfSerialize<u8> for TraitProbe<T> {}

trait AmbiguousIfDeserialize<A> {
    fn marker() {}
}
impl<T: ?Sized> AmbiguousIfDeserialize<()> for TraitProbe<T> {}
impl<T: serde::de::DeserializeOwned> AmbiguousIfDeserialize<u8> for TraitProbe<T> {}

#[test]
fn secret_bytes_is_bounded_single_owner_non_serde_and_explicitly_exposed() {
    let owned = FIRST_CANARY.to_vec();
    let allocation = owned.as_ptr();
    let secret = SecretBytes::try_from_bytes(owned).unwrap();

    assert_eq!(secret.expose_bytes().as_ptr(), allocation);
    assert_eq!(secret.expose_bytes(), FIRST_CANARY);
    assert_eq!(secret.expose_utf8().unwrap().as_bytes(), FIRST_CANARY);

    let debug = format!("{secret:?}");
    assert!(debug.contains("redacted"));
    assert!(!debug.contains(std::str::from_utf8(FIRST_CANARY).unwrap()));
    assert!(!debug.contains(std::str::from_utf8(SECOND_CANARY).unwrap()));

    let _ = <TraitProbe<SecretBytes> as AmbiguousIfClone<_>>::marker;
    let _ = <TraitProbe<SecretBytes> as AmbiguousIfCopy<_>>::marker;
    let _ = <TraitProbe<SecretBytes> as AmbiguousIfDefault<_>>::marker;
    let _ = <TraitProbe<SecretBytes> as AmbiguousIfSerialize<_>>::marker;
    let _ = <TraitProbe<SecretBytes> as AmbiguousIfDeserialize<_>>::marker;
}

#[test]
fn secret_bytes_rejects_empty_oversize_and_invalid_utf8_without_echoing_material() {
    assert_eq!(
        SecretBytes::try_from_bytes(Vec::new())
            .unwrap_err()
            .to_string(),
        "secure_mesh_secret_empty"
    );

    let mut oversize = vec![b'x'; MAX_SECRET_BYTES + 1];
    oversize[..FIRST_CANARY.len()].copy_from_slice(FIRST_CANARY);
    let oversize_error = SecretBytes::try_from_bytes(oversize)
        .unwrap_err()
        .to_string();
    assert_eq!(oversize_error, "secure_mesh_secret_oversize");
    assert!(!oversize_error.contains(std::str::from_utf8(FIRST_CANARY).unwrap()));

    let invalid = SecretBytes::try_from_bytes(vec![0xff, 0xfe, 0xfd]).unwrap();
    assert_eq!(
        invalid.expose_utf8().unwrap_err().to_string(),
        "secure_mesh_secret_not_utf8"
    );
}

#[test]
fn explicit_zeroize_and_drop_wipe_the_owned_allocation_before_release() {
    let explicit_probe = SecretZeroizeProbe::new();
    let explicit_len = FIRST_CANARY.len();
    let mut explicit = SecretBytes::try_from_bytes_with_test_zeroize_probe(
        FIRST_CANARY.to_vec(),
        explicit_probe.clone(),
    )
    .unwrap();
    explicit.zeroize();
    assert!(explicit.expose_bytes().is_empty());
    assert_eq!(
        explicit_probe.observations(),
        vec![vec![0; explicit_len]],
        "the test-only observer must see the backing bytes after the real wipe and before release"
    );

    let drop_probe = SecretZeroizeProbe::new();
    let drop_len = SECOND_CANARY.len();
    {
        let dropped = SecretBytes::try_from_bytes_with_test_zeroize_probe(
            SECOND_CANARY.to_vec(),
            drop_probe.clone(),
        )
        .unwrap();
        assert_eq!(dropped.expose_bytes(), SECOND_CANARY);
    }
    assert_eq!(
        drop_probe.observations(),
        vec![vec![0; drop_len]],
        "Drop must exercise the same observable wipe path without inspecting freed memory"
    );
}
