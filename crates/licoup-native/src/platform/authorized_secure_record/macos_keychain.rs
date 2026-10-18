use anyhow::{Result, anyhow, ensure};
use core_foundation::base::TCFType;
use core_foundation::boolean::CFBoolean;
use core_foundation::data::CFData;
use core_foundation::dictionary::CFDictionary;
use core_foundation::string::CFString;
use core_foundation_sys::array::{CFArrayGetCount, CFArrayGetTypeID, CFArrayGetValueAtIndex};
use core_foundation_sys::base::{
    CFEqual, CFGetTypeID, CFRelease, CFTypeID, CFTypeRef, kCFAllocatorDefault,
};
use core_foundation_sys::dictionary::{CFDictionaryGetTypeID, CFDictionaryGetValueIfPresent};
use core_foundation_sys::error::CFErrorRef;
use core_foundation_sys::string::CFStringRef;
use security_framework::access_control::{ProtectionMode, SecAccessControl};
use security_framework_sys::access_control::{
    kSecAccessControlUserPresence, kSecAttrAccessibleWhenUnlockedThisDeviceOnly,
};
use security_framework_sys::base::{
    errSecAuthFailed, errSecDuplicateItem, errSecItemNotFound, errSecSuccess,
};
use security_framework_sys::item::{
    kSecAttrAccessControl, kSecAttrAccessGroup, kSecAttrAccount, kSecAttrService,
    kSecAttrSynchronizable, kSecClass, kSecClassGenericPassword, kSecReturnAttributes,
    kSecReturnData, kSecReturnPersistentRef, kSecUseAuthenticationContext,
    kSecUseDataProtectionKeychain, kSecValueData,
};
use security_framework_sys::keychain_item::{SecItemAdd, SecItemCopyMatching};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::ptr;

use super::ledger::{LedgerEntry, locator_digest};
use crate::core::authorized_secure_record::SecureRecordLocator;
use crate::platform::user_presence::UserPresenceSession;

const SERVICE_PREFIX: &str = "land.lico.licoup.authorized-ledger.v1";
const EXPECTED_BUNDLE_IDENTIFIER: &str = "land.lico.licoup";
const PROOF_SCHEMA: &str = "licoup.authorized-secure-ledger-proof.v1";
const ERR_SEC_INTERACTION_NOT_ALLOWED: i32 = -25308;

macro_rules! sec_key {
    ($value:ident) => {{
        // SAFETY: every caller names an SDK-exported process-lifetime CFString.
        unsafe { CFString::wrap_under_get_rule($value) }
    }};
}

macro_rules! sec_value {
    ($value:ident) => {{ sec_key!($value).into_CFType() }};
}

macro_rules! sec_static {
    ($value:ident) => {{
        // SAFETY: reading an SDK-exported immutable static is valid for the
        // process lifetime.
        unsafe { $value }
    }};
}

#[link(name = "Security", kind = "framework")]
unsafe extern "C" {
    static kSecAttrAccessible: CFStringRef;
    static kSecValuePersistentRef: CFStringRef;
    fn SecTaskCreateFromSelf(allocator: *const core::ffi::c_void) -> CFTypeRef;
    fn SecTaskCopyValueForEntitlement(
        task: CFTypeRef,
        entitlement: CFStringRef,
        error: *mut CFErrorRef,
    ) -> CFTypeRef;
}

#[derive(Clone, Debug)]
pub(super) struct LoadedGeneration {
    pub(super) entry: LedgerEntry,
    pub(super) proof_complete: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct PersistentReferenceProof {
    schema_version: String,
    generation: u64,
    entry_digest_sha256: String,
    record_account: String,
    persistent_reference_digest_sha256: String,
}

#[derive(Clone, Debug)]
struct ProtectedItem {
    data: Vec<u8>,
    persistent_reference: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AddDisposition {
    Added,
    Duplicate,
}

pub(super) struct MacosKeychainLedger {
    access_group: String,
}

impl MacosKeychainLedger {
    pub(super) fn new() -> Result<Self> {
        Ok(Self {
            access_group: expected_access_group()?,
        })
    }

