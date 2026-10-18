//! Private local transport shared by the Conversation host and admitted native
//! clients. The endpoint name is an opaque token stored below the client-owned
//! portable data root; no runtime path or process fact crosses the RPC wire.

use anyhow::{Context, Result, anyhow};
use fs2::FileExt;
use interprocess::local_socket::{GenericNamespaced, Stream, ToNsName as _, traits::Stream as _};
use sha2::{Digest, Sha256};
use std::{
    env, fs,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::OnceLock,
};

fn host_owner_lock_path(root: &Path) -> PathBuf {
    root.join("client-state")
        .join("conversation-runtime")
        .join("host-owner.lock")
}

/// Try to take the listener owner lock. A returned file remains locked until
/// it is dropped, giving stop callers direct proof that the host has exited.
pub fn try_acquire_host_owner_lock(root: &Path) -> Result<Option<fs::File>> {
    let path = host_owner_lock_path(root);
    licoup_foundation::platform::file_security::ensure_private_dir(&root.join("client-state"))?;
    if let Some(parent) = path.parent() {
        licoup_foundation::platform::file_security::ensure_private_dir(parent)?;
    }
    let file = licoup_foundation::platform::file_security::open_private_lock_file(&path)?;
    match FileExt::try_lock_exclusive(&file) {
        Ok(()) => Ok(Some(file)),
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(None),
        Err(_) => Err(anyhow!("conversation host unavailable")),
    }
}

pub const STDIO_RPC_PROTOCOL: &str = "licoup.stdio.v1";

static EXECUTABLE_GENERATION: OnceLock<Option<String>> = OnceLock::new();

fn metadata_identity(metadata: &fs::Metadata) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        format!(
            "{}:{}:{}:{}:{}",
            metadata.dev(),
            metadata.ino(),
            metadata.len(),
            metadata.mtime(),
            metadata.mtime_nsec()
        )
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt as _;
        // `file_index` is not available on stable Windows; the creation time
        // changes whenever an installer replaces the binary at the same path,
        // which is the generation change this identity must observe.
        format!(
            "{}:{}:{}",
            metadata.creation_time(),
            metadata.file_size(),
            metadata.last_write_time()
        )
    }
}

/// Opaque generation of the executable backing this process.
///
/// The first lookup is retained for the process lifetime. If an installer
/// replaces the file at the same path, an older host therefore keeps using
/// its original endpoint while the newly launched binary selects a new one.
pub fn executable_generation() -> io::Result<String> {
    EXECUTABLE_GENERATION
        .get_or_init(|| {
            env::current_exe()
                .and_then(fs::metadata)
                .map(|metadata| endpoint_generation(&metadata_identity(&metadata)))
                .ok()
        })
        .clone()
        .ok_or_else(|| io::Error::other("conversation host unavailable"))
}

fn endpoint_generation(identity: &str) -> String {
    let digest = Sha256::digest(identity.as_bytes());
    digest[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub fn endpoint_name() -> Result<interprocess::local_socket::Name<'static>> {
    let root = licoup_foundation::platform::paths::portable_data_dir()?;
    endpoint_name_for_root(&root)
}

fn existing_endpoint_name() -> Result<interprocess::local_socket::Name<'static>> {
    let root = licoup_foundation::platform::paths::portable_data_dir_read_only()?;
    existing_endpoint_name_for_root(&root)
}

fn read_endpoint_token(token_path: &Path) -> Result<String> {
    let token = licoup_foundation::platform::file_security::read_existing_private_text_bounded(
        token_path, 64,
    )?
    .ok_or_else(|| anyhow!("conversation endpoint unavailable"))?;
    let token = token.trim();
    if token.len() != 32 || !token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(anyhow!("conversation endpoint unavailable"));
    }
    Ok(token.to_owned())
}

fn endpoint_name_from_token(token: &str) -> Result<interprocess::local_socket::Name<'static>> {
    endpoint_name_from_token_and_generation(token, &executable_generation()?)
}

fn endpoint_name_from_token_and_generation(
    token: &str,
    generation: &str,
) -> Result<interprocess::local_socket::Name<'static>> {
    format!("licoup-conversation-{token}-{generation}")
        .to_ns_name::<GenericNamespaced>()
        .context("conversation endpoint unavailable")
}

