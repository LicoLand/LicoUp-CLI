//! The thirteen per-Agent halves this host composes.
//!
//! Each subtree below is one Agent's driver protocol and moves to that Agent's
//! crate (`licoup-agent-<agent>`); this file is the composition, and it travels
//! last, because it is what tilts from naming thirteen drivers to naming the
//! crates that hold them. Until then the moved crate reads every one of them
//! through its port and names none of them.
//!
//! The normalization each arm ends in is the host's own vocabulary and stays in
//! `licoup-agent-drivers`; the composition calls it because it is the one place
//! that holds both an Agent's execution and the host's normalization in view.

use licoup_agent_adapter_sdk::port::ParserRegistration;
use licoup_agent_drivers::AcpParserRegistration;
use licoup_agent_drivers::runtime_adapters::adapter::RuntimeAdapter;
use licoup_agent_drivers::runtime_adapters::model::NormalizedExecution;
use licoup_agent_drivers::runtime_adapters::normalization::{
    normalize_acp, normalize_antigravity, normalize_claude, normalize_codex, normalize_cursor,
    normalize_deepseek_harness, normalize_hermes, normalize_lico_agent, normalize_openclaw,
    normalize_pi,
};
use licoup_agent_drivers::runtime_adapters::port::{
    AcpCapabilityFacts, AgentDriverRegistration, AgentRun, DrivenRun, DriverEffectiveSettings,
    DriverFailureFacts, collaboration_acp_servers,
};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::OnceLock;

use crate::platform::acp_driver_runtime;
// The DeepSeek Harness driver — the `--profile sdk` transport, the turn it
// carries and the cleanup of one session — is the DeepSeek Harness package's.
// The composition names the package and keeps the host's own projection of its
// result; it holds no protocol frame, no request builder and no transport of its
// own.
use licoup_agent_deepseek::driver as deepseek_harness_driver;
// The Hermes driver — the `hermes acp` launch, the fixed capability probes and
// one bounded turn over the shared persistent ACP transport — is the Hermes
// package's. The composition names the package and keeps the host's own
// projection of its result; it holds no launch argument, no probe command and no
// transport of its own. Hermes reaches its target through two protocols, and
// both are the package's: the Gateway lane the composition picks where the
// runtime connection is in view is `tui_gateway_driver`, and the history page
// over the same connection is named by the host's history lane.
use licoup_agent_hermes::driver as hermes_driver;
use licoup_agent_hermes::tui_gateway_driver as hermes_tui_gateway_driver;
// The OpenClaw driver — the Gateway attach it names, the ACP bridge it spawns,
// the framed lines it reads and the turn it supervises — is the OpenClaw
// package's. The composition names the package and keeps the host's own
// projection of its result; it holds no attach endpoint, no frame rule and no
// turn phase of its own. The one fact the host still answers at the call site is
// which MCP servers an OpenClaw turn registers, because that is read from the
// user's collaboration-plugin configuration and the package may not reach it.
use licoup_agent_openclaw::driver as openclaw_driver;
// The Claude Code driver — the supervised streaming-input CLI, the frames it
// classifies and the turn it reports — is the Claude Code package's. The
// composition names the package and keeps the host's own projection of its
// result; it holds no launch field, no frame dialect and no turn phase of its
// own.
use licoup_agent_claude_code::driver as claude_code_driver;
// The Antigravity driver — the `agy --print` launch, the PTY turn it
// supervises, the Stop-hook receipt it harvests and the terminal classification
// it reports — is the Antigravity package's. The composition names the package
// and keeps the host's own projection of its result; it holds no launch
// metadata, no argv field, no receipt rule and no turn phase of its own.
use licoup_agent_antigravity::driver as antigravity_driver;
// The Copilot driver — the `--acp --stdio` launch, the probe and the turn it
// runs — is the Copilot package's. The composition names the package and keeps
// the host's own projection of its result; it holds no launch declaration and
// no ACP phase of its own.
use licoup_agent_copilot::driver as copilot_driver;
// The Kilo Code driver — the `serve` launch, the endpoint probe and the turn it
// performs — is the Kilo Code package's. The composition names the package and
// keeps the host's own projection of its result; it holds no launch declaration,
// no endpoint policy and no turn phase of its own.
use licoup_agent_kilo::driver as kilo_code_driver;
// The Cursor driver — the `cursor-agent` launch on the host's pty, the
// strict-NDJSON stream it classifies, the chat storage it retires and the
// outcome it reports — is the Cursor package's. The composition names the
// package and keeps the host's own projection of its result; it holds no launch
// argument, no session rule and no turn phase of its own.
use licoup_agent_cursor::driver as cursor_driver;
// The Kimi Code driver — the `kimi acp` launch, the frames it classifies and
// the outcome it reports — is the Kimi Code package's. The composition names the
// package and keeps the host's own projection of its result; it holds no launch
// metadata, no ACP dialect and no turn phase of its own.
use licoup_agent_kimi::driver as kimi_code_driver;
// The Lico Agent driver — the `--mode rpc` launch, the stdio JSONL exchange it
// supervises and the outcome it reports — is the Lico Agent package's. The
// composition names the package and keeps the host's own projection of its
// result; it holds no RPC frame, no session layout and no sandbox profile of its
// own.
use licoup_agent_lico_agent::driver as lico_agent_driver;
// The OpenCode driver — the serve endpoint's launch, the session-open protocol
// and the turn it runs — is the OpenCode package's. The composition names the
// package and keeps the host's own projection of its result; it holds no launch
// declaration, no document shape and no stream rule of its own.
use licoup_agent_opencode::driver as opencode_driver;
// The Codex driver — the app-server process and the protocol it speaks — is the
// Codex package's. The composition names the package and keeps the host's own
// projection of its result; it holds no app-server field, no launch and no
// protocol phase of its own.
use licoup_agent_codex::app_server::contract::RUNTIME_PROTOCOL as CODEX_RUNTIME_PROTOCOL;
use licoup_agent_codex::app_server::driver as codex_driver;
use licoup_agent_codex::app_server::model::RunResult as CodexRunResult;
// The Pi driver — the `pi --mode rpc --offline` launch, the JSONL frames it
// classifies and the turn it runs — is the Pi package's. The composition names
// the package and keeps the host's own projection of its result; it holds no
// launch argument, no RPC frame rule and no turn phase of its own.
use licoup_agent_pi::driver as pi_driver;

