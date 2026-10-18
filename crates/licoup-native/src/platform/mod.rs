// The ACP engines and the local-service control plane live in
// `licoup-agent-drivers`; these paths stay reachable at the visibility this
// host exposed before they moved.
pub(crate) use licoup_agent_drivers::{acp_driver_runtime, acp_session_transport};
// The bounded-workspace selection is a foundation primitive; this path stays
// reachable at the visibility this host exposed before it moved.
pub use licoup_foundation::platform::agent_workspace;
// The turn-event bus, the raw-execution observer and the native interaction
// registry are foundation primitives; these paths stay reachable at the
// visibility this host exposed before they moved.
pub use licoup_foundation::platform::native_agent_interaction;
pub use licoup_foundation::platform::raw_execution;
pub use licoup_foundation::platform::turn_event_emit;
#[cfg_attr(target_os = "linux", allow(dead_code))]
pub mod authorized_secure_record;
pub(crate) mod badtower_station;
pub(crate) mod conversation_lane;
pub mod diagnostics;
pub mod extension_host;
pub mod extension_packages;
pub(crate) mod generic_cli_driver;
pub(crate) mod kilo_code_host;
pub(crate) mod lico_agent_host;
pub mod package_registration_release;
pub(crate) use licoup_agent_drivers::local_service;
pub(crate) mod mcp_approval_plan_store;
pub(crate) mod mcp_streamable_http;
mod native_agent_parser;
pub(crate) mod openclaw_host;
pub(crate) mod opencode_host;
pub mod process_sandbox;
pub(crate) mod provider_mcp_registration;
pub(crate) mod remote_acp_history;
pub(crate) mod secure_mesh_mls_store;
pub mod stop_control;
pub(crate) mod strategy_runtime;
pub(crate) mod user_presence;
pub mod user_shell_environment;
pub(crate) mod virtual_machine;
pub mod work_context_ports;

pub mod antigravity_subagent_mcp_manager;
pub mod catalog_cache_store;
pub mod claude_code_subagent_mcp_manager;
pub mod client_autostart;
pub mod client_state;
pub mod conversation_host_client;
pub mod conversation_host_transport;
pub mod cursor_subagent_mcp_manager;
pub mod data_home_relocation;
pub mod gateway_composition;
pub mod gateway_runtime;
pub mod llm_api_key_vault;
pub mod llm_gateway_autostart;
pub mod llm_gateway_service;
pub mod mcp_service_process;
pub mod openclaw_gateway;
pub mod opencode_serve;
pub mod runtime_adapters;
pub mod secure_mesh_capability_probe;
pub mod secure_mesh_secret_store;
pub mod subagent_mcp_ensure;
pub mod telegram_channel_service;

pub use acp_session_transport::resolve_interaction_approval as resolve_native_agent_interaction_approval;
pub use native_agent_interaction::resolve as resolve_native_agent_interaction;
pub use native_agent_interaction::resolve_scoped as resolve_scoped_native_agent_interaction;
pub use turn_event_emit::{
    StreamSinkGuard, clear_stream_sink, emit_agent_message_chunk, emit_agent_message_completed,
    emit_agent_processing, emit_turn_event, install_stdout_ndjson_sink, install_stream_sink,
};

/// This host's answer for the Antigravity adapter package's turn-event port.
///
/// The package owns *what* one Antigravity turn emits; this host owns *where* it
/// goes, because the host owns the consumer. The answer is this host's own
/// emitters rather than a second sink, so an Antigravity event and a Cursor
/// event reach the same reader through the same path. The package emits no tool
/// failure of its own, so it declares no such sink.
pub(crate) fn antigravity_turn_event_port()
-> licoup_agent_antigravity::port::turn_event::TurnEventPort {
    licoup_agent_antigravity::port::turn_event::TurnEventPort {
        emit_turn_event,
        emit_agent_message_chunk,
        emit_agent_message_completed,
        emit_agent_processing,
    }
}

/// This host's answer for the Codex adapter package's turn-event port.
///
/// The package owns *what* one Codex turn emits; this host owns *where* it goes,
/// because the host owns the consumer. The answer is this host's own emitter
/// rather than a second sink, so a Codex event and a Cursor event reach the same
/// reader through the same path.
pub(crate) fn codex_turn_event_port() -> licoup_agent_codex::port::turn_event::TurnEventPort {
    licoup_agent_codex::port::turn_event::TurnEventPort {
        emit: emit_turn_event,
    }
}

/// This host's answer for the Pi adapter package's turn-event port.
///
/// The package owns *what* one Pi turn emits; this host owns *where* it goes,
/// because the host owns the consumer. The answer is this host's own emitters
/// rather than a second sink, so a Pi event and a Cursor event reach the same
/// reader through the same path.
pub(crate) fn pi_turn_event_port() -> licoup_agent_pi::port::turn_event::TurnEventPort {
    licoup_agent_pi::port::turn_event::TurnEventPort {
        emit_turn_event,
        emit_agent_message_chunk,
        emit_agent_message_completed,
        emit_agent_processing,
    }
}

/// This host's answer for the Cursor adapter package's turn-event port.
///
/// The package owns *what* one Cursor turn emits — the accepted turn, the
/// streamed chunks, the tool observations and the auto-update phases; this host
/// owns *where* they go, because the host owns the consumer. The answer is this
/// host's own emitters rather than a second sink, so a Cursor event and a Pi
/// event reach one reader through one path. The pty the Cursor turn is launched
/// on is not answered here: it is the shared primitive both halves link.
pub(crate) fn cursor_turn_event_port() -> licoup_agent_cursor::port::turn_event::TurnEventPort {
    // Cursor is the one package that also reports a tool failure, and this
    // host's re-export list above carries the four emitters it shares with the
    // other arms; the fifth is named at its own module so the shared list stays
    // the shape the other arms read.
    licoup_agent_cursor::port::turn_event::TurnEventPort {
        emit_turn_event,
        emit_agent_message_chunk,
        emit_agent_message_completed,
        emit_agent_processing,
        emit_agent_tool_error: turn_event_emit::emit_agent_tool_error,
    }
}

/// The environment one Codex app-server child is launched with.
///
/// The user's own login shell is the default command authority (ADR 0007), so
/// the child is cleared to exactly that snapshot rather than inheriting this
/// process's environment. The package applies the set; this host owns where it
/// came from.
pub(crate) fn codex_app_server_environment() -> Vec<(String, String)> {
    user_shell_environment::snapshot()
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

// The bounded process owner moved to `licoup-foundation`. It is re-exported at
// its former path and former visibility, because the driver engines, the
// sandboxed execution helpers and the Agent inventory all supervise the same
// child processes and one implementation serves them all.
pub(crate) use licoup_foundation::platform::process_supervisor;
pub(crate) use licoup_foundation::platform::process_supervisor::{
    run_bounded_command_input, run_bounded_command_output, run_bounded_untrusted_agent_output,
};
// The pseudo-terminal transport moved to `licoup-foundation` with it, for the
// same reason: attaching a child to a pty is a primitive every Agent's CLI lane
// reuses, not any one Agent's protocol. This path stays reachable, at the
// visibility the host exposed before it moved, for the Cursor lane and the
// generic CLI lane that still open one from here.
#[cfg(unix)]
pub(crate) use licoup_foundation::platform::pty_transport;