    pub(super) fn available() -> bool {
        crate::platform::user_presence::available() && expected_access_group().is_ok()
    }

    pub(super) fn load_generation(
        &self,
        session: &UserPresenceSession,
        locator: &SecureRecordLocator,
        generation: u64,
        repair_incomplete: bool,
    ) -> Result<Option<LoadedGeneration>> {
        let service = service(locator);
        let record_account = record_account(generation);
        let Some(record_item) = self.copy_protected_item(session, &service, &record_account)?
        else {
            ensure!(
                self.copy_protected_item(session, &service, &proof_account(generation))?
                    .is_none(),
                "authorized_secure_record_ledger_orphan_proof"
            );
            return Ok(None);
        };
        let record_json = String::from_utf8(record_item.data.clone())
            .map_err(|_| anyhow!("authorized_secure_record_ledger_entry_invalid"))?;
        let entry = LedgerEntry::from_json(locator, &record_json)?;
        ensure!(
            entry.generation() == generation,
            "authorized_secure_record_ledger_generation_mismatch"
        );
        let proof = PersistentReferenceProof::new(
            &entry,
            &record_account,
            &record_item.persistent_reference,
        );
        let proof_json = serde_json::to_vec(&proof)?;
        let proof_name = proof_account(generation);
        let stored_proof = self.copy_protected_item(session, &service, &proof_name)?;
        let proof_complete = match stored_proof {
            Some(item) => {
                ensure!(
                    item.data == proof_json,
                    "authorized_secure_record_ledger_provenance_mismatch"
                );
                true
            }
            None if repair_incomplete => {
                self.add_or_verify_protected_item(session, &service, &proof_name, &proof_json)?;
                true
            }
            None => false,
        };
        Ok(Some(LoadedGeneration {
            entry,
            proof_complete,
        }))
    }

    pub(super) fn append(
        &self,
        session: &UserPresenceSession,
        locator: &SecureRecordLocator,
        entry: &LedgerEntry,
    ) -> Result<()> {
        entry.validate(locator)?;
        let service = service(locator);
        let account = record_account(entry.generation());
        let entry_json = entry.to_json()?.into_bytes();
        self.add_or_verify_protected_item(session, &service, &account, &entry_json)?;
        let record_item = self
            .copy_protected_item(session, &service, &account)?
            .ok_or_else(|| anyhow!("authorized_secure_record_ledger_append_incomplete"))?;
        let proof =
            PersistentReferenceProof::new(entry, &account, &record_item.persistent_reference);
        self.add_or_verify_protected_item(
            session,
            &service,
            &proof_account(entry.generation()),
            &serde_json::to_vec(&proof)?,
        )
    }

    fn add_or_verify_protected_item(
        &self,
        session: &UserPresenceSession,
        service: &str,
        account: &str,
        data: &[u8],
    ) -> Result<()> {
        match self.add_protected_item(session, service, account, data)? {
            AddDisposition::Added => Ok(()),
            AddDisposition::Duplicate => {
                let existing = self
                    .copy_protected_item(session, service, account)?
                    .ok_or_else(|| anyhow!("authorized_secure_record_ledger_append_race"))?;
                ensure!(
                    existing.data == data,
                    "authorized_secure_record_compare_and_swap_failed"
                );
                Ok(())
            }
        }
    }