/// Project one Agent's own driver failure onto the host's protocol-agnostic
/// failure facts.
///
/// It is `pub(crate)` because the host's own suites build an Agent's raw result
/// and must project it the same way the composition does; a second projection
/// in a test would be a second answer to what an Agent reported.
///
/// `component` and `retryable` are per-Agent: three of the thirteen Agents
/// report a component and a retryability, and the other ten project both as
/// absent. The flags keep that projection verbatim rather than widening it.
macro_rules! failure_facts_crate {
    ($error:expr) => {
        $error.map(|failure| {
            let failure = failure.into_payload();
            DriverFailureFacts {
                code: failure.code.to_string(),
                message: failure.message.to_string(),
                stage: failure.stage.to_string(),
                component: None,
                retryable: None,
                recovery: None,
                user_interaction_required: failure.user_interaction_required,
                request_method: failure.request_method,
                session_id: failure.session_id,
                thread_id: failure.thread_id,
                turn_id: failure.turn_id,
                turn_status: failure.turn_status,
            }
        })
    };
    ($error:expr, session_thread) => {
        $error.map(|failure| {
            let failure = failure.into_payload();
            let thread_id = failure.session_id.clone();
            DriverFailureFacts {
                code: failure.code.to_string(),
                message: failure.message.to_string(),
                stage: failure.stage.to_string(),
                component: None,
                retryable: None,
                recovery: None,
                user_interaction_required: failure.user_interaction_required,
                request_method: failure.request_method,
                session_id: failure.session_id,
                thread_id,
                turn_id: failure.turn_id,
                turn_status: failure.turn_status,
            }
        })
    };
    ($error:expr, rich) => {
        $error.map(|failure| {
            let failure = failure.into_payload();
            DriverFailureFacts {
                code: failure.code.to_string(),
                message: failure.message.to_string(),
                stage: failure.stage.to_string(),
                component: failure.component.map(str::to_owned),
                retryable: failure.retryable,
                recovery: failure.recovery.map(str::to_owned),
                user_interaction_required: failure.user_interaction_required,
                request_method: failure.request_method,
                session_id: failure.session_id,
                thread_id: failure.thread_id,
                turn_id: failure.turn_id,
                turn_status: failure.turn_status,
            }
        })
    };
}

/// The registration of every Agent this host composes, in packaged inventory
/// order.
///
/// The parser half of each entry is the same `ParserRegistration` the host
/// injects into `licoup-agent-adapter-sdk`, read from the one set
/// `native_agent_parser` composes, so an Agent's declaration and its
/// protocol-agnostic answers cannot drift apart.
pub(crate) use failure_facts_crate as failure_facts;

/// The parser registration one Agent reports, from the one set this host
/// injects into the adapter SDK.
pub(super) fn parser_for_agent(agent_id: &str) -> ParserRegistration {
    crate::platform::native_agent_parser::parser_set()
        .registration(agent_id)
        .expect("the dispatch enum and the composed parser set are one set")
}