fn existing_endpoint_name_for_root(
    root: &Path,
) -> Result<interprocess::local_socket::Name<'static>> {
    let token_path = root
        .join("client-state")
        .join("conversation-runtime")
        .join("endpoint-token");
    endpoint_name_from_token(&read_endpoint_token(&token_path)?)
}

pub fn endpoint_name_for_root(root: &Path) -> Result<interprocess::local_socket::Name<'static>> {
    let endpoint_root = root.join("client-state").join("conversation-runtime");
    licoup_foundation::platform::file_security::ensure_private_dir(&root.join("client-state"))?;
    licoup_foundation::platform::file_security::ensure_private_dir(&endpoint_root)?;
    let token_path = endpoint_root.join("endpoint-token");
    let token = match read_endpoint_token(&token_path) {
        Ok(token) => token,
        Err(_) => {
            let token = uuid::Uuid::new_v4().simple().to_string();
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&token_path)
            {
                Ok(mut file) => {
                    file.write_all(token.as_bytes())?;
                    file.sync_all()?;
                    licoup_foundation::platform::file_security::harden_private_path(&token_path)?;
                    token
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    read_endpoint_token(&token_path)?
                }
                Err(_) => return Err(anyhow!("conversation endpoint unavailable")),
            }
        }
    };
    endpoint_name_from_token(&token)
}

pub fn connect() -> io::Result<Stream> {
    let name = endpoint_name().map_err(io::Error::other)?;
    Stream::connect(name)
}

/// Connect only when the host has already published its private endpoint.
/// Unlike `connect`, this read-only lookup never creates a directory or token.
pub fn connect_existing() -> io::Result<Stream> {
    let name = existing_endpoint_name().map_err(io::Error::other)?;
    Stream::connect(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_is_stable_and_private_to_the_portable_root() {
        let root = std::env::temp_dir().join(format!(
            "licoup-conversation-endpoint-test-{}",
            uuid::Uuid::new_v4()
        ));
        let first = endpoint_name_for_root(&root).unwrap();
        let second = endpoint_name_for_root(&root).unwrap();
        assert_eq!(first, second);
        let token =
            fs::read_to_string(root.join("client-state/conversation-runtime/endpoint-token"))
                .unwrap();
        assert_eq!(token.len(), 32);
        assert!(token.bytes().all(|byte| byte.is_ascii_hexdigit()));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn endpoint_isolated_by_executable_generation() {
        let token = "0".repeat(32);
        let current_generation = endpoint_generation("fixture-build-current");
        let stale_generation = endpoint_generation("fixture-build-stale");
        let current = endpoint_name_from_token_and_generation(&token, &current_generation).unwrap();
        let stale = endpoint_name_from_token_and_generation(&token, &stale_generation).unwrap();

        assert_ne!(current, stale);
        assert_eq!(
            current,
            endpoint_name_from_token_and_generation(&token, &current_generation).unwrap()
        );
        assert_eq!(current_generation.len(), 16);
    }

    #[test]
    fn executable_generation_is_stable_for_the_process() {
        let first = executable_generation().unwrap();
        let second = executable_generation().unwrap();
        assert_eq!(first, second);
        assert_eq!(first.len(), 16);
        assert!(first.bytes().all(|byte| byte.is_ascii_hexdigit()));
    }

    #[test]
    fn existing_endpoint_lookup_never_creates_missing_state() {
        let root = std::env::temp_dir().join(format!(
            "licoup-conversation-endpoint-read-only-test-{}",
            uuid::Uuid::new_v4()
        ));
        let previous =
            licoup_foundation::platform::paths::set_portable_data_dir_override(Some(root.clone()));
        let missing = existing_endpoint_name();
        licoup_foundation::platform::paths::set_portable_data_dir_override(previous);
        assert!(missing.is_err());
        assert!(!root.exists());

        let created = endpoint_name_for_root(&root).unwrap();
        let observed = existing_endpoint_name_for_root(&root).unwrap();
        assert_eq!(created, observed);
        fs::remove_dir_all(root).unwrap();
    }
}
