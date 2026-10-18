//! Private Gateway Runtime: the loopback LLM model gateway.

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

const MAX_ARGUMENTS: usize = 12;
const MAX_ARGUMENT_BYTES: usize = 8 * 1024;
const MAX_CONFIG_BYTES: u64 = 1024 * 1024;
#[cfg(unix)]
const MAX_HANDOFF_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Eq, PartialEq)]
struct Arguments {
    /// The configuration the caller named, or none: a caller that names none
    /// gets the runtime's own state layout, which is what the login item does
    /// when it launches this program directly.
    config: Option<PathBuf>,
    /// The data home this process was started for, when the caller selected one.
    state_root: Option<PathBuf>,
    port: u16,
    check_only: bool,
    credentials_fd: Option<u32>,
    credentials_control: Option<PathBuf>,
    client_token_file: Option<PathBuf>,
    /// The usage journal the caller named, or none, resolved like `config`.
    usage: Option<PathBuf>,
}

impl Arguments {
    /// The paths this run reads and writes, resolved from the caller's own
    /// selection exactly as the client's service manager resolves them.
    fn state_paths(&self) -> Result<GatewayStatePaths, ()> {
        match &self.state_root {
            Some(root) => GatewayStatePaths::under_data_home(root).map_err(|_| ()),
            None => GatewayStatePaths::for_current_home().map_err(|_| ()),
        }
    }
}

fn main() -> ExitCode {
    match run(env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(()) => ExitCode::FAILURE,
    }
}

fn run(raw: Vec<String>) -> Result<(), ()> {
    let arguments = parse_arguments(&raw)?;
    let paths = arguments.state_paths()?;
    // The runtime owns its configuration: a caller that named one is obeyed, and
    // a caller that named none — the login item, which launches the installed
    // package's entry with nothing but a port — gets the document this runtime
    // would have written for itself.
    let config = match arguments.config.as_deref() {
        Some(named) => load_config(named)?,
        None => load_config(&paths.config_or_default().map_err(|_| ())?)?,
    };
    let gateway = Arc::new(CompiledGateway::compile(config).map_err(|_| ())?);
    if arguments.check_only {
        println!(
            r#"{{"ok":true,"schemaVersion":"licoup.gateway-runtime-check.v1","layers":["llm"]}}"#
        );
        return Ok(());
    }
    let _data_home_access =
        licoup_foundation::platform::data_home_access::acquire_process_data_home_access()
            .map_err(|_| ())?;
    gateway_composition::install().map_err(|_| ())?;
    let credentials = Arc::new(match arguments.credentials_fd {
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
    // The authenticated local endpoint always has a token: the caller may name
    // the file, and a caller that names none gets the one this layout holds,
    // created when the runtime is the first program to need it.
    let token_path = paths.client_token_path(arguments.client_token_file.as_deref());
    let client_token = Arc::new(match arguments.client_token_file.as_deref() {
        Some(_) => client_auth::read_token(&token_path).map_err(|_| ())?,
        None => client_auth::ensure_token_at(&token_path).map_err(|_| ())?,
    });
    let listener = TcpListener::bind(bind_address(arguments.port)).map_err(|_| ())?;
    let usage_path = paths.usage_path(arguments.usage.as_deref());
    let usage = Arc::new(GatewayUsageRecorder::open(usage_path).map_err(|_| ())?);
    let stop = Arc::new(AtomicBool::new(false));
    serve_gateway_runtime(
        GatewayServeArgs {
            listener,
            gateway,
            credentials,
            client_token,
            usage,
            credentials_control: arguments.credentials_control,
        },
        stop,
    )
    .map_err(|_| ())
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

fn parse_arguments(raw: &[String]) -> Result<Arguments, ()> {
    if raw.len() > MAX_ARGUMENTS || raw.iter().map(String::len).sum::<usize>() > MAX_ARGUMENT_BYTES
    {
        return Err(());
    }
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
                if config.is_none() {
                    return Err(());
                }
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
                port = raw
                    .get(index)
                    .and_then(|value| value.parse().ok())
                    .ok_or(())?;
                if port == 0 {
                    return Err(());
                }
            }
            "--check" if !check_only => check_only = true,
            // Accepted for the launch arguments installed runtimes still pass;
            // this runtime has no channel layer to disable.
            "--disable-channels" => {}
            "--credentials-fd" if credentials_fd.is_none() => {
                index += 1;
                credentials_fd = Some(
                    raw.get(index)
                        .and_then(|value| value.parse::<u32>().ok())
                        .ok_or(())?,
                );
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
                if usage.as_ref().is_none_or(|path| !path.is_absolute()) {
                    return Err(());
                }
            }
            _ => return Err(()),
        }
        index += 1;
    }
    // A configuration and a usage journal are no longer required arguments:
    // naming one is an override, and naming none is the login item's own case.
    if config.as_ref().is_some_and(|path| !path.is_absolute()) {
        return Err(());
    }
    Ok(Arguments {
        config,
        state_root,
        port,
        check_only,
        credentials_fd,
        credentials_control,
        client_token_file,
        usage,
    })
}

fn load_config(path: &Path) -> Result<GatewayConfig, ()> {
    let metadata = fs::symlink_metadata(path).map_err(|_| ())?;
    if !metadata.file_type().is_file() || metadata.len() == 0 || metadata.len() > MAX_CONFIG_BYTES {
        return Err(());
    }
    let bytes = fs::read(path).map_err(|_| ())?;
    serde_json::from_slice(&bytes).map_err(|_| ())
}