    fn add_protected_item(
        &self,
        session: &UserPresenceSession,
        service: &str,
        account: &str,
        data: &[u8],
    ) -> Result<AddDisposition> {
        ensure!(
            !data.is_empty() && data.len() <= 384 * 1024,
            "authorized_secure_record_ledger_item_invalid"
        );
        let effect_lock = session.effect_lock();
        let _effect = effect_lock
            .lock()
            .map_err(|_| anyhow!("authorized_secure_record_user_presence_unavailable"))?;
        let access_control = expected_access_control()?;
        let pairs = vec![
            (sec_key!(kSecClass), sec_value!(kSecClassGenericPassword)),
            (
                sec_key!(kSecAttrService),
                CFString::from(service).into_CFType(),
            ),
            (
                sec_key!(kSecAttrAccount),
                CFString::from(account).into_CFType(),
            ),
            (
                sec_key!(kSecAttrAccessGroup),
                CFString::from(self.access_group.as_str()).into_CFType(),
            ),
            (
                sec_key!(kSecAttrSynchronizable),
                CFBoolean::false_value().into_CFType(),
            ),
            (
                sec_key!(kSecAttrAccessControl),
                access_control.into_CFType(),
            ),
            (sec_key!(kSecUseAuthenticationContext), session.as_cf_type()),
            (
                sec_key!(kSecUseDataProtectionKeychain),
                CFBoolean::true_value().into_CFType(),
            ),
            (
                sec_key!(kSecValueData),
                CFData::from_buffer(data).into_CFType(),
            ),
        ];
        let query = CFDictionary::from_CFType_pairs(&pairs);
        // SAFETY: the query owns all values during the synchronous call and no
        // result pointer is requested.
        let status = unsafe { SecItemAdd(query.as_concrete_TypeRef(), ptr::null_mut()) };
        if status == errSecSuccess {
            Ok(AddDisposition::Added)
        } else if status == errSecDuplicateItem {
            Ok(AddDisposition::Duplicate)
        } else {
            invalidate_failed_authorization(status);
            Err(anyhow!(
                "authorized_secure_record_keychain_add_failed:{status}"
            ))
        }
    }

    fn copy_protected_item(
        &self,
        session: &UserPresenceSession,
        service: &str,
        account: &str,
    ) -> Result<Option<ProtectedItem>> {
        let effect_lock = session.effect_lock();
        let _effect = effect_lock
            .lock()
            .map_err(|_| anyhow!("authorized_secure_record_user_presence_unavailable"))?;
        let pairs = vec![
            (sec_key!(kSecClass), sec_value!(kSecClassGenericPassword)),
            (
                sec_key!(kSecAttrService),
                CFString::from(service).into_CFType(),
            ),
            (
                sec_key!(kSecAttrAccount),
                CFString::from(account).into_CFType(),
            ),
            (
                sec_key!(kSecAttrAccessGroup),
                CFString::from(self.access_group.as_str()).into_CFType(),
            ),
            (
                sec_key!(kSecAttrSynchronizable),
                CFBoolean::false_value().into_CFType(),
            ),
            (sec_key!(kSecUseAuthenticationContext), session.as_cf_type()),
            (
                sec_key!(kSecUseDataProtectionKeychain),
                CFBoolean::true_value().into_CFType(),
            ),
            (
                sec_key!(kSecReturnAttributes),
                CFBoolean::true_value().into_CFType(),
            ),
            (
                sec_key!(kSecReturnData),
                CFBoolean::true_value().into_CFType(),
            ),
            (
                sec_key!(kSecReturnPersistentRef),
                CFBoolean::true_value().into_CFType(),
            ),
        ];
        let query = CFDictionary::from_CFType_pairs(&pairs);
        let mut copied: CFTypeRef = ptr::null();
        // SAFETY: copied is a valid initialized out-pointer and the query lives
        // for the synchronous call.
        let status = unsafe { SecItemCopyMatching(query.as_concrete_TypeRef(), &mut copied) };
        if status == errSecItemNotFound {
            return Ok(None);
        }
        if status != errSecSuccess {
            invalidate_failed_authorization(status);
        }
        ensure!(
            status == errSecSuccess && !copied.is_null(),
            "authorized_secure_record_keychain_read_failed:{status}"
        );
        // SAFETY: successful combined-result queries return a +1 dictionary.
        ensure!(
            unsafe { CFGetTypeID(copied) } == unsafe { CFDictionaryGetTypeID() },
            "authorized_secure_record_keychain_result_invalid"
        );
        let dictionary = unsafe {
            CFDictionary::<*const core::ffi::c_void, *const core::ffi::c_void>::wrap_under_create_rule(
                copied.cast(),
            )
        };
        self.validate_attributes(&dictionary, service, account)?;
        let data = dictionary_data(&dictionary, sec_static!(kSecValueData))?;
        let persistent_reference =
            dictionary_data(&dictionary, sec_static!(kSecValuePersistentRef))?;
        ensure!(
            !persistent_reference.is_empty(),
            "authorized_secure_record_keychain_persistent_reference_missing"
        );
        Ok(Some(ProtectedItem {
            data,
            persistent_reference,
        }))
    }