/// The ACP frame dialects this host composes, keyed by driver identity.
///
/// The ACP transport engines in `licoup-agent-drivers` read one Agent's frames
/// — how a line decodes, what a frame means, how a failure is worded — through
/// this table rather than by naming a parser, so that crate names no Agent and
/// an Agent that changes its dialect is a change to that Agent's package and to
/// this table, never to the transport.
///
/// The table is keyed by **driver identity** rather than by Agent, because two
/// Agents legitimately share one ACP dialect. Measured on the parsers here:
/// `copilot`, `opencode` and `kilo-code` all select the same Copilot-profile
/// dialect. Copilot's, Kimi Code's and Hermes' entries are not restated here at
/// all: each package owns its dialect and publishes it whole, so this
/// composition installs the package's registration rather than assembling a
/// second copy of it.
///
/// The dialect an Agent's own package carries is named by that package rather
/// than rebuilt here: Copilot's entry is the constant `licoup-agent-copilot`
/// registers, and Kimi Code's and Hermes' are the registrations their own
/// packages publish, so the frame policy the transport reads is the one the
/// package ships. No parser path is imported for any of the three.
pub(super) fn acp_dialects() -> &'static [AcpParserRegistration] {
    static DIALECTS: OnceLock<Vec<AcpParserRegistration>> = OnceLock::new();
    DIALECTS.get_or_init(|| {
        vec![
            // The Copilot package's own dialect, installed rather than
            // restated: the package owns the parser behind the frame policy and
            // the request projection it needs, so the two cannot drift.
            licoup_agent_copilot::registration::DIALECT,
            // The Kimi Code package's own dialect, installed rather than
            // restated: the package owns the parser behind it, so the two cannot
            // drift.
            licoup_agent_kimi::dialect::registration(),
            // The Hermes package's own dialect, installed the same way. Hermes is
            // the one Agent on the persistent ACP dialect, and its entry borrows
            // nothing: every member is the package's own parser function.
            licoup_agent_hermes::dialect::registration(),
        ]
    })
}

pub(super) fn registrations() -> &'static [AgentDriverRegistration] {
    static REGISTRATIONS: OnceLock<Vec<AgentDriverRegistration>> = OnceLock::new();
    REGISTRATIONS.get_or_init(|| {
        let parsers = crate::platform::native_agent_parser::adapters::REGISTRATIONS;
        vec![
            AgentDriverRegistration {
                agent_id: "antigravity",
                driver_id: antigravity_driver::DRIVER_ID,
                runtime_protocol: antigravity_driver::RUNTIME_PROTOCOL,
                probe: probe_antigravity,
                run: run_antigravity,
                parser: parsers[0],
            },
            AgentDriverRegistration {
                agent_id: "claude-code",
                driver_id: "claude-code-stream-json",
                runtime_protocol: claude_code_driver::RUNTIME_PROTOCOL,
                probe: probe_claude_code,
                run: run_claude_code,
                parser: parsers[1],
            },
            AgentDriverRegistration {
                agent_id: "codex",
                driver_id: "codex-app-server",
                runtime_protocol: CODEX_RUNTIME_PROTOCOL,
                probe: probe_codex,
                run: run_codex,
                parser: parsers[2],
            },
            AgentDriverRegistration {
                agent_id: "copilot",
                driver_id: "copilot-acp",
                runtime_protocol: copilot_driver::RUNTIME_PROTOCOL,
                probe: probe_copilot,
                run: run_copilot,
                parser: parsers[3],
            },
            AgentDriverRegistration {
                agent_id: "cursor",
                driver_id: cursor_driver::DRIVER_ID,
                runtime_protocol: cursor_driver::RUNTIME_PROTOCOL,
                probe: probe_cursor,
                run: run_cursor,
                parser: parsers[4],
            },
            AgentDriverRegistration {
                agent_id: "hermes",
                // The dialect identity is the package's own declaration: the
                // transport resolves Hermes' frames by this identity, so a
                // second literal here could silently reach the fail-closed
                // dialect instead of the package's parser.
                driver_id: licoup_agent_hermes::dialect::DRIVER_ID,
                runtime_protocol: hermes_driver::RUNTIME_PROTOCOL,
                probe: probe_hermes,
                run: run_hermes,
                parser: parsers[5],
            },
            AgentDriverRegistration {
                agent_id: "kilo-code",
                driver_id: "kilo-code-serve",
                runtime_protocol: kilo_code_driver::RUNTIME_PROTOCOL,
                probe: probe_kilo_code,
                run: run_kilo_code,
                parser: parsers[6],
            },
            AgentDriverRegistration {
                agent_id: "kimi-code",
                driver_id: "kimi-code-acp",
                runtime_protocol: kimi_code_driver::RUNTIME_PROTOCOL,
                probe: probe_kimi_code,
                run: run_kimi_code,
                parser: parsers[7],
            },
            AgentDriverRegistration {
                agent_id: "openclaw",
                driver_id: "openclaw-acp",
                runtime_protocol: openclaw_driver::RUNTIME_PROTOCOL,
                probe: probe_openclaw,
                run: run_openclaw,
                parser: parsers[8],
            },
            AgentDriverRegistration {
                agent_id: "opencode",
                driver_id: "opencode-serve",
                runtime_protocol: opencode_driver::RUNTIME_PROTOCOL,
                probe: probe_opencode,
                run: run_opencode,
                parser: parsers[9],
            },
            AgentDriverRegistration {
                agent_id: "pi",
                driver_id: "pi-rpc",
                runtime_protocol: pi_driver::RUNTIME_PROTOCOL,
                probe: probe_pi,
                run: run_pi,
                parser: parsers[10],
            },
            AgentDriverRegistration {
                agent_id: "lico-agent",
                driver_id: "lico-agent-rpc",
                runtime_protocol: lico_agent_driver::RUNTIME_PROTOCOL,
                probe: probe_lico_agent,
                run: run_lico_agent,
                parser: parsers[11],
            },
            AgentDriverRegistration {
                agent_id: "deepseek-harness",
                driver_id: deepseek_harness_driver::DRIVER_ID,
                runtime_protocol: deepseek_harness_driver::RUNTIME_PROTOCOL,
                probe: probe_deepseek_harness,
                run: run_deepseek_harness,
                parser: parsers[12],
            },
        ]
    })
}

