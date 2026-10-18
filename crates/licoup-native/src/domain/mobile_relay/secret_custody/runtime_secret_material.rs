use std::collections::BTreeMap;
use std::fmt;
#[cfg(test)]
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};

use crate::core::secure_mesh_secret_store::SecretBytes;
#[cfg(test)]
use crate::core::secure_mesh_secret_store::SecretZeroizeProbe;

pub const MOBILE_RELAY_SECRET_BUNDLE_MAGIC: [u8; 4] = *b"LRSB";
pub const MOBILE_RELAY_SECRET_BUNDLE_VERSION: u8 = 1;
pub const MOBILE_RELAY_SECRET_FIELD_MAX_BYTES: usize = 64 * 1024;
pub const MOBILE_RELAY_SECRET_BUNDLE_MAX_BYTES: usize = 512 * 1024;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum MobileRelayE2eeSecretField {
    PrivateKey,
    SigningKey,
    SignedPrekeyPrivateKey,
    OneTimePrekeyPrivateKey,
    OneTimeMlKem1024PrekeySeed,
    PairingSecret,
}

impl MobileRelayE2eeSecretField {
    pub const ALL: [Self; 6] = [
        Self::PrivateKey,
        Self::SigningKey,
        Self::SignedPrekeyPrivateKey,
        Self::OneTimePrekeyPrivateKey,
        Self::OneTimeMlKem1024PrekeySeed,
        Self::PairingSecret,
    ];

    pub const fn wire_tag(self) -> u8 {
        match self {
            Self::PrivateKey => 1,
            Self::SigningKey => 2,
            Self::SignedPrekeyPrivateKey => 3,
            Self::OneTimePrekeyPrivateKey => 4,
            Self::OneTimeMlKem1024PrekeySeed => 5,
            Self::PairingSecret => 6,
        }
    }

    pub const fn config_field(self) -> &'static str {
        match self {
            Self::PrivateKey => "privateKeyBase64url",
            Self::SigningKey => "signingKeyBase64url",
            Self::SignedPrekeyPrivateKey => "signedPrekeyPrivateKeyBase64url",
            Self::OneTimePrekeyPrivateKey => "oneTimePrekeyPrivateKeyBase64url",
            Self::OneTimeMlKem1024PrekeySeed => "oneTimeMlKem1024PrekeySeedBase64url",
            Self::PairingSecret => "pairingSecretBase64url",
        }
    }

    fn from_wire_tag(tag: u8) -> Option<Self> {
        Self::ALL.into_iter().find(|field| field.wire_tag() == tag)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MobileRelaySecretMaterialError {
    DuplicateField,
    FieldOversize,
    BundleOversize,
    Malformed,
    UnknownField,
    Truncated,
    EmptyBundle,
}

impl fmt::Display for MobileRelaySecretMaterialError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::DuplicateField => "mobile_relay_secret_bundle_duplicate_field",
            Self::FieldOversize => "mobile_relay_secret_bundle_field_oversize",
            Self::BundleOversize => "mobile_relay_secret_bundle_oversize",
            Self::Malformed => "mobile_relay_secret_bundle_malformed",
            Self::UnknownField => "mobile_relay_secret_bundle_unknown_field",
            Self::Truncated => "mobile_relay_secret_bundle_truncated",
            Self::EmptyBundle => "mobile_relay_secret_bundle_empty",
        })
    }
}

impl std::error::Error for MobileRelaySecretMaterialError {}

pub struct MobileRelayE2eeSecretBundle {
    fields: BTreeMap<MobileRelayE2eeSecretField, SecretBytes>,
}

impl fmt::Debug for MobileRelayE2eeSecretBundle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("MobileRelayE2eeSecretBundle([redacted])")
    }
}

impl MobileRelayE2eeSecretBundle {
    pub fn try_from_fields(
        fields: Vec<(MobileRelayE2eeSecretField, SecretBytes)>,
    ) -> Result<Self, MobileRelaySecretMaterialError> {
        if fields.is_empty() {
            return Err(MobileRelaySecretMaterialError::EmptyBundle);
        }
        let mut owned = BTreeMap::new();
        for (field, secret) in fields {
            if secret.expose_bytes().len() > MOBILE_RELAY_SECRET_FIELD_MAX_BYTES {
                return Err(MobileRelaySecretMaterialError::FieldOversize);
            }
            if owned.insert(field, secret).is_some() {
                return Err(MobileRelaySecretMaterialError::DuplicateField);
            }
        }
        Ok(Self { fields: owned })
    }

    pub fn secret(&self, field: MobileRelayE2eeSecretField) -> Option<&SecretBytes> {
        self.fields.get(&field)
    }

    pub fn merge_replacing(
        mut self,
        incoming: Self,
    ) -> Result<Self, MobileRelaySecretMaterialError> {
        for (field, secret) in incoming.fields {
            self.fields.insert(field, secret);
        }
        Ok(self)
    }

