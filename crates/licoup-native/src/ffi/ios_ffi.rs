use std::ffi::{CStr, CString, c_char, c_void};
use std::sync::Arc;

use anyhow::{Context, Result, anyhow, ensure};
use serde_json::json;

use crate::core::secure_mesh_secret_store::{MAX_SECRET_BYTES, SecretBytes};
use crate::platform::secure_mesh_secret_store::{SecretStoreHandle, SecureMeshSecretStore};

const IOS_SECRET_STORE_BACKEND: &str = "ios-keychain";
const IOS_SECRET_GET_ERROR: i32 = -1;
const IOS_SECRET_GET_NOT_FOUND: i32 = 0;
const IOS_SECRET_GET_FOUND: i32 = 1;

#[repr(C)]
pub struct LicoSecureMeshSecretStoreCallbacks {
    ctx: *mut c_void,
    backend: *const c_char,
    set_secret: Option<
        unsafe extern "C" fn(
            ctx: *mut c_void,
            namespace: *const c_char,
            key: *const c_char,
            secret: *const u8,
            secret_len: usize,
        ) -> bool,
    >,
    get_secret: Option<
        unsafe extern "C" fn(
            ctx: *mut c_void,
            namespace: *const c_char,
            key: *const c_char,
            value_out: *mut *mut u8,
            value_len_out: *mut usize,
        ) -> i32,
    >,
    delete_secret: Option<
        unsafe extern "C" fn(
            ctx: *mut c_void,
            namespace: *const c_char,
            key: *const c_char,
        ) -> bool,
    >,
    bytes_zeroize_and_free:
        Option<unsafe extern "C" fn(ctx: *mut c_void, value: *mut u8, value_len: usize)>,
}

#[unsafe(no_mangle)]
pub extern "C" fn lico_secure_mesh_runtime_self_test() -> i32 {
    i32::from(crate::ffi::secure_mesh_mobile_ffi::runtime_self_test())
}

#[unsafe(no_mangle)]
pub extern "C" fn lico_secure_mesh_runtime_feature_flags() -> i32 {
    crate::ffi::secure_mesh_mobile_ffi::runtime_feature_flags()
}

#[unsafe(no_mangle)]
pub extern "C" fn lico_secure_mesh_runtime_protocol_hash() -> i32 {
    crate::ffi::secure_mesh_mobile_ffi::runtime_protocol_hash()
}

#[unsafe(no_mangle)]
pub extern "C" fn lico_secure_mesh_json(
    request_json: *const c_char,
    files_dir: *const c_char,
) -> *mut c_char {
    let response = match ios_secure_mesh_json(request_json, files_dir) {
        Ok(value) => value,
        Err(_error) => json!({
            "ok": false,
            "code": "ios_secure_mesh_native_json_failed",
            "error": "Secure Mesh native request failed.",
            "errorDetailRedacted": true,
        }),
    };
    response_to_c_string(response)
}

