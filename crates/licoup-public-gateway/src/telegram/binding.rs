//! Pairing and per-chat agent/session binding store.

use anyhow::{Result, anyhow, ensure};
use fs2::FileExt;
use licoup_foundation::platform::file_security::{
    atomic_write_private_text, ensure_private_dir, open_private_lock_file,
    read_private_text_bounded,
};
use licoup_foundation::platform::paths;
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const STATE_DIRECTORY: &str = "telegram-gateway";
const STATE_FILE: &str = "bindings.json";
const MAX_STATE_BYTES: usize = 256 * 1024;
const PAIRING_TTL: Duration = Duration::from_secs(60 * 60);
pub const BINDING_SCHEMA: &str = "licoup.telegram-gateway-bindings.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingRecord {
    pub code: String,
    pub chat_id: i64,
    pub user_id: i64,
    pub username: Option<String>,
    pub created_at: u64,
    pub expires_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ChatBinding {
    pub chat_id: i64,
    pub user_id: i64,
    pub username: Option<String>,
    pub paired: bool,
    pub agent_id: Option<String>,
    pub session_id: Option<String>,
    pub updated_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct BindingDocument {
    schema_version: String,
    pairings: Vec<PairingRecord>,
    chats: BTreeMap<String, ChatBinding>,
}

#[derive(Debug)]
pub struct BindingStore {
    path: PathBuf,
    lock: File,
    document: BindingDocument,
}

impl BindingStore {
    pub fn open_default() -> Result<Self> {
        let root = paths::portable_data_dir()?.join(STATE_DIRECTORY);
        ensure_private_dir(&root)?;
        Self::open(root.join(STATE_FILE))
    }

    pub fn open(path: PathBuf) -> Result<Self> {
        if let Some(parent) = path.parent() {
            ensure_private_dir(parent)?;
        }
        let lock = open_private_lock_file(&lock_path(&path))?;
        let document = read_document(&path)?;
        let store = Self {
            path,
            lock,
            document,
        };
        Ok(store)
    }

    /// Reload the latest on-disk document so external (CLI or another
    /// runtime) writes become visible before reads.
    pub fn refresh(&mut self) -> Result<()> {
        self.lock
            .lock_exclusive()
            .map_err(|_| anyhow!("telegram_gateway_bindings_lock_failed"))?;
        let result = read_document(&self.path).map(|document| self.document = document);
        let _ = self.lock.unlock();
        result
    }

    /// Apply one mutation atomically: hold the advisory lock across reload,
    /// merge, and atomic save so concurrent writers never silently overwrite
    /// each other's changes.
    fn mutate<R>(&mut self, change: impl FnOnce(&mut BindingDocument) -> Result<R>) -> Result<R> {
        self.lock
            .lock_exclusive()
            .map_err(|_| anyhow!("telegram_gateway_bindings_lock_failed"))?;
        let result = (|| {
            let mut document = read_document(&self.path)?;
            let value = change(&mut document)?;
            let raw = serde_json::to_string_pretty(&document)?;
            atomic_write_private_text(&self.path, &raw)?;
            self.document = document;
            Ok(value)
        })();
        let _ = self.lock.unlock();
        result
    }

    pub fn binding(&self, chat_id: i64) -> Option<&ChatBinding> {
        self.document.chats.get(&chat_id.to_string())
    }

    pub fn is_paired(&self, chat_id: i64, user_id: i64) -> bool {
        self.document
            .chats
            .get(&chat_id.to_string())
            .is_some_and(|binding| binding.paired && binding.user_id == user_id)
    }

    pub fn request_pairing(
        &mut self,
        chat_id: i64,
        user_id: i64,
        username: Option<String>,
    ) -> Result<PairingRecord> {
        self.mutate(|document| {
            let now = now_secs();
            document.pairings.retain(|record| record.expires_at > now);
            ensure!(
                !document
                    .chats
                    .get(&chat_id.to_string())
                    .is_some_and(|binding| binding.paired && binding.user_id == user_id),
                "telegram_gateway_already_paired"
            );
            document
                .pairings
                .retain(|record| record.chat_id != chat_id || record.user_id != user_id);
            let record = PairingRecord {
                code: generate_pairing_code(),
                chat_id,
                user_id,
                username,
                created_at: now,
                expires_at: now + PAIRING_TTL.as_secs(),
            };
            document.pairings.push(record.clone());
            Ok(record)
        })
    }

    pub fn approve(&mut self, code: &str) -> Result<ChatBinding> {
        let normalized = code.trim().to_uppercase();
        ensure!(
            !normalized.is_empty(),
            "telegram_gateway_pairing_code_invalid"
        );
        self.mutate(|document| {
            let now = now_secs();
            document.pairings.retain(|record| record.expires_at > now);
            let index = document
                .pairings
                .iter()
                .position(|record| record.code == normalized)
                .ok_or_else(|| anyhow!("telegram_gateway_pairing_not_found"))?;
            let record = document.pairings.remove(index);
            let binding = ChatBinding {
                chat_id: record.chat_id,
                user_id: record.user_id,
                username: record.username,
                paired: true,
                agent_id: None,
                session_id: None,
                updated_at: now,
            };
            document
                .chats
                .insert(record.chat_id.to_string(), binding.clone());
            Ok(binding)
        })
    }

    pub fn revoke(&mut self, chat_id: i64) -> Result<bool> {
        self.mutate(|document| {
            let removed = document.chats.remove(&chat_id.to_string()).is_some();
            let before = document.pairings.len();
            document.pairings.retain(|record| record.chat_id != chat_id);
            Ok(removed || document.pairings.len() != before)
        })
    }

    pub fn paired_chats(&self) -> Vec<ChatBinding> {
        self.document
            .chats
            .values()
            .filter(|binding| binding.paired)
            .cloned()
            .collect()
    }

    pub fn set_agent(&mut self, chat_id: i64, agent_id: Option<String>) -> Result<ChatBinding> {
        self.mutate(|document| {
            let mut binding = document
                .chats
                .get(&chat_id.to_string())
                .cloned()
                .ok_or_else(|| anyhow!("telegram_gateway_not_paired"))?;
            ensure!(binding.paired, "telegram_gateway_not_paired");
            if binding.agent_id.as_deref() != agent_id.as_deref() {
                binding.session_id = None;
            }
            binding.agent_id = agent_id;
            binding.updated_at = now_secs();
            document.chats.insert(chat_id.to_string(), binding.clone());
            Ok(binding)
        })
    }

    pub fn set_session(&mut self, chat_id: i64, session_id: Option<String>) -> Result<ChatBinding> {
        self.mutate(|document| {
            let mut binding = document
                .chats
                .get(&chat_id.to_string())
                .cloned()
                .ok_or_else(|| anyhow!("telegram_gateway_not_paired"))?;
            ensure!(binding.paired, "telegram_gateway_not_paired");
            ensure!(
                binding.agent_id.is_some(),
                "telegram_gateway_agent_required"
            );
            binding.session_id = session_id;
            binding.updated_at = now_secs();
            document.chats.insert(chat_id.to_string(), binding.clone());
            Ok(binding)
        })
    }

    pub fn pending_pairings(&self) -> Vec<PairingRecord> {
        self.document.pairings.clone()
    }

    /// Drop expired pairing codes under the same lock protocol used by
    /// mutations (used by read-facing CLI listing).
    pub fn prune_expired(&mut self) -> Result<()> {
        let now = now_secs();
        self.mutate(|document| {
            document.pairings.retain(|record| record.expires_at > now);
            Ok(())
        })
    }
}

pub fn list_pairings() -> Result<Value> {
    let mut store = BindingStore::open_default()?;
    store.refresh()?;
    store.prune_expired()?;
    let pairings = store.pending_pairings();
    let chats = store.paired_chats();
    Ok(json!({
        "ok": true,
        "schemaVersion": BINDING_SCHEMA,
        "pairings": pairings,
        "chats": chats,
    }))
}

pub fn approve_pairing(code: &str) -> Result<Value> {
    let mut store = BindingStore::open_default()?;
    let binding = store.approve(code)?;
    Ok(json!({
        "ok": true,
        "schemaVersion": BINDING_SCHEMA,
        "approved": true,
        "binding": binding,
    }))
}

pub fn revoke_pairing(chat_id: i64) -> Result<Value> {
    let mut store = BindingStore::open_default()?;
    let removed = store.revoke(chat_id)?;
    Ok(json!({
        "ok": true,
        "schemaVersion": BINDING_SCHEMA,
        "revoked": removed,
        "chatId": chat_id,
    }))
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or(0)
}

fn generate_pairing_code() -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut bytes = [0u8; 6];
    OsRng.fill_bytes(&mut bytes);
    bytes
        .iter()
        .map(|byte| ALPHABET[(*byte as usize) % ALPHABET.len()] as char)
        .collect()
}