// ---------------------------------------------------------------------------
// Probes. Each arm is the probe the host ran before the tree moved, verbatim.
// ---------------------------------------------------------------------------

fn probe_antigravity(executable: &str, _cwd: &Path) -> Value {
    let probe = antigravity_driver::probe(executable, 2_000, 64 * 1024);
    json!({
        "available": probe.available,
        "supported": probe.supported,
        "stdinPrompt": probe.stdin_prompt,
        "structuredStream": probe.structured_stream,
        "newSession": probe.new_session,
        "resumeSession": probe.resume_session,
        "model": probe.model,
        "reasoningEffort": probe.reasoning_effort,
        "permissionMode": probe.permission_mode,
        "interactiveApprovalEvents": probe.interactive_approval_events,
        "versionCommandOk": probe.version_command_ok,
        "helpCommandOk": probe.help_command_ok,
        "errorCode": probe.error_code
    })
}

fn probe_claude_code(executable: &str, _cwd: &Path) -> Value {
    let probe = claude_code_driver::probe(executable, 2_000, 64 * 1024);
    json!({
        "available": probe.available,
        "supported": probe.available,
        "stdinPrompt": probe.stdin_prompt,
        "structuredStream": probe.structured_stream,
        "newSession": probe.new_session,
        "resumeSession": probe.resume_session,
        "model": probe.model,
        "reasoningEffort": probe.reasoning_effort,
        "permissionMode": probe.permission_mode,
        "interactiveApprovalEvents": probe.interactive_approval_events
    })
}

fn probe_codex(executable: &str, _cwd: &Path) -> Value {
    json!({
        "available": executable != "",
        "supported": true,
        "stdinPrompt": true,
        "structuredStream": true,
        "newSession": true,
        "resumeSession": true,
        "interactiveApprovalEvents": false
    })
}

fn probe_copilot(executable: &str, cwd: &Path) -> Value {
    probe_acp_runtime(copilot_driver::capability_probe(
        executable,
        cwd,
        2_000,
        Some(64 * 1024),
        16 * 1024,
    ))
}

fn probe_cursor(executable: &str, _cwd: &Path) -> Value {
    let probe = cursor_driver::probe(executable, 2_000, 64 * 1024);
    json!({
        "available": probe.available,
        "supported": probe.supported,
        "createChat": probe.create_chat,
        "printTurn": probe.print_turn,
        "resumeSession": probe.resume_session,
        "structuredStream": probe.structured_stream,
        "versionCommandOk": probe.version_command_ok,
        "helpCommandOk": probe.help_command_ok,
        "errorCode": probe.error_code
    })
}

fn probe_hermes(executable: &str, _cwd: &Path) -> Value {
    let probe = hermes_driver::probe(executable, 2_000, 64 * 1024);
    json!({
        "available": probe.available,
        "supported": probe.supported,
        "newSession": probe.supported,
        "resumeSession": probe.supported,
        "structuredStream": probe.supports_streaming,
        "tools": probe.supports_tools,
        "approvals": probe.supports_approvals,
        "modelOverride": probe.supports_model_override,
        "reasoningOverride": probe.supports_reasoning_override,
        "versionDetected": probe.version.is_some(),
        "errorCode": probe.error_code
    })
}

fn probe_kilo_code(executable: &str, cwd: &Path) -> Value {
    probe_acp_runtime(kilo_code_driver::capability_probe(
        executable,
        cwd,
        2_000,
        Some(64 * 1024),
        16 * 1024,
    ))
}

fn probe_kimi_code(executable: &str, cwd: &Path) -> Value {
    probe_acp_runtime(kimi_code_driver::capability_probe(
        executable,
        cwd,
        2_000,
        Some(64 * 1024),
        16 * 1024,
    ))
}