    fn into_fields(self) -> BTreeMap<MobileRelayE2eeSecretField, SecretBytes> {
        self.fields
    }
}

pub struct RuntimeSecretMaterial {
    e2ee: BTreeMap<MobileRelayE2eeSecretField, SecretBytes>,
    pc_token: Option<SecretBytes>,
    mobile_token: Option<SecretBytes>,
    paired_device_tokens: BTreeMap<String, SecretBytes>,
    // Ephemeral evidence from authorized local custody, never transported.
    local_custody_namespace: Option<String>,
}

impl Default for RuntimeSecretMaterial {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for RuntimeSecretMaterial {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RuntimeSecretMaterial([redacted])")
    }
}

impl RuntimeSecretMaterial {
    pub fn new() -> Self {
        Self {
            e2ee: BTreeMap::new(),
            pc_token: None,
            mobile_token: None,
            paired_device_tokens: BTreeMap::new(),
            local_custody_namespace: None,
        }
    }

    pub fn e2ee_secret(&self, field: MobileRelayE2eeSecretField) -> Option<&SecretBytes> {
        self.e2ee.get(&field)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.e2ee.is_empty()
            && self.pc_token.is_none()
            && self.mobile_token.is_none()
            && self.paired_device_tokens.is_empty()
    }

    pub(in crate::domain::mobile_relay) fn local_custody_namespace(&self) -> Option<&str> {
        self.local_custody_namespace.as_deref()
    }

    pub(in crate::domain::mobile_relay) fn mark_local_custody(&mut self, namespace: &str) {
        self.local_custody_namespace = Some(namespace.to_owned());
    }

    pub fn insert_e2ee_secret(
        &mut self,
        field: MobileRelayE2eeSecretField,
        secret: SecretBytes,
    ) -> Result<(), MobileRelaySecretMaterialError> {
        if secret.expose_bytes().len() > MOBILE_RELAY_SECRET_FIELD_MAX_BYTES {
            return Err(MobileRelaySecretMaterialError::FieldOversize);
        }
        if self.e2ee.contains_key(&field) {
            return Err(MobileRelaySecretMaterialError::DuplicateField);
        }
        if matches!(
            field,
            MobileRelayE2eeSecretField::PrivateKey | MobileRelayE2eeSecretField::SigningKey
        ) {
            self.local_custody_namespace = None;
        }
        self.e2ee.insert(field, secret);
        Ok(())
    }

    pub fn replace_e2ee_secret(
        &mut self,
        field: MobileRelayE2eeSecretField,
        secret: SecretBytes,
    ) -> Result<(), MobileRelaySecretMaterialError> {
        if secret.expose_bytes().len() > MOBILE_RELAY_SECRET_FIELD_MAX_BYTES {
            return Err(MobileRelaySecretMaterialError::FieldOversize);
        }
        if matches!(
            field,
            MobileRelayE2eeSecretField::PrivateKey | MobileRelayE2eeSecretField::SigningKey
        ) {
            self.local_custody_namespace = None;
        }
        self.e2ee.insert(field, secret);
        Ok(())
    }

    pub fn remove_e2ee_secret(&mut self, field: MobileRelayE2eeSecretField) {
        if matches!(
            field,
            MobileRelayE2eeSecretField::PrivateKey | MobileRelayE2eeSecretField::SigningKey
        ) {
            self.local_custody_namespace = None;
        }
        self.e2ee.remove(&field);
    }

    pub fn merge_e2ee_bundle(&mut self, bundle: MobileRelayE2eeSecretBundle) {
        for (field, secret) in bundle.into_fields() {
            if matches!(
                field,
                MobileRelayE2eeSecretField::PrivateKey | MobileRelayE2eeSecretField::SigningKey
            ) {
                self.local_custody_namespace = None;
            }
            self.e2ee.insert(field, secret);
        }
    }

    pub fn take_e2ee_bundle(&mut self) -> Option<MobileRelayE2eeSecretBundle> {
        if self.e2ee.is_empty() {
            return None;
        }
        self.local_custody_namespace = None;
        Some(MobileRelayE2eeSecretBundle {
            fields: std::mem::take(&mut self.e2ee),
        })
    }

    pub fn set_token(&mut self, field: &str, secret: SecretBytes) {
        match field {
            "pcToken" => self.pc_token = Some(secret),
            "mobileToken" => self.mobile_token = Some(secret),
            _ => {}
        }
    }

    pub fn set_paired_device_token(&mut self, key: String, secret: SecretBytes) {
        self.paired_device_tokens.insert(key, secret);
    }

    #[cfg(test)]
    pub fn attach_test_zeroize_probe(
        &mut self,
        field: MobileRelayE2eeSecretField,
        probe: SecretZeroizeProbe,
    ) -> Result<(), MobileRelaySecretMaterialError> {
        let secret = self
            .e2ee
            .get_mut(&field)
            .ok_or(MobileRelaySecretMaterialError::Malformed)?;
        secret.attach_test_zeroize_probe(probe);
        Ok(())
    }
}