fn lock_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|value| value.to_os_string())
        .unwrap_or_else(|| "bindings.json".into());
    name.push(".lock");
    path.with_file_name(name)
}

fn read_document(path: &Path) -> Result<BindingDocument> {
    match read_private_text_bounded(path, MAX_STATE_BYTES)? {
        Some(raw) => {
            serde_json::from_str(&raw).map_err(|_| anyhow!("telegram_gateway_bindings_invalid"))
        }
        None => Ok(BindingDocument {
            schema_version: BINDING_SCHEMA.to_owned(),
            pairings: Vec::new(),
            chats: BTreeMap::new(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use licoup_foundation::platform::paths::set_portable_data_dir_override;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::{Duration, Instant};

    static LOCK: Mutex<()> = Mutex::new(());

    fn with_store<F: FnOnce(BindingStore)>(body: F) {
        let _guard = LOCK.lock().unwrap();
        let root = std::env::temp_dir().join(format!("licoup-tg-bind-{}", uuid::Uuid::new_v4()));
        let _ = std::fs::create_dir_all(&root);
        let previous = set_portable_data_dir_override(Some(root.clone()));
        let store = BindingStore::open_default().unwrap();
        body(store);
        set_portable_data_dir_override(previous);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn pairing_approve_and_agent_switch_clears_session() {
        with_store(|mut store| {
            let pending = store.request_pairing(11, 22, Some("alice".into())).unwrap();
            let binding = store.approve(&pending.code).unwrap();
            assert!(binding.paired);
            store.set_agent(11, Some("cursor".into())).unwrap();
            store.set_session(11, Some("sess-1".into())).unwrap();
            let switched = store.set_agent(11, Some("codex".into())).unwrap();
            assert_eq!(switched.agent_id.as_deref(), Some("codex"));
            assert_eq!(switched.session_id, None);
        });
    }

    #[test]
    fn revoke_persists_when_only_pending_pairing_exists() {
        with_store(|mut store| {
            let pending = store.request_pairing(33, 44, Some("bob".into())).unwrap();
            assert_eq!(store.pending_pairings().len(), 1);
            assert!(store.revoke(pending.chat_id).unwrap());
            assert!(store.pending_pairings().is_empty());
            let reopened = BindingStore::open_default().unwrap();
            assert!(reopened.pending_pairings().is_empty());
        });
    }

    #[test]
    fn list_pairings_includes_approved_chats() {
        with_store(|mut store| {
            let pending = store.request_pairing(55, 66, Some("carol".into())).unwrap();
            store.approve(&pending.code).unwrap();
            let listed = list_pairings().unwrap();
            assert!(listed["pairings"].as_array().unwrap().is_empty());
            assert_eq!(listed["chats"].as_array().unwrap().len(), 1);
            assert_eq!(listed["chats"][0]["chatId"], 55);
        });
    }

    #[test]
    fn separate_handles_merge_without_silent_overwrite() {
        with_store(|store| {
            let path = store.path.clone();
            let mut first = store;
            let mut second = BindingStore::open(path).unwrap();

            let pending = first.request_pairing(11, 22, Some("alice".into())).unwrap();
            first.approve(&pending.code).unwrap();
            let generation = first
                .set_agent(11, Some("cursor".into()))
                .unwrap()
                .updated_at;

            let pending = second.request_pairing(33, 44, Some("bob".into())).unwrap();
            second.approve(&pending.code).unwrap();
            second.set_agent(33, Some("codex".into())).unwrap();
            let merged = second.set_session(33, Some("sess-9".into())).unwrap();

            assert!(merged.updated_at >= generation);
            let binding = first.binding(11).cloned().unwrap();
            assert_eq!(binding.agent_id.as_deref(), Some("cursor"));
            assert_eq!(binding.session_id, None);
            first.refresh().unwrap();
            let other = first.binding(33).cloned().unwrap();
            assert_eq!(other.session_id.as_deref(), Some("sess-9"));
            assert!(other.updated_at >= generation);
        });
    }

    #[test]
    fn advisory_lock_serializes_refresh() {
        with_store(|store| {
            let path = store.path.clone();
            drop(store);
            let held = open_private_lock_file(&lock_path(&path)).unwrap();
            FileExt::lock_exclusive(&held).unwrap();

            let started = Arc::new(AtomicBool::new(false));
            let finished = Arc::new(AtomicBool::new(false));
            let thread_started = Arc::clone(&started);
            let thread_finished = Arc::clone(&finished);
            let handle = thread::spawn(move || {
                let mut store = BindingStore::open(path).unwrap();
                thread_started.store(true, Ordering::SeqCst);
                store.refresh().unwrap();
                thread_finished.store(true, Ordering::SeqCst);
            });

            let deadline = Instant::now() + Duration::from_secs(2);
            while !started.load(Ordering::SeqCst) {
                assert!(Instant::now() < deadline, "refresh thread never started");
                thread::sleep(Duration::from_millis(2));
            }
            thread::sleep(Duration::from_millis(80));
            assert!(
                !finished.load(Ordering::SeqCst),
                "refresh completed while the advisory lock was held"
            );
            FileExt::unlock(&held).unwrap();
            handle.join().unwrap();
            assert!(finished.load(Ordering::SeqCst));
        });
    }
}