fn probe_openclaw(executable: &str, _cwd: &Path) -> Value {
    let probe = openclaw_driver::probe(executable, 2_000, 64 * 1024);
    json!({
        "available": probe.available,
        "supported": probe.supported,
        "newSession": probe.supported,
        "resumeSession": probe.supported,
        "structuredStream": probe.supports_streaming,
        "tools": probe.supports_tools,
        "approvals": probe.supports_approvals,
        "reasoning": probe.supports_reasoning,
        "modelOverride": probe.supports_model_override,
        "versionDetected": probe.version.is_some(),
        "errorCode": probe.error_code
    })
}

fn probe_opencode(executable: &str, cwd: &Path) -> Value {
    probe_acp_runtime(opencode_driver::capability_probe(
        executable,
        cwd,
        2_000,
        Some(64 * 1024),
        16 * 1024,
    ))
}

fn probe_pi(executable: &str, _cwd: &Path) -> Value {
    let probe = pi_driver::probe(executable, 2_000, 64 * 1024);
    json!({
        "available": probe.available,
        "supported": probe.supported,
        "newSession": probe.supported,
        "resumeSession": probe.supported,
        "structuredStream": probe.supported,
        "versionCommandOk": probe.version_command_ok,
        "helpCommandOk": probe.help_command_ok,
        "errorCode": probe.error_code
    })
}

fn probe_lico_agent(executable: &str, _cwd: &Path) -> Value {
    let probe = lico_agent_driver::probe(Path::new(executable));
    json!({
        "available": probe.available,
        "supported": probe.supported,
        "newSession": probe.supported,
        "resumeSession": probe.supported,
        "structuredStream": probe.supported,
        "versionCommandOk": probe.version_command_ok,
        "helpCommandOk": probe.help_command_ok,
        "errorCode": probe.error_code
    })
}

fn probe_deepseek_harness(executable: &str, _cwd: &Path) -> Value {
    let available = !executable.is_empty();
    // Discovery has no selected route or authentication handshake. The
    // actual SDK initialize validates both when the user sends a turn.
    json!({
        "available": available,
        "supported": false,
        "newSession": false,
        "resumeSession": false,
        "structuredStream": false,
        "cancel": false,
        "interruptSteer": false,
        "history": false,
        "errorCode": if available {
            "deepseek_harness_initialize_required"
        } else {
            "runtime_not_detected"
        }
    })
}

fn probe_acp_runtime(
    result: std::result::Result<
        acp_driver_runtime::CapabilityProbe,
        acp_driver_runtime::ProtocolFailure,
    >,
) -> Value {
    match result {
        Ok(probe) => json!({
            "available": true,
            "supported": probe.protocol_version == Some(1),
            "protocolVersion": probe.protocol_version,
            "loadSession": probe.load_session,
            "resumeSession": probe.resume_session,
            "closeSession": probe.close_session,
            "listSessions": probe.list_sessions,
            "deleteSession": probe.delete_session,
            "imagePrompts": probe.image_prompts,
            "audioPrompts": probe.audio_prompts,
            "embeddedContext": probe.embedded_context
        }),
        Err(failure) => json!({
            "available": false,
            "supported": false,
            "errorCode": failure.code
        }),
    }
}

// ---------------------------------------------------------------------------
// Executions. Each arm runs the driver the host ran before the tree moved, with
// the same arguments in the same order, and projects its own result onto the
// host's protocol-agnostic shape. The host then normalizes it, so the
// per-Agent pipeline is: this Agent's execution, then the host's report.
// ---------------------------------------------------------------------------

fn run_antigravity(run: &AgentRun<'_>) -> NormalizedExecution {
    let result = antigravity_driver::execute(
        run.executable,
        run.params,
        run.prompt,
        run.session_id,
        run.cwd,
        run.timeout_ms,
        run.max_stdout,
        run.max_stderr,
    );
    normalize_antigravity(DrivenRun {
        ok: result.ok,
        output: result.output,
        transitions: result.transitions,
        error: failure_facts!(result.error),
        session_id: result.session_id,
        thread_id: result.thread_id,
        turn_id: result.turn_id,
        turn_status: result.turn_status,
        effective: DriverEffectiveSettings {
            cwd: result.effective.cwd,
            model: result.effective.model,
            reasoning_effort: result.effective.reasoning_effort,
            permission_mode: result.effective.permission_mode,
            sandbox: result.effective.sandbox,
            approval_policy: result.effective.approval_policy,
            ..DriverEffectiveSettings::default()
        },
        acp_capabilities: None,
        status_code: result.status_code,
        stdout_truncated: result.stdout_truncated,
        stderr_truncated: result.stderr_truncated,
        started_at: result.started_at,
        runtime_protocol: antigravity_driver::RUNTIME_PROTOCOL,
        driver_id: antigravity_driver::DRIVER_ID,
    })
}

