use chacha20poly1305::{
    ChaCha20Poly1305, Key, KeyInit, Nonce,
    aead::{Aead, Payload},
};
use hkdf::Hkdf;
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use super::store::{ContentKind, ManifestEntry, ObjectId};

const OBJECT_KEY_SALT: &[u8] = b"licoup.history-backup.object-key.v1";
const OBJECT_AAD_DOMAIN: &[u8] = b"licoup.history-backup.object-aad.v1";

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum StorageMode {
    #[default]
    ProviderManaged,
    ClientEncrypted {
        key_generation: u64,
    },
}

pub trait HistoryKeySource: Sync {
    fn key_for_generation(&self, generation: u64) -> Option<Zeroizing<[u8; 32]>>;
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredObjectHeader {
    pub format_version: u16,
    pub object_id: ObjectId,
    pub content_kind: ContentKind,
    pub storage_mode: StorageMode,
    pub canonical_length: u64,
    pub canonical_digest: [u8; 32],
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredHistoryObject {
    pub header: StoredObjectHeader,
    pub nonce: Option<[u8; 12]>,
    pub body: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObjectOpenError {
    HeaderMismatch,
    MissingKey(u64),
    AuthenticationFailed,
    ContentCorrupt,
}

pub fn seal_history_object(
    entry: &ManifestEntry,
    canonical_bytes: &[u8],
    keys: &dyn HistoryKeySource,
) -> Result<StoredHistoryObject, ObjectOpenError> {
    if canonical_bytes.len() as u64 != entry.canonical_length
        || digest(canonical_bytes) != entry.canonical_digest
    {
        return Err(ObjectOpenError::ContentCorrupt);
    }
    let header = StoredObjectHeader {
        format_version: 1,
        object_id: entry.object_id.clone(),
        content_kind: entry.content_kind,
        storage_mode: entry.storage_mode,
        canonical_length: entry.canonical_length,
        canonical_digest: entry.canonical_digest,
    };
    match entry.storage_mode {
        StorageMode::ProviderManaged => Ok(StoredHistoryObject {
            header,
            nonce: None,
            body: canonical_bytes.to_vec(),
        }),
        StorageMode::ClientEncrypted { key_generation } => {
            let key = keys
                .key_for_generation(key_generation)
                .ok_or(ObjectOpenError::MissingKey(key_generation))?;
            let mut nonce = [0_u8; 12];
            OsRng.fill_bytes(&mut nonce);
            let derived = derive_object_key(&key, &entry.object_id, key_generation);
            let aad = header_aad(&header);
            let body = ChaCha20Poly1305::new(Key::from_slice(&derived[..]))
                .encrypt(
                    Nonce::from_slice(&nonce),
                    Payload {
                        msg: canonical_bytes,
                        aad: &aad,
                    },
                )
                .map_err(|_| ObjectOpenError::AuthenticationFailed)?;
            Ok(StoredHistoryObject {
                header,
                nonce: Some(nonce),
                body,
            })
        }
    }
}

pub fn open_history_object(
    entry: &ManifestEntry,
    object: &StoredHistoryObject,
    keys: &dyn HistoryKeySource,
) -> Result<Vec<u8>, ObjectOpenError> {
    validate_stored_object_header(entry, object)?;
    let canonical = match entry.storage_mode {
        StorageMode::ProviderManaged => object.body.clone(),
        StorageMode::ClientEncrypted { key_generation } => {
            let key = keys
                .key_for_generation(key_generation)
                .ok_or(ObjectOpenError::MissingKey(key_generation))?;
            let nonce = object.nonce.ok_or(ObjectOpenError::HeaderMismatch)?;
            let derived = derive_object_key(&key, &entry.object_id, key_generation);
            let aad = header_aad(&object.header);
            ChaCha20Poly1305::new(Key::from_slice(&derived[..]))
                .decrypt(
                    Nonce::from_slice(&nonce),
                    Payload {
                        msg: &object.body,
                        aad: &aad,
                    },
                )
                .map_err(|_| ObjectOpenError::AuthenticationFailed)?
        }
    };
    if canonical.len() as u64 != entry.canonical_length
        || digest(&canonical) != entry.canonical_digest
    {
        return Err(ObjectOpenError::ContentCorrupt);
    }
    Ok(canonical)
}

pub(crate) fn validate_stored_object_header(
    entry: &ManifestEntry,
    object: &StoredHistoryObject,
) -> Result<(), ObjectOpenError> {
    let expected = StoredObjectHeader {
        format_version: 1,
        object_id: entry.object_id.clone(),
        content_kind: entry.content_kind,
        storage_mode: entry.storage_mode,
        canonical_length: entry.canonical_length,
        canonical_digest: entry.canonical_digest,
    };
    if object.header != expected {
        return Err(ObjectOpenError::HeaderMismatch);
    }
    match entry.storage_mode {
        StorageMode::ProviderManaged if object.nonce.is_none() => Ok(()),
        StorageMode::ClientEncrypted { .. } if object.nonce.is_some() => Ok(()),
        _ => Err(ObjectOpenError::HeaderMismatch),
    }
}

pub(crate) fn digest(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

fn derive_object_key(
    base_key: &[u8; 32],
    object_id: &ObjectId,
    generation: u64,
) -> Zeroizing<[u8; 32]> {
    let hkdf = Hkdf::<Sha256>::new(Some(OBJECT_KEY_SALT), base_key);
    let mut info = Vec::with_capacity(OBJECT_AAD_DOMAIN.len() + object_id.as_str().len() + 8);
    info.extend_from_slice(OBJECT_AAD_DOMAIN);
    info.extend_from_slice(object_id.as_str().as_bytes());
    info.extend_from_slice(&generation.to_be_bytes());
    let mut output = Zeroizing::new([0_u8; 32]);
    hkdf.expand(&info, &mut output[..])
        .expect("fixed-size HKDF output is valid");
    output
}

fn header_aad(header: &StoredObjectHeader) -> Vec<u8> {
    let mut aad = OBJECT_AAD_DOMAIN.to_vec();
    aad.extend(serde_json::to_vec(header).expect("stored history header serializes"));
    aad
}