#[unsafe(no_mangle)]
pub extern "C" fn lico_secure_mesh_json_with_secret_store(
    request_json: *const c_char,
    files_dir: *const c_char,
    callbacks: *const LicoSecureMeshSecretStoreCallbacks,
) -> *mut c_char {
    let response = match ios_secure_mesh_json_with_secret_store(request_json, files_dir, callbacks)
    {
        Ok(value) => value,
        Err(_error) => json!({
            "ok": false,
            "code": "ios_secure_mesh_native_json_with_secret_store_failed",
            "error": "Secure Mesh native request failed.",
            "errorDetailRedacted": true,
        }),
    };
    response_to_c_string(response)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn lico_secure_mesh_string_free(value: *mut c_char) {
    if value.is_null() {
        return;
    }
    unsafe {
        drop(CString::from_raw(value));
    }
}

fn ios_secure_mesh_json(
    request_json: *const c_char,
    files_dir: *const c_char,
) -> Result<serde_json::Value> {
    let request_text = c_string_arg(request_json, "request_json")?;
    let files_dir_text = c_string_arg(files_dir, "files_dir")?;
    crate::ffi::secure_mesh_mobile_ffi::dispatch_json_with_files_dir(
        &request_text,
        &files_dir_text,
        "ios_secure_mesh_native_json_action_unsupported",
    )
}

fn ios_secure_mesh_json_with_secret_store(
    request_json: *const c_char,
    files_dir: *const c_char,
    callbacks: *const LicoSecureMeshSecretStoreCallbacks,
) -> Result<serde_json::Value> {
    let request_text = c_string_arg(request_json, "request_json")?;
    let files_dir_text = c_string_arg(files_dir, "files_dir")?;
    let store: Arc<dyn SecureMeshSecretStore> = Arc::new(IosCallbackSecretStore::new(callbacks)?);
    crate::ffi::secure_mesh_mobile_ffi::dispatch_json_with_files_dir_and_pairwise_secret_store(
        &request_text,
        &files_dir_text,
        "ios_secure_mesh_native_json_action_unsupported",
        store,
    )
}

fn c_string_arg(value: *const c_char, name: &'static str) -> Result<String> {
    if value.is_null() {
        return Err(anyhow!("{name} is null"));
    }
    let c_str = unsafe { CStr::from_ptr(value) };
    Ok(c_str.to_str()?.to_owned())
}

fn response_to_c_string(value: serde_json::Value) -> *mut c_char {
    let serialized = serde_json::to_string(&value).unwrap_or_else(|_error| {
        r#"{"ok":false,"code":"ios_secure_mesh_json_serialize_failed","error":"Secure Mesh response serialization failed.","errorDetailRedacted":true}"#.to_string()
    });
    c_string_lossy(serialized).into_raw()
}

fn c_string_lossy(value: String) -> CString {
    match CString::new(value) {
        Ok(c_string) => c_string,
        Err(error) => {
            let sanitized: Vec<u8> = error
                .into_vec()
                .into_iter()
                .filter(|byte| *byte != 0)
                .collect();
            CString::new(sanitized).unwrap_or_else(|_| {
                CString::new(r#"{"ok":false,"code":"ios_secure_mesh_json_cstring_failed"}"#)
                    .expect("static JSON contains no nul bytes")
            })
        }
    }
}

struct IosCallbackSecretStore {
    ctx: *mut c_void,
    set_secret: unsafe extern "C" fn(
        ctx: *mut c_void,
        namespace: *const c_char,
        key: *const c_char,
        secret: *const u8,
        secret_len: usize,
    ) -> bool,
    get_secret: unsafe extern "C" fn(
        ctx: *mut c_void,
        namespace: *const c_char,
        key: *const c_char,
        value_out: *mut *mut u8,
        value_len_out: *mut usize,
    ) -> i32,
    delete_secret: unsafe extern "C" fn(
        ctx: *mut c_void,
        namespace: *const c_char,
        key: *const c_char,
    ) -> bool,
    bytes_zeroize_and_free:
        unsafe extern "C" fn(ctx: *mut c_void, value: *mut u8, value_len: usize),
}

// The callback table is used synchronously during a single nativeJson call. The
// opaque Swift context owns the actual Keychain operations and no pointer is
// retained after dispatch returns.
unsafe impl Send for IosCallbackSecretStore {}
unsafe impl Sync for IosCallbackSecretStore {}

impl IosCallbackSecretStore {
    fn new(callbacks: *const LicoSecureMeshSecretStoreCallbacks) -> Result<Self> {
        ensure!(
            !callbacks.is_null(),
            "ios secret-store callback table is null"
        );
        let callbacks = unsafe { &*callbacks };
        let backend = c_string_arg(callbacks.backend, "ios_secret_store_backend")?;
        ensure!(
            backend.trim() == IOS_SECRET_STORE_BACKEND,
            "ios secret-store backend is unsupported"
        );
        Ok(Self {
            ctx: callbacks.ctx,
            set_secret: callbacks
                .set_secret
                .ok_or_else(|| anyhow!("ios secret-store set callback is missing"))?,
            get_secret: callbacks
                .get_secret
                .ok_or_else(|| anyhow!("ios secret-store get callback is missing"))?,
            delete_secret: callbacks
                .delete_secret
                .ok_or_else(|| anyhow!("ios secret-store delete callback is missing"))?,
            bytes_zeroize_and_free: callbacks
                .bytes_zeroize_and_free
                .ok_or_else(|| anyhow!("ios secret-store bytes cleanup callback is missing"))?,
        })
    }

    fn c_handle_args(handle: &SecretStoreHandle) -> Result<(CString, CString)> {
        Ok((
            CString::new(handle.namespace()).context("ios secret-store namespace is invalid")?,
            CString::new(handle.key()).context("ios secret-store key is invalid")?,
        ))
    }
}

impl SecureMeshSecretStore for IosCallbackSecretStore {
    fn backend(&self) -> &'static str {
        IOS_SECRET_STORE_BACKEND
    }

    fn supported(&self) -> bool {
        // A callback table proves only that the Swift bridge is wired. It does
        // not prove that Keychain user-presence access control was created or
        // evaluated on this device. Until the bridge supplies measured,
        // authenticated capability facts, selection must remain memory-only.
        false
    }

    fn set_secret(&self, handle: &SecretStoreHandle, secret: SecretBytes) -> Result<()> {
        let (namespace, key) = Self::c_handle_args(handle)?;
        let ok = unsafe {
            (self.set_secret)(
                self.ctx,
                namespace.as_ptr(),
                key.as_ptr(),
                secret.expose_bytes().as_ptr(),
                secret.expose_bytes().len(),
            )
        };
        ensure!(ok, "ios secret-store write failed for {}", handle.key());
        Ok(())
    }

    fn get_secret(&self, handle: &SecretStoreHandle) -> Result<Option<SecretBytes>> {
        let (namespace, key) = Self::c_handle_args(handle)?;
        let mut value = std::ptr::null_mut();
        let mut value_len = 0_usize;
        let status = unsafe {
            (self.get_secret)(
                self.ctx,
                namespace.as_ptr(),
                key.as_ptr(),
                &mut value,
                &mut value_len,
            )
        };
        match status {
            IOS_SECRET_GET_FOUND => {
                ensure!(
                    !value.is_null(),
                    "ios secret-store reported found without a value for {}",
                    handle.key()
                );
                ensure!(
                    value_len > 0 && value_len <= MAX_SECRET_BYTES,
                    "ios secret-store returned an invalid value length"
                );
                // SAFETY: the callback reports a live allocation of value_len
                // bytes and retains ownership until the cleanup callback.
                let bytes = unsafe { std::slice::from_raw_parts(value, value_len) }.to_vec();
                unsafe {
                    (self.bytes_zeroize_and_free)(self.ctx, value, value_len);
                }
                SecretBytes::try_from_bytes(bytes)
                    .map(Some)
                    .map_err(|_| anyhow!("ios secret-store returned invalid secret bytes"))
            }
            IOS_SECRET_GET_NOT_FOUND => {
                if value.is_null() && value_len == 0 {
                    Ok(None)
                } else {
                    unsafe {
                        (self.bytes_zeroize_and_free)(self.ctx, value, value_len);
                    }
                    Err(anyhow!(
                        "ios secret-store reported not-found with an unexpected value for {}",
                        handle.key()
                    ))
                }
            }
            IOS_SECRET_GET_ERROR => {
                if !value.is_null() {
                    unsafe {
                        (self.bytes_zeroize_and_free)(self.ctx, value, value_len);
                    }
                }
                Err(anyhow!("ios secret-store read failed for {}", handle.key()))
            }
            unexpected => {
                if !value.is_null() {
                    unsafe {
                        (self.bytes_zeroize_and_free)(self.ctx, value, value_len);
                    }
                }
                Err(anyhow!(
                    "ios secret-store returned unknown read status {unexpected} for {}",
                    handle.key()
                ))
            }
        }
    }

    fn delete_secret(&self, handle: &SecretStoreHandle) -> Result<()> {
        let (namespace, key) = Self::c_handle_args(handle)?;
        let ok = unsafe { (self.delete_secret)(self.ctx, namespace.as_ptr(), key.as_ptr()) };
        ensure!(ok, "ios secret-store delete failed for {}", handle.key());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};

    static IOS_TEST_SECRETS: OnceLock<Mutex<HashMap<String, Vec<u8>>>> = OnceLock::new();

    fn test_store() -> &'static Mutex<HashMap<String, Vec<u8>>> {
        IOS_TEST_SECRETS.get_or_init(|| Mutex::new(HashMap::new()))
    }

    unsafe extern "C" fn test_set_secret(
        _ctx: *mut c_void,
        namespace: *const c_char,
        key: *const c_char,
        secret: *const u8,
        secret_len: usize,
    ) -> bool {
        let namespace = unsafe { CStr::from_ptr(namespace) }
            .to_string_lossy()
            .to_string();
        let key = unsafe { CStr::from_ptr(key) }.to_string_lossy().to_string();
        let secret = unsafe { std::slice::from_raw_parts(secret, secret_len) }.to_vec();
        test_store()
            .lock()
            .map(|mut store| store.insert(format!("{namespace}:{key}"), secret))
            .is_ok()
    }

    unsafe extern "C" fn test_get_secret(
        _ctx: *mut c_void,
        namespace: *const c_char,
        key: *const c_char,
        value_out: *mut *mut u8,
        value_len_out: *mut usize,
    ) -> i32 {
        if value_out.is_null() || value_len_out.is_null() {
            return IOS_SECRET_GET_ERROR;
        }
        unsafe {
            *value_out = std::ptr::null_mut();
            *value_len_out = 0;
        }
        let namespace = unsafe { CStr::from_ptr(namespace) }
            .to_string_lossy()
            .to_string();
        let key = unsafe { CStr::from_ptr(key) }.to_string_lossy().to_string();
        let Some(secret) = test_store()
            .lock()
            .ok()
            .and_then(|store| store.get(&format!("{namespace}:{key}")).cloned())
        else {
            return IOS_SECRET_GET_NOT_FOUND;
        };
        let mut secret = secret.into_boxed_slice();
        unsafe {
            *value_len_out = secret.len();
            *value_out = secret.as_mut_ptr();
        }
        std::mem::forget(secret);
        IOS_SECRET_GET_FOUND
    }

    unsafe extern "C" fn test_get_secret_error(
        _ctx: *mut c_void,
        _namespace: *const c_char,
        _key: *const c_char,
        value_out: *mut *mut u8,
        value_len_out: *mut usize,
    ) -> i32 {
        if !value_out.is_null() {
            unsafe {
                *value_out = std::ptr::null_mut();
            }
        }
        if !value_len_out.is_null() {
            unsafe {
                *value_len_out = 0;
            }
        }
        IOS_SECRET_GET_ERROR
    }

    unsafe extern "C" fn test_delete_secret(
        _ctx: *mut c_void,
        namespace: *const c_char,
        key: *const c_char,
    ) -> bool {
        let namespace = unsafe { CStr::from_ptr(namespace) }
            .to_string_lossy()
            .to_string();
        let key = unsafe { CStr::from_ptr(key) }.to_string_lossy().to_string();
        test_store()
            .lock()
            .map(|mut store| {
                store.remove(&format!("{namespace}:{key}"));
            })
            .is_ok()
    }

    unsafe extern "C" fn test_bytes_zeroize_and_free(
        _ctx: *mut c_void,
        value: *mut u8,
        value_len: usize,
    ) {
        if !value.is_null() {
            unsafe {
                let mut value = Box::from_raw(std::ptr::slice_from_raw_parts_mut(value, value_len));
                value.fill(0);
            }
        }
    }

    fn callback_table(backend: &CString) -> LicoSecureMeshSecretStoreCallbacks {
        LicoSecureMeshSecretStoreCallbacks {
            ctx: std::ptr::null_mut(),
            backend: backend.as_ptr(),
            set_secret: Some(test_set_secret),
            get_secret: Some(test_get_secret),
            delete_secret: Some(test_delete_secret),
            bytes_zeroize_and_free: Some(test_bytes_zeroize_and_free),
        }
    }

    fn callback_table_with_get(
        backend: &CString,
        get_secret: unsafe extern "C" fn(
            *mut c_void,
            *const c_char,
            *const c_char,
            *mut *mut u8,
            *mut usize,
        ) -> i32,
    ) -> LicoSecureMeshSecretStoreCallbacks {
        let mut callbacks = callback_table(backend);
        callbacks.get_secret = Some(get_secret);
        callbacks
    }

    #[test]
    fn ios_callback_secret_store_round_trips_opaque_handles() {
        test_store().lock().unwrap().clear();
        let backend = CString::new(IOS_SECRET_STORE_BACKEND).unwrap();
        let callbacks = callback_table(&backend);
        let store = IosCallbackSecretStore::new(&callbacks).unwrap();
        assert!(!store.supported());
        assert!(store.capability_facts().unwrap().is_empty());
        let handle =
            SecretStoreHandle::new("mobileRelayE2ee:mobileRelayRuntime", "privateKeyBase64url")
                .unwrap();

        store
            .set_secret(
                &handle,
                SecretBytes::try_from_bytes(b"ios-callback-secret-store-canary".to_vec()).unwrap(),
            )
            .unwrap();
        assert_eq!(
            store
                .get_secret(&handle)
                .unwrap()
                .as_ref()
                .map(SecretBytes::expose_bytes),
            Some(b"ios-callback-secret-store-canary".as_slice())
        );
        assert!(
            test_store()
                .lock()
                .unwrap()
                .contains_key("mobileRelayE2ee:mobileRelayRuntime:privateKeyBase64url")
        );
        store.delete_secret(&handle).unwrap();
        assert!(store.get_secret(&handle).unwrap().is_none());
    }

    #[test]
    fn ios_callback_secret_store_propagates_read_errors() {
        let backend = CString::new(IOS_SECRET_STORE_BACKEND).unwrap();
        let callbacks = callback_table_with_get(&backend, test_get_secret_error);
        let store = IosCallbackSecretStore::new(&callbacks).unwrap();
        let handle =
            SecretStoreHandle::new("mobileRelayE2ee:mobileRelayRuntime", "privateKeyBase64url")
                .unwrap();

        let error = store.get_secret(&handle).unwrap_err();
        assert!(error.to_string().contains("ios secret-store read failed"));
    }
}