fn run_claude_code(run: &AgentRun<'_>) -> NormalizedExecution {
    let result = claude_code_driver::execute(
        run.executable,
        run.params,
        run.prompt,
        run.session_id,
        run.cwd,
        run.timeout_ms,
        run.max_stdout,
        run.max_stderr,
    );
    normalize_claude(DrivenRun {
        ok: result.ok,
        output: result.output,
        transitions: result.transitions,
        error: failure_facts!(result.error),
        session_id: result.session_id,
        thread_id: result.thread_id,
        turn_id: result.turn_id,
        turn_status: result.turn_status,
        effective: DriverEffectiveSettings {
            cwd: result.effective.cwd,
            model: result.effective.model,
            reasoning_effort: result.effective.reasoning_effort,
            permission_mode: result.effective.permission_mode,
            sandbox: result.effective.sandbox,
            approval_policy: result.effective.approval_policy,
            ..DriverEffectiveSettings::default()
        },
        acp_capabilities: None,
        status_code: result.status_code,
        stdout_truncated: result.stdout_truncated,
        stderr_truncated: result.stderr_truncated,
        started_at: result.started_at,
        runtime_protocol: claude_code_driver::RUNTIME_PROTOCOL,
        driver_id: "claude-code-stream-json",
    })
}

/// Project one Codex driver result onto the host's protocol-agnostic shape.
pub(in crate::platform) fn codex_driven(result: CodexRunResult) -> DrivenRun {
    DrivenRun {
        ok: result.ok,
        output: result.output,
        transitions: result.transitions,
        error: failure_facts!(result.error, rich),
        session_id: result.session_id,
        thread_id: result.thread_id,
        turn_id: result.turn_id,
        turn_status: result.turn_status,
        effective: DriverEffectiveSettings {
            cwd: result.effective.cwd,
            model: result.effective.model,
            reasoning_effort: result.effective.reasoning_effort,
            sandbox: result.effective.sandbox,
            approval_policy: result.effective.approval_policy,
            ..DriverEffectiveSettings::default()
        },
        acp_capabilities: None,
        status_code: result.status_code,
        stdout_truncated: result.stdout_truncated,
        stderr_truncated: result.stderr_truncated,
        started_at: result.started_at,
        runtime_protocol: CODEX_RUNTIME_PROTOCOL,
        driver_id: "codex-app-server",
    }
}

fn run_codex(run: &AgentRun<'_>) -> NormalizedExecution {
    let result = codex_driver::execute(
        run.executable,
        run.params,
        run.prompt,
        run.session_id,
        run.cwd,
        run.timeout_ms,
        run.max_stdout,
        run.max_stderr,
        Some(crate::platform::codex_app_server_environment()),
    );
    normalize_codex(codex_driven(result))
}

/// Project one Cursor driver result onto the host's protocol-agnostic shape.
pub(in crate::platform) fn cursor_driven(result: cursor_driver::RunResult) -> DrivenRun {
    DrivenRun {
        ok: result.ok,
        output: result.output,
        transitions: result.transitions,
        error: failure_facts!(result.error, rich),
        session_id: result.session_id,
        thread_id: result.thread_id,
        turn_id: result.turn_id,
        turn_status: result.turn_status,
        effective: DriverEffectiveSettings {
            cwd: result.effective.cwd,
            model: result.effective.model,
            reasoning_effort: result.effective.reasoning_effort,
            permission_mode: result.effective.permission_mode,
            sandbox: result.effective.sandbox,
            approval_policy: result.effective.approval_policy,
            ..DriverEffectiveSettings::default()
        },
        acp_capabilities: None,
        status_code: result.status_code,
        stdout_truncated: result.stdout_truncated,
        stderr_truncated: result.stderr_truncated,
        started_at: result.started_at,
        runtime_protocol: cursor_driver::RUNTIME_PROTOCOL,
        driver_id: cursor_driver::DRIVER_ID,
    }
}

/// The four Agents on the shared ACP engine share one arm shape.
macro_rules! acp_run {
    ($adapter:expr, $execute:path, $result:ident, $run:ident) => {{
        let $result = $execute(
            $run.executable,
            $run.params,
            $run.prompt,
            $run.session_id,
            $run.cwd,
            $run.timeout_ms,
            $run.max_stdout,
            $run.max_stderr,
        );
        normalize_acp(
            $adapter,
            DrivenRun {
                ok: $result.ok,
                output: $result.output,
                transitions: $result.transitions,
                error: failure_facts!($result.error),
                session_id: $result.session_id,
                thread_id: $result.thread_id,
                turn_id: $result.turn_id,
                turn_status: $result.turn_status,
                effective: DriverEffectiveSettings {
                    cwd: $result.effective.cwd,
                    model: $result.effective.model,
                    reasoning_effort: $result.effective.reasoning_effort,
                    mode: $result.effective.mode,
                    runtime_agent: $result.effective.runtime_agent,
                    allow_all: $result.effective.allow_all,
                    sandbox: $result.effective.sandbox,
                    approval_policy: $result.effective.approval_policy,
                    ..DriverEffectiveSettings::default()
                },
                acp_capabilities: Some(AcpCapabilityFacts {
                    protocol_version: $result.capabilities.protocol_version,
                    load_session: $result.capabilities.load_session,
                    resume_session: $result.capabilities.resume_session,
                    close_session: $result.capabilities.close_session,
                    list_sessions: $result.capabilities.list_sessions,
                    delete_session: $result.capabilities.delete_session,
                    image_prompts: $result.capabilities.image_prompts,
                    audio_prompts: $result.capabilities.audio_prompts,
                    embedded_context: $result.capabilities.embedded_context,
                }),
                status_code: $result.status_code,
                stdout_truncated: $result.stdout_truncated,
                stderr_truncated: $result.stderr_truncated,
                started_at: $result.started_at,
                runtime_protocol: $result.runtime_protocol,
                driver_id: $result.driver_id,
            },
        )
    }};
}

