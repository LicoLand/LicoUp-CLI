//! Local transport credentials. No runtime content enters this directory.
use anyhow::{Result, anyhow};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

pub fn portable_data_dir_read_only() -> Result<PathBuf> {
    licoup_foundation::platform::paths::portable_data_dir_read_only()
        .map(|path| path.canonicalize().unwrap_or(path))
        .map_err(|_| anyhow!("mcp_state_unavailable"))
}
pub fn portable_data_dir() -> Result<PathBuf> {
    licoup_foundation::platform::paths::portable_data_dir()
        .and_then(|path| path.canonicalize().map_err(Into::into))
        .map_err(|_| anyhow!("mcp_state_unavailable"))
}

fn reject_links(path: &Path) -> Result<()> {
    for ancestor in path.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(anyhow!("mcp_state_unsafe"));
            }
            Ok(metadata) => {
                #[cfg(windows)]
                {
                    use std::os::windows::fs::MetadataExt;
                    if metadata.file_attributes() & 0x400 != 0 {
                        return Err(anyhow!("mcp_state_unsafe"));
                    }
                }
                #[cfg(not(windows))]
                let _ = metadata;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(anyhow!("mcp_state_unavailable")),
        }
    }
    Ok(())
}
#[cfg(unix)]
fn private_metadata(path: &Path, directory: bool) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink()
        || (if directory {
            !metadata.is_dir()
        } else {
            !metadata.is_file()
        })
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
    {
        return Err(anyhow!("mcp_state_unsafe"));
    }
    Ok(())
}
#[cfg(windows)]
fn private_metadata(path: &Path, directory: bool) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || metadata.is_dir() != directory {
        return Err(anyhow!("mcp_state_unsafe"));
    }
    crate::windows_private_state::validate(path)
}
pub fn ensure_private_dir(path: &Path) -> Result<()> {
    reject_links(path)?;
    if !path.exists() {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            fs::DirBuilder::new().mode(0o700).create(path)?;
        }
        #[cfg(windows)]
        fs::create_dir(path)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if fs::symlink_metadata(path)?.uid() != unsafe { libc::geteuid() } {
            return Err(anyhow!("mcp_state_unsafe"));
        }
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(windows)]
    crate::windows_private_state::harden(path)?;
    private_metadata(path, true)
}
pub fn read_existing_private_text_bounded(path: &Path, limit: usize) -> Result<Option<String>> {
    reject_links(path)?;
    if !path.exists() {
        return Ok(None);
    }
    private_metadata(path, false)?;
    if let Some(parent) = path.parent() {
        private_metadata(parent, true)?;
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = file.metadata()?;
        let named = fs::symlink_metadata(path)?;
        if !metadata.is_file()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o077 != 0
            || metadata.dev() != named.dev()
            || metadata.ino() != named.ino()
        {
            return Err(anyhow!("mcp_state_unsafe"));
        }
    }
    let mut text = String::new();
    file.take(limit as u64 + 1).read_to_string(&mut text)?;
    if text.len() > limit {
        return Err(anyhow!("mcp_state_invalid"));
    }
    Ok(Some(text))
}
pub fn atomic_write_private_text_bounded(path: &Path, text: &str, limit: usize) -> Result<()> {
    if text.len() > limit {
        return Err(anyhow!("mcp_state_invalid"));
    }
    let parent = path.parent().ok_or_else(|| anyhow!("mcp_state_invalid"))?;
    ensure_private_dir(parent)?;
    reject_links(path)?;
    let temporary = parent.join(format!(".publish-{}", uuid::Uuid::new_v4().simple()));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let result = (|| -> Result<()> {
        let mut file = options.open(&temporary)?;
        #[cfg(windows)]
        crate::windows_private_state::harden(&temporary)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::env;
    use std::os::unix::fs::{PermissionsExt, symlink};
    #[test]
    fn discovery_rejects_public_permissions_and_symlink_substitution() {
        let root = env::temp_dir().join(format!("mcp-private-fixture-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let private = root.join("private");
        ensure_private_dir(&private).unwrap();
        let file = private.join("fixture.json");
        atomic_write_private_text_bounded(&file, "synthetic", 32).unwrap();
        assert_eq!(
            read_existing_private_text_bounded(&file, 32)
                .unwrap()
                .as_deref(),
            Some("synthetic")
        );
        fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(read_existing_private_text_bounded(&file, 32).is_err());
        fs::remove_file(&file).unwrap();
        let other = root.join("other");
        fs::write(&other, "synthetic").unwrap();
        symlink(&other, &file).unwrap();
        assert!(read_existing_private_text_bounded(&file, 32).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
