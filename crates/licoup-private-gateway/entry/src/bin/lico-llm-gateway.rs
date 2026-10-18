//! Legacy binary name for the Gateway Runtime.
//! Prefer `lico-gateway`. Kept so installs that still look for this name work.

use licoup_native::platform::{gateway_composition, llm_api_key_vault::PlatformLlmApiKeyVault};
use licoup_private_gateway::control::client_auth;
use licoup_private_gateway::http::server::bind_address;
use licoup_private_gateway::model::llm_gateway::{CompiledGateway, GatewayConfig};
use licoup_private_gateway::state::DEFAULT_PORT;
use licoup_private_gateway::state::GatewayStatePaths;
use licoup_private_gateway::usage::GatewayUsageRecorder;
use licoup_private_gateway::{GatewayServeArgs, serve_gateway_runtime};
use licoup_vault::api_key::{GatewayCredentialHandoff, GatewayCredentialSlot};
use std::{
    env, fs,
    net::TcpListener,
    path::{Path, PathBuf},
    process::ExitCode,
    sync::{Arc, atomic::AtomicBool},
};

const MAX_CONFIG_BYTES: u64 = 1024 * 1024;
#[cfg(unix)]
const MAX_HANDOFF_BYTES: usize = 4 * 1024 * 1024;

fn main() -> ExitCode {
    match run(env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(()) => ExitCode::FAILURE,
    }
}

fn run(raw: Vec<String>) -> Result<(), ()> {
    let mut config = None;
    let mut state_root = None;
    let mut port = DEFAULT_PORT;
    let mut check_only = false;
    let mut credentials_fd = None;
    let mut credentials_control = None;
    let mut client_token_file = None;
    let mut usage = None;
    let mut index = 0;
    while index < raw.len() {
        match raw[index].as_str() {
            "--config" if config.is_none() => {
                index += 1;
                config = raw.get(index).map(PathBuf::from);
            }
            "--state-root" if state_root.is_none() => {
                index += 1;
                state_root = raw.get(index).map(PathBuf::from);
                if state_root.as_ref().is_none_or(|path| !path.is_absolute()) {
                    return Err(());
                }
            }
            "--port" => {
                index += 1;
                port = raw.get(index).and_then(|v| v.parse().ok()).ok_or(())?;
                if port == 0 {
                    return Err(());
                }
            }
            "--check" => check_only = true,
            "--disable-channels" => {}
            "--credentials-fd" if credentials_fd.is_none() => {
                index += 1;
                credentials_fd = Some(raw.get(index).and_then(|v| v.parse().ok()).ok_or(())?);
            }
            "--credentials-control" if credentials_control.is_none() => {
                index += 1;
                credentials_control = raw.get(index).map(PathBuf::from);
                if credentials_control
                    .as_ref()
                    .is_none_or(|path| !path.is_absolute())
                {
                    return Err(());
                }
            }
            "--client-token-file" if client_token_file.is_none() => {
                index += 1;
                client_token_file = raw.get(index).map(PathBuf::from);
                if client_token_file
                    .as_ref()
                    .is_none_or(|path| !path.is_absolute())
                {
                    return Err(());
                }
            }
            "--usage" if usage.is_none() => {
                index += 1;
                usage = raw.get(index).map(PathBuf::from);
            }
            _ => return Err(()),
        }
        index += 1;
    }
    if config.as_ref().is_some_and(|path| !path.is_absolute()) {
        return Err(());
    }
    if usage.as_ref().is_some_and(|path| !path.is_absolute()) {
        return Err(());
    }
    // The legacy name runs the same runtime, so it resolves the same layout: the
    // login item and the sidecar manager start one program under two names.
    let paths = match &state_root {
        Some(root) => GatewayStatePaths::under_data_home(root).map_err(|_| ())?,
        None => GatewayStatePaths::for_current_home().map_err(|_| ())?,
    };
    let config = match config.as_deref() {
        Some(named) => named.to_path_buf(),
        None => paths.config_or_default().map_err(|_| ())?,
    };
    let gateway_config = load_config(&config)?;
    let gateway = Arc::new(CompiledGateway::compile(gateway_config).map_err(|_| ())?);
    if check_only {
        println!(r#"{{"ok":true,"schemaVersion":"licoup.gateway-runtime-check.v1"}}"#);
        return Ok(());
    }
    let _data_home_access =
        licoup_foundation::platform::data_home_access::acquire_process_data_home_access()
            .map_err(|_| ())?;
    gateway_composition::install().map_err(|_| ())?;
    let credentials = Arc::new(match credentials_fd {
        Some(fd) => {
            let vault = PlatformLlmApiKeyVault::production().map_err(|_| ())?;
            GatewayCredentialSlot::new(
                vault
                    .gateway_lease_from_handoff(read_handoff(fd)?)
                    .map_err(|_| ())?,
            )
        }
        None => GatewayCredentialSlot::disconnected(),
    });
    let token_path = paths.client_token_path(client_token_file.as_deref());
    let client_token = Arc::new(match client_token_file.as_deref() {
        Some(_) => client_auth::read_token(&token_path).map_err(|_| ())?,
        None => client_auth::ensure_token_at(&token_path).map_err(|_| ())?,
    });
    let listener = TcpListener::bind(bind_address(port)).map_err(|_| ())?;
    let usage =
        Arc::new(GatewayUsageRecorder::open(paths.usage_path(usage.as_deref())).map_err(|_| ())?);
    serve_gateway_runtime(
        GatewayServeArgs {
            listener,
            gateway,
            credentials,
            client_token,
            usage,
            credentials_control,
        },
        Arc::new(AtomicBool::new(false)),
    )
    .map_err(|_| ())
}

fn load_config(path: &Path) -> Result<GatewayConfig, ()> {
    let metadata = fs::symlink_metadata(path).map_err(|_| ())?;
    if !metadata.file_type().is_file() || metadata.len() == 0 || metadata.len() > MAX_CONFIG_BYTES {
        return Err(());
    }
    serde_json::from_slice(&fs::read(path).map_err(|_| ())?).map_err(|_| ())
}

#[cfg(unix)]
fn read_handoff(fd: u32) -> Result<GatewayCredentialHandoff, ()> {
    use std::io::Read as _;
    use std::os::fd::{FromRawFd as _, RawFd};
    let mut pipe = unsafe { fs::File::from_raw_fd(fd as RawFd) };
    let mut buffer = Vec::new();
    pipe.by_ref()
        .take(MAX_HANDOFF_BYTES as u64 + 1)
        .read_to_end(&mut buffer)
        .map_err(|_| ())?;
    if buffer.len() > MAX_HANDOFF_BYTES {
        return Err(());
    }
    GatewayCredentialHandoff::from_json(&buffer).map_err(|_| ())
}

#[cfg(not(unix))]
fn read_handoff(_fd: u32) -> Result<GatewayCredentialHandoff, ()> {
    Err(())
}