fn run_copilot(run: &AgentRun<'_>) -> NormalizedExecution {
    acp_run!(
        RuntimeAdapter::Copilot,
        copilot_driver::execute,
        result,
        run
    )
}

fn run_kilo_code(run: &AgentRun<'_>) -> NormalizedExecution {
    acp_run!(
        RuntimeAdapter::KiloCode,
        kilo_code_driver::execute,
        result,
        run
    )
}

fn run_kimi_code(run: &AgentRun<'_>) -> NormalizedExecution {
    acp_run!(
        RuntimeAdapter::KimiCode,
        kimi_code_driver::execute,
        result,
        run
    )
}

fn run_opencode(run: &AgentRun<'_>) -> NormalizedExecution {
    acp_run!(
        RuntimeAdapter::OpenCode,
        opencode_driver::execute,
        result,
        run
    )
}

fn run_cursor(run: &AgentRun<'_>) -> NormalizedExecution {
    let result = cursor_driver::execute(
        run.executable,
        run.params,
        run.prompt,
        run.session_id,
        run.cwd,
        run.timeout_ms,
        run.max_stdout,
        run.max_stderr,
    );
    normalize_cursor(cursor_driven(result))
}

fn run_hermes(run: &AgentRun<'_>) -> NormalizedExecution {
    // The lane is the host's fact, not the Agent's: a turn bound to a Hermes
    // TUI gateway speaks the gateway's protocol, and a local turn speaks the
    // package's ACP transport. The host picks it here, where the runtime
    // connection is in view, and hands it to the Agent's parser and the host's
    // normalization. Both protocols are the package's, so the choice is this
    // composition's and the package is asked for the lane it owns.
    let gateway_connection = run
        .runtime_connection
        .filter(|connection| connection.is_hermes_tui_gateway());
    let runtime_protocol = if gateway_connection.is_some() {
        licoup_agent_hermes::tui_gateway::RUNTIME_PROTOCOL
    } else {
        hermes_driver::RUNTIME_PROTOCOL
    };
    let result = match gateway_connection {
        Some(connection) => hermes_tui_gateway_driver::execute(
            connection,
            run.params,
            run.prompt,
            run.session_id,
            run.cwd,
            run.timeout_ms,
            run.max_stdout,
            run.max_stderr,
        ),
        // A connection that is not this gateway — including none at all — is
        // the shared ACP transport's, read through the package's own entry.
        None => hermes_driver::execute_with_connection(
            run.executable,
            run.runtime_connection,
            run.params,
            run.prompt,
            run.session_id,
            run.cwd,
            run.timeout_ms,
            run.max_stdout,
            run.max_stderr,
        ),
    };
    let parser = super::registrations_parser("hermes");
    normalize_hermes(
        DrivenRun {
            ok: result.ok,
            output: result.output,
            // Hermes reports no transition list of its own; its registered
            // parser derives them from the outcome through the shared port.
            transitions: Vec::new(),
            error: failure_facts!(result.error, session_thread),
            session_id: result.session_id,
            thread_id: result.thread_id,
            turn_id: result.turn_id,
            turn_status: result.turn_status,
            effective: DriverEffectiveSettings {
                cwd: result.effective.cwd,
                model: result.effective.model,
                reasoning_effort: result.effective.reasoning_effort,
                sandbox: result.effective.sandbox,
                approval_policy: result.effective.approval_policy,
                ..DriverEffectiveSettings::default()
            },
            acp_capabilities: None,
            status_code: result.status_code,
            stdout_truncated: result.stdout_truncated,
            stderr_truncated: result.stderr_truncated,
            started_at: result.started_at,
            runtime_protocol,
            driver_id: licoup_agent_hermes::dialect::DRIVER_ID,
        },
        parser,
    )
}

