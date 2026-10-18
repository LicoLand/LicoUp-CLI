//! The Gateway Runtime's own state layout and the document it starts from.
//!
//! The runtime process and the client's service manager are separate programs,
//! and they have to agree about one thing: where this runtime keeps its bytes.
//! That agreement lives here rather than in the caller, so both sides resolve
//! the same paths from the same declarations. The layout belongs to the runtime
//! because the runtime owns the format — it is the process that appends the
//! usage journal and reads the configuration it was handed — and the client is
//! a reader of it.
//!
//! Nothing here creates a data root beyond the private state directory itself,
//! and nothing here decides what the runtime should contain: a caller that
//! wants to know whether a path exists asks the filesystem.

use crate::default_config::default_config;
use crate::model::llm_gateway::{CompiledGateway, GatewayConfig};
use anyhow::{Result, anyhow};
use licoup_foundation::platform::{file_security, file_security::ensure_private_dir, paths};
use std::path::{Path, PathBuf};

/// The one directory this runtime keeps its state in, under the data home.
pub const STATE_DIRECTORY: &str = "llm-gateway";

/// The port the Gateway Runtime binds when a caller names none.
pub const DEFAULT_PORT: u16 = 15_722;

/// Where the running Gateway and the client that manages it read their paths.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GatewayStatePaths {
    /// The `llm-gateway` directory itself.
    pub root: PathBuf,
    /// The generated provider and routing configuration.
    pub config: PathBuf,
    /// The pid record the managing client writes and reads.
    pub pid: PathBuf,
    /// The append-only sidecar log.
    pub log: PathBuf,
    /// The usage journal this runtime appends to.
    pub usage: PathBuf,
    /// The private credentials control socket.
    pub credentials_control: PathBuf,
    /// The token that authenticates local API clients.
    pub client_token: PathBuf,
}

impl GatewayStatePaths {
    /// The layout under one data home, with the state directory prepared.
    ///
    /// The directory is created and hardened here because every caller of this
    /// function is about to write into it or hand a path inside it to another
    /// process. The root of the layout is resolved from the caller's own
    /// selection — the environment or the saved root — by
    /// [`licoup_foundation::platform::paths::portable_data_dir`].
    pub fn under_data_home(data_home: impl AsRef<Path>) -> Result<Self> {
        let root = data_home.as_ref().join(STATE_DIRECTORY);
        ensure_private_dir(&root)?;
        Ok(Self {
            credentials_control: crate::control::credentials::control_socket_path(&root),
            client_token: root.join("client-token"),
            config: root.join("config.json"),
            pid: root.join("gateway.pid"),
            log: root.join("gateway.log"),
            usage: root.join("usage.json"),
            root,
        })
    }

    /// The layout under the data home this process was started for.
    pub fn for_current_home() -> Result<Self> {
        Self::under_data_home(paths::portable_data_dir()?)
    }

    /// The usage journal path, defaulted to this layout when the caller named
    /// none.
    pub fn usage_path(&self, named: Option<&Path>) -> PathBuf {
        named
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.usage.clone())
    }

    /// The client-token path, defaulted to this layout when the caller named
    /// none.
    pub fn client_token_path(&self, named: Option<&Path>) -> PathBuf {
        named
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.client_token.clone())
    }

    /// The configuration this layout holds, when it holds one.
    ///
    /// A caller that runs the runtime standalone asks this first: an absent
    /// configuration is the caller's to materialize, and a path that is present
    /// but is not a regular file is a refusal rather than a file to overwrite.
    pub fn existing_config(&self) -> Result<Option<PathBuf>> {
        let metadata = match std::fs::symlink_metadata(&self.config) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(anyhow!("gateway_config_unreadable")),
        };
        if !metadata.file_type().is_file() {
            return Err(anyhow!("gateway_config_unreadable"));
        }
        Ok(Some(self.config.clone()))
    }

    /// The configuration this layout holds, materializing the provider
    /// boundaries of [`default_config`] when it holds none yet.
    ///
    /// This is what makes the runtime startable by itself: the login item
    /// launches the installed package's entry with nothing but a port, so the
    /// entry has to be able to arrive at a configuration the first time it runs.
    /// A configuration that is already there is left exactly as it is — the
    /// runtime never rewrites the user's file — and one this call writes is
    /// validated by compiling it before it is returned.
    pub fn config_or_default(&self) -> Result<PathBuf> {
        if self.existing_config()?.is_some() {
            return Ok(self.config.clone());
        }
        let body = serde_json::to_string_pretty(&default_config())?;
        file_security::atomic_write_private_text(&self.config, &body)?;
        let text = std::fs::read_to_string(&self.config)
            .map_err(|_| anyhow!("gateway_config_unreadable"))?;
        let config: GatewayConfig =
            serde_json::from_str(&text).map_err(|_| anyhow!("gateway_config_invalid"))?;
        CompiledGateway::compile(config).map_err(|_| anyhow!("gateway_config_invalid"))?;
        Ok(self.config.clone())
    }
}