#[cfg(test)]
pub(crate) fn test_runtime_secret_material(
    variable: &str,
) -> MutexGuard<'static, RuntimeSecretMaterial> {
    static MATERIALS: OnceLock<Mutex<BTreeMap<String, &'static Mutex<RuntimeSecretMaterial>>>> =
        OnceLock::new();

    let current_thread = std::thread::current();
    let test_name = current_thread.name().unwrap_or("unnamed-test");
    let variable = variable.trim().trim_start_matches('&').trim();
    let variable = variable.strip_prefix("mut ").unwrap_or(variable).trim();
    let key = format!("{test_name}:{variable}");
    let material = {
        let mut materials = MATERIALS
            .get_or_init(|| Mutex::new(BTreeMap::new()))
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        *materials
            .entry(key)
            .or_insert_with(|| Box::leak(Box::new(Mutex::new(RuntimeSecretMaterial::new()))))
    };
    material.lock().unwrap_or_else(PoisonError::into_inner)
}

pub fn encode_mobile_relay_e2ee_secret_bundle(
    bundle: MobileRelayE2eeSecretBundle,
) -> Result<SecretBytes, MobileRelaySecretMaterialError> {
    let fields = bundle.into_fields();
    let mut encoded = Vec::with_capacity(
        MOBILE_RELAY_SECRET_BUNDLE_MAGIC.len()
            + 2
            + fields
                .values()
                .map(|secret| 5 + secret.expose_bytes().len())
                .sum::<usize>(),
    );
    encoded.extend_from_slice(&MOBILE_RELAY_SECRET_BUNDLE_MAGIC);
    encoded.push(MOBILE_RELAY_SECRET_BUNDLE_VERSION);
    encoded.push(
        u8::try_from(fields.len()).map_err(|_| MobileRelaySecretMaterialError::BundleOversize)?,
    );
    for (field, secret) in fields {
        encoded.push(field.wire_tag());
        encoded.extend_from_slice(
            &u32::try_from(secret.expose_bytes().len())
                .map_err(|_| MobileRelaySecretMaterialError::FieldOversize)?
                .to_be_bytes(),
        );
        encoded.extend_from_slice(secret.expose_bytes());
    }
    if encoded.len() > MOBILE_RELAY_SECRET_BUNDLE_MAX_BYTES {
        return Err(MobileRelaySecretMaterialError::BundleOversize);
    }
    SecretBytes::try_from_bytes(encoded).map_err(|_| MobileRelaySecretMaterialError::BundleOversize)
}

pub fn decode_mobile_relay_e2ee_secret_bundle(
    encoded: SecretBytes,
) -> Result<MobileRelayE2eeSecretBundle, MobileRelaySecretMaterialError> {
    let bytes = encoded.expose_bytes();
    if bytes.len() > MOBILE_RELAY_SECRET_BUNDLE_MAX_BYTES {
        return Err(MobileRelaySecretMaterialError::BundleOversize);
    }
    if bytes.len() < 6
        || bytes[..4] != MOBILE_RELAY_SECRET_BUNDLE_MAGIC
        || bytes[4] != MOBILE_RELAY_SECRET_BUNDLE_VERSION
    {
        return Err(MobileRelaySecretMaterialError::Malformed);
    }
    let count = usize::from(bytes[5]);
    let mut cursor = 6;
    let mut fields = Vec::with_capacity(count);
    for _ in 0..count {
        if bytes.len().saturating_sub(cursor) < 5 {
            return Err(MobileRelaySecretMaterialError::Truncated);
        }
        let field = MobileRelayE2eeSecretField::from_wire_tag(bytes[cursor])
            .ok_or(MobileRelaySecretMaterialError::UnknownField)?;
        cursor += 1;
        let length = u32::from_be_bytes(
            bytes[cursor..cursor + 4]
                .try_into()
                .map_err(|_| MobileRelaySecretMaterialError::Truncated)?,
        ) as usize;
        cursor += 4;
        if length > MOBILE_RELAY_SECRET_FIELD_MAX_BYTES {
            return Err(MobileRelaySecretMaterialError::FieldOversize);
        }
        if bytes.len().saturating_sub(cursor) < length {
            return Err(MobileRelaySecretMaterialError::Truncated);
        }
        let secret = SecretBytes::try_from_bytes(bytes[cursor..cursor + length].to_vec())
            .map_err(|_| MobileRelaySecretMaterialError::Malformed)?;
        fields.push((field, secret));
        cursor += length;
    }
    if cursor != bytes.len() {
        return Err(MobileRelaySecretMaterialError::Malformed);
    }
    MobileRelayE2eeSecretBundle::try_from_fields(fields)
}