    fn validate_attributes(
        &self,
        dictionary: &CFDictionary,
        service: &str,
        account: &str,
    ) -> Result<()> {
        ensure!(
            dictionary_string(dictionary, sec_static!(kSecAttrService))? == service
                && dictionary_string(dictionary, sec_static!(kSecAttrAccount))? == account
                && dictionary_string(dictionary, sec_static!(kSecAttrAccessGroup))?
                    == self.access_group
                && dictionary_cf_equal(
                    dictionary,
                    sec_static!(kSecAttrAccessible),
                    sec_static!(kSecAttrAccessibleWhenUnlockedThisDeviceOnly).cast(),
                )?
                && dictionary_cf_equal(
                    dictionary,
                    sec_static!(kSecAttrSynchronizable),
                    CFBoolean::false_value().as_CFTypeRef(),
                )?
                && dictionary_cf_equal(
                    dictionary,
                    sec_static!(kSecAttrAccessControl),
                    expected_access_control()?.as_CFTypeRef(),
                )?,
            "authorized_secure_record_keychain_provenance_invalid"
        );
        Ok(())
    }
}

fn invalidate_failed_authorization(status: i32) {
    if status == errSecAuthFailed || status == ERR_SEC_INTERACTION_NOT_ALLOWED {
        crate::platform::user_presence::invalidate();
    }
}

impl PersistentReferenceProof {
    fn new(entry: &LedgerEntry, record_account: &str, persistent_reference: &[u8]) -> Self {
        Self {
            schema_version: PROOF_SCHEMA.to_owned(),
            generation: entry.generation(),
            entry_digest_sha256: entry.entry_digest_sha256().to_owned(),
            record_account: record_account.to_owned(),
            persistent_reference_digest_sha256: format!(
                "{:x}",
                Sha256::digest(persistent_reference)
            ),
        }
    }
}

fn expected_access_control() -> Result<SecAccessControl> {
    SecAccessControl::create_with_protection(
        Some(ProtectionMode::AccessibleWhenUnlockedThisDeviceOnly),
        kSecAccessControlUserPresence,
    )
    .map_err(|_| anyhow!("authorized_secure_record_keychain_access_control_unavailable"))
}

fn expected_access_group() -> Result<String> {
    let application_identifier = entitlement_string("com.apple.application-identifier")?;
    ensure!(
        application_identifier
            .strip_suffix(EXPECTED_BUNDLE_IDENTIFIER)
            .is_some_and(|prefix| prefix.ends_with('.')),
        "authorized_secure_record_application_identity_invalid"
    );
    ensure!(
        entitlement_string_array("keychain-access-groups")?
            .iter()
            .any(|value| value == &application_identifier),
        "authorized_secure_record_keychain_access_group_unavailable"
    );
    Ok(application_identifier)
}

fn entitlement_value(name: &str) -> Result<CFTypeRef> {
    let entitlement = CFString::from(name);
    // SAFETY: both functions return +1 Core Foundation objects. Error detail is
    // intentionally not exposed because it may contain local signing metadata.
    let task = unsafe { SecTaskCreateFromSelf(kCFAllocatorDefault) };
    ensure!(
        !task.is_null(),
        "authorized_secure_record_application_identity_unavailable"
    );
    let mut error: CFErrorRef = ptr::null_mut();
    let value = unsafe {
        SecTaskCopyValueForEntitlement(task, entitlement.as_concrete_TypeRef(), &mut error)
    };
    unsafe { CFRelease(task) };
    if !error.is_null() {
        unsafe { CFRelease(error.cast()) };
    }
    ensure!(
        !value.is_null(),
        "authorized_secure_record_application_entitlement_missing"
    );
    Ok(value)
}

fn entitlement_string(name: &str) -> Result<String> {
    let value = entitlement_value(name)?;
    ensure!(
        unsafe { CFGetTypeID(value) } == CFString::type_id(),
        "authorized_secure_record_application_entitlement_invalid"
    );
    let value = unsafe { CFString::wrap_under_create_rule(value.cast()) };
    Ok(value.to_string())
}

fn entitlement_string_array(name: &str) -> Result<Vec<String>> {
    let value = entitlement_value(name)?;
    ensure!(
        unsafe { CFGetTypeID(value) } == unsafe { CFArrayGetTypeID() },
        "authorized_secure_record_application_entitlement_invalid"
    );
    let count = unsafe { CFArrayGetCount(value.cast()) };
    let mut result = Vec::with_capacity(count as usize);
    for index in 0..count {
        let item = unsafe { CFArrayGetValueAtIndex(value.cast(), index) } as CFTypeRef;
        ensure!(
            !item.is_null() && unsafe { CFGetTypeID(item) } == CFString::type_id(),
            "authorized_secure_record_application_entitlement_invalid"
        );
        let item = unsafe { CFString::wrap_under_get_rule(item.cast()) };
        result.push(item.to_string());
    }
    unsafe { CFRelease(value) };
    Ok(result)
}

fn service(locator: &SecureRecordLocator) -> String {
    format!("{SERVICE_PREFIX}.{}", locator_digest(locator))
}

fn record_account(generation: u64) -> String {
    format!("generation-{generation:020}.record")
}

fn proof_account(generation: u64) -> String {
    format!("generation-{generation:020}.proof")
}

fn dictionary_value(dictionary: &CFDictionary, key: CFStringRef) -> Result<CFTypeRef> {
    let mut value: *const core::ffi::c_void = ptr::null();
    let found = unsafe {
        CFDictionaryGetValueIfPresent(dictionary.as_concrete_TypeRef(), key.cast(), &mut value)
    };
    ensure!(
        found != 0 && !value.is_null(),
        "authorized_secure_record_keychain_attribute_missing"
    );
    Ok(value.cast_mut().cast())
}

fn dictionary_data(dictionary: &CFDictionary, key: CFStringRef) -> Result<Vec<u8>> {
    let value = dictionary_value(dictionary, key)?;
    ensure!(
        unsafe { CFGetTypeID(value) } == CFData::type_id(),
        "authorized_secure_record_keychain_attribute_invalid"
    );
    let value = unsafe { CFData::wrap_under_get_rule(value.cast()) };
    Ok(value.bytes().to_vec())
}

fn dictionary_string(dictionary: &CFDictionary, key: CFStringRef) -> Result<String> {
    let value = dictionary_value(dictionary, key)?;
    ensure!(
        unsafe { CFGetTypeID(value) } == CFString::type_id(),
        "authorized_secure_record_keychain_attribute_invalid"
    );
    Ok(unsafe { CFString::wrap_under_get_rule(value.cast()) }.to_string())
}

fn dictionary_cf_equal(
    dictionary: &CFDictionary,
    key: CFStringRef,
    expected: CFTypeRef,
) -> Result<bool> {
    let value = dictionary_value(dictionary, key)?;
    Ok(unsafe { CFEqual(value, expected) != 0 })
}

#[allow(dead_code)]
fn _assert_cf_type_id(_: CFTypeID) {}