fn run_openclaw(run: &AgentRun<'_>) -> NormalizedExecution {
    let result = openclaw_driver::execute_with_connection(
        run.executable,
        run.runtime_connection,
        || {
            collaboration_acp_servers("openclaw").map_err(|_| {
                openclaw_driver::ProtocolFailure::new(
                    "openclaw_acp_mcp_registration_invalid",
                    "The optional MCP registration could not be validated safely.",
                    "session/mcp",
                )
            })
        },
        run.params,
        run.prompt,
        run.session_id,
        run.cwd,
        run.timeout_ms,
        run.max_stdout,
        run.max_stderr,
    );
    normalize_openclaw(DrivenRun {
        ok: result.ok,
        output: result.output,
        transitions: result.transitions,
        error: failure_facts!(result.error, session_thread),
        session_id: result.session_id,
        thread_id: result.thread_id,
        turn_id: result.turn_id,
        turn_status: result.turn_status,
        effective: DriverEffectiveSettings {
            cwd: result.effective.cwd,
            model: result.effective.model,
            reasoning_effort: result.effective.reasoning_effort,
            sandbox: result.effective.sandbox,
            approval_policy: result.effective.approval_policy,
            ..DriverEffectiveSettings::default()
        },
        acp_capabilities: None,
        status_code: result.status_code,
        stdout_truncated: result.stdout_truncated,
        stderr_truncated: result.stderr_truncated,
        started_at: result.started_at,
        runtime_protocol: openclaw_driver::RUNTIME_PROTOCOL,
        driver_id: "openclaw-acp",
    })
}

fn run_pi(run: &AgentRun<'_>) -> NormalizedExecution {
    let result = pi_driver::execute(
        run.executable,
        run.params,
        run.prompt,
        run.session_id,
        run.cwd,
        run.timeout_ms,
        run.max_stdout,
        run.max_stderr,
    );
    normalize_pi(DrivenRun {
        ok: result.ok,
        output: result.output,
        transitions: result.transitions,
        error: failure_facts!(result.error, session_thread),
        session_id: result.session_id,
        thread_id: result.thread_id,
        turn_id: result.turn_id,
        turn_status: result.turn_status,
        effective: DriverEffectiveSettings {
            cwd: result.effective.cwd,
            model: result.effective.model,
            reasoning_effort: result.effective.reasoning_effort,
            permission_mode: result.effective.permission_mode,
            sandbox: result.effective.sandbox,
            approval_policy: result.effective.approval_policy,
            ..DriverEffectiveSettings::default()
        },
        acp_capabilities: None,
        status_code: result.status_code,
        stdout_truncated: result.stdout_truncated,
        stderr_truncated: result.stderr_truncated,
        started_at: result.started_at,
        runtime_protocol: pi_driver::RUNTIME_PROTOCOL,
        driver_id: "pi-rpc",
    })
}

fn run_lico_agent(run: &AgentRun<'_>) -> NormalizedExecution {
    let result = lico_agent_driver::execute(
        run.executable,
        run.params,
        run.prompt,
        run.session_id,
        run.cwd,
        run.timeout_ms,
        run.max_stdout,
        run.max_stderr,
    );
    normalize_lico_agent(DrivenRun {
        ok: result.ok,
        output: result.output,
        transitions: result.transitions,
        error: failure_facts!(result.error, session_thread),
        session_id: result.session_id,
        thread_id: result.thread_id,
        turn_id: result.turn_id,
        turn_status: result.turn_status,
        effective: DriverEffectiveSettings {
            cwd: result.effective.cwd,
            model: result.effective.model,
            reasoning_effort: result.effective.reasoning_effort,
            permission_mode: result.effective.permission_mode,
            sandbox: result.effective.sandbox,
            approval_policy: result.effective.approval_policy,
            ..DriverEffectiveSettings::default()
        },
        acp_capabilities: None,
        status_code: result.status_code,
        stdout_truncated: result.stdout_truncated,
        stderr_truncated: result.stderr_truncated,
        started_at: result.started_at,
        runtime_protocol: lico_agent_driver::RUNTIME_PROTOCOL,
        driver_id: "lico-agent-rpc",
    })
}

fn run_deepseek_harness(run: &AgentRun<'_>) -> NormalizedExecution {
    let result = deepseek_harness_driver::execute(
        run.executable,
        run.params,
        run.prompt,
        run.session_id,
        run.cwd,
        run.timeout_ms,
        run.max_stdout,
        run.max_stderr,
    );
    normalize_deepseek_harness(DrivenRun {
        ok: result.ok,
        output: result.output,
        transitions: result.transitions,
        error: failure_facts!(result.error),
        session_id: result.session_id,
        thread_id: result.thread_id,
        turn_id: result.turn_id,
        turn_status: result.turn_status,
        effective: DriverEffectiveSettings {
            cwd: result.effective.cwd,
            model: result.effective.model,
            reasoning_effort: result.effective.reasoning_effort,
            permission_mode: result.effective.permission_mode,
            sandbox: result.effective.sandbox,
            approval_policy: result.effective.approval_policy,
            ..DriverEffectiveSettings::default()
        },
        acp_capabilities: None,
        status_code: result.status_code,
        stdout_truncated: result.stdout_truncated,
        stderr_truncated: result.stderr_truncated,
        started_at: result.started_at,
        runtime_protocol: deepseek_harness_driver::RUNTIME_PROTOCOL,
        driver_id: deepseek_harness_driver::DRIVER_ID,
    })
}
