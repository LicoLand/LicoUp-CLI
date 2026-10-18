//! Native client CLI and bounded stdio RPC entry point.

use anyhow::Result;
use serde_json::{Value, json};
use std::{
    env,
    io::{self, BufRead, Write},
    panic::{self, AssertUnwindSafe, catch_unwind},
    path::PathBuf,
    sync::{Arc, Mutex, atomic::AtomicU64},
};

#[path = "licoup/conversation_host.rs"]
mod conversation_host;
#[path = "licoup/presentation.rs"]
mod presentation;
#[path = "licoup/private_stdin_json.rs"]
mod private_stdin_json;
#[path = "licoup/stdio_rpc.rs"]
mod stdio_rpc;

use presentation::{print_json, print_usage};
use private_stdin_json::materialize_private_stdin_json;
use stdio_rpc::{execute_rpc_cli, serve_data_home_stdio_rpc, serve_stdio_rpc};

// Keep the public CLI boundary token explicit here: the source-boundary gate
// verifies that malformed or substituted protocols cannot silently enter the
// native stdio request parser.
const STDIO_RPC_PROTOCOL: &str = "licoup.stdio.v1";
const STDIO_RPC_MAX_REQUEST_BYTES: usize = 16 * 1024 * 1024;
const STDIO_RPC_MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
const STDIO_RPC_MAX_ID_BYTES: usize = 128;
const STDIO_RPC_MAX_ARGS: usize = 4_097;
fn main() -> Result<()> {
    env_logger::Builder::from_default_env()
        .target(env_logger::Target::Stderr)
        .init();
    let args = env::args().skip(1).collect::<Vec<_>>();
    // Relocation and archive recovery use dedicated, otherwise idle processes: they
    // must acquire the exclusive coordinator lease rather than wait on their own
    // process-lifetime shared lease.
    let host_stop_control = matches!(args.as_slice(), [rpc, host, action]
        if rpc == "rpc" && host == "conversation-host"
            && matches!(action.as_str(), "--stop" | "stop" | "--shutdown" | "shutdown"));
    let exclusive_data_home_process = args.as_slice() == ["rpc", "data-home"]
        // A stop request only controls an already owned host. Taking a shared
        // data lease here would prevent it from draining an exclusive barrier.
        || host_stop_control
        || args.first().map(String::as_str) == Some("backup");
    let _data_home_access = if exclusive_data_home_process {
        None
    } else {
        Some(licoup_foundation::platform::data_home_access::acquire_process_data_home_access()?)
    };
    // The domain asks environment ports instead of inspecting the host itself.
    // This process installs the answers it can truthfully give; a port whose
    // installation fails stays fail-closed rather than fabricating a fact.
    if let Err(error) = licoup_native::install_environment_ports() {
        eprintln!("licoup environment ports remain unavailable: {error}");
    }
    if args.as_slice() == ["rpc", "stdio"] {
        // The RPC wire response is already fail-closed and redacted. Keep the
        // process panic hook equally bounded so a panic payload cannot leak a
        // path, command argument, or secret to the parent application's logs.
        panic::set_hook(Box::new(|_| {
            eprintln!("licoup RPC command terminated unexpectedly");
        }));
        // The desktop client spawns this bridge lane during normal startup,
        // so it owns starting the persistent conversation host — and with it
        // the default-enabled, supervised Subagent MCP service — without
        // waiting for a first conversation RPC.
        conversation_host::ensure_host_for_desktop_start();
        let stdin = io::stdin();
        return serve_stdio_rpc(stdin.lock(), io::stdout(), execute_rpc_cli).map(|_| ());
    }
    if args.as_slice() == ["rpc", "data-home"] {
        panic::set_hook(Box::new(|_| {
            eprintln!("licoup data-home operation terminated unexpectedly");
        }));
        let stdin = io::stdin();
        return serve_data_home_stdio_rpc(stdin.lock(), io::stdout(), execute_rpc_cli).map(|_| ());
    }
    if args.as_slice() == ["rpc", "conversation"] {
        panic::set_hook(Box::new(|_| {
            eprintln!("licoup conversation RPC proxy terminated unexpectedly");
        }));
        return conversation_host::serve_proxy();
    }
    if args.as_slice() == ["rpc", "conversation-host", "--stop"]
        || args.as_slice() == ["rpc", "conversation-host", "stop"]
        || args.as_slice() == ["rpc", "conversation-host", "--shutdown"]
        || args.as_slice() == ["rpc", "conversation-host", "shutdown"]
    {
        return conversation_host::request_host_stop();
    }
    if args.as_slice() == ["rpc", "conversation-host"] {
        panic::set_hook(Box::new(|_| {
            eprintln!("licoup conversation RPC host terminated unexpectedly");
        }));
        return conversation_host::serve_host();
    }
    let args = materialize_private_stdin_json(args, io::stdin().lock())?;
    match execute_local_cli(args)? {
        licoup_native::ffi::commands::CliExecution::Usage => print_usage(),
        licoup_native::ffi::commands::CliExecution::Json(value) => print_json(&value),
        licoup_native::ffi::commands::CliExecution::Streamed => {}
    }
    Ok(())
}

/// Compose native command admission with the persistent host without Flutter.
fn execute_local_cli(args: Vec<String>) -> Result<licoup_native::ffi::commands::CliExecution> {
    use licoup_native::ffi::commands::{admit_cli_command, execute_cli, native_rpc};
    if matches!(args.as_slice(), [value] if matches!(value.as_str(), "help" | "--help" | "-h")) {
        return execute_cli(args);
    }
    let command = admit_cli_command(args)?;
    if let Some((request, output)) = native_rpc::request_for_command(&command)? {
        let stream =
            conversation_host::connect_for_cli(command.option_flag("require-running-host"))?;
        return native_rpc::execute_host_call(stream, request, io::stdout().lock(), output);
    }
    let requires_host = matches!(
        command.path(),
        ["subagents", "execute"] | ["conversation", "list"] | ["mcp", "start" | "reload"]
    ) || (command.path() == ["conversation", "get"]
        && command
            .option_text("conversation-id")
            .is_some_and(|value| !value.trim().is_empty()));
    if requires_host {
        drop(conversation_host::connect_for_cli(false)?);
    }
    command.execute()
}

#[cfg(test)]
#[path = "licoup/tests.rs"]
mod tests;
