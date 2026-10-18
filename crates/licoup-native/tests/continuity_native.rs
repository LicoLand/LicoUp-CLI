//! Hermetic Codex/Pi work-context fixtures. No real agent, install, or existing adapter call.

use licoup_agent_runtime::work_context::{
    CapabilityProfile, ChildBinding, ContinuityFailureCode, HermeticProtocol,
    NativeCapabilitySupport, NativeControlRequest, NativeWorkContextKey, NativeWorkContextPort,
    OperationKind, ProtocolFamily, SessionPresence, WorkContextConfig, WorkContextRuntime,
    default_snapshot, protocol_methods, unavailable_work_context_port,
};

fn child() -> ChildBinding {
    ChildBinding {
        child_conversation_id: "conversation:child".into(),
        membership_id: "membership:child-assistant".into(),
        source_task_id: "goal:source-task".into(),
        parent_conversation_id: "conversation:parent".into(),
    }
}

fn config(knowledge: bool) -> WorkContextConfig {
    WorkContextConfig::child(child()).with_knowledge_injected(knowledge)
}

fn key(matter: &str) -> NativeWorkContextKey {
    NativeWorkContextKey {
        conversation_id: child().child_conversation_id,
        membership_id: child().membership_id,
        matter_id: matter.into(),
        generation: 1,
    }
}

fn runtime(
    family: ProtocolFamily,
    profile: CapabilityProfile,
    presence: SessionPresence,
) -> WorkContextRuntime {
    let protocol = match family {
        ProtocolFamily::Codex => HermeticProtocol::codex(profile),
        ProtocolFamily::Pi => HermeticProtocol::pi(profile),
    }
    .with_presence(presence)
    .with_knowledge_injected(profile == CapabilityProfile::Low);
    WorkContextRuntime::from_hermetic(protocol, config(profile == CapabilityProfile::Low))
}

#[test]
fn codex_and_pi_high_low_degrade_honestly() {
    let codex_high = default_snapshot(ProtocolFamily::Codex, CapabilityProfile::High);
    let codex_low = default_snapshot(ProtocolFamily::Codex, CapabilityProfile::Low);
    let pi_high = default_snapshot(ProtocolFamily::Pi, CapabilityProfile::High);
    let pi_low = default_snapshot(ProtocolFamily::Pi, CapabilityProfile::Low);
    assert_eq!(codex_high.exact_resume, NativeCapabilitySupport::Supported);
    assert_eq!(codex_high.fork, NativeCapabilitySupport::Unsupported);
    assert_eq!(
        codex_high.parallel_contexts,
        NativeCapabilitySupport::Supported
    );
    assert_eq!(
        codex_low.parallel_contexts,
        NativeCapabilitySupport::Unsupported
    );
    assert_eq!(
        codex_low.isolated_context,
        NativeCapabilitySupport::Unverified
    );
    assert_eq!(
        pi_high.parallel_contexts,
        NativeCapabilitySupport::Unsupported
    );
    assert_eq!(
        pi_low.exact_resume,
        NativeCapabilitySupport::TemporarilyUnavailable
    );
    assert_ne!(codex_high.steer, codex_low.steer);
    assert_ne!(pi_high.steer, pi_low.steer);
}

#[test]
fn codex_lost_resume_uses_thread_resume_then_explicit_start() {
    let port = runtime(
        ProtocolFamily::Codex,
        CapabilityProfile::High,
        SessionPresence::Lost,
    );
    assert_eq!(
        port.exact_resume(&key("matter:docs")).unwrap_err().code,
        ContinuityFailureCode::NativeBindingLost
    );
    let generation = port.rehydrate(&key("matter:docs")).unwrap();
    assert_eq!(generation, 2);
    let operations = port.operations().unwrap();
    let methods = protocol_methods(ProtocolFamily::Codex);
    assert_eq!(operations[0].protocol_method, methods.exact_resume);
    assert_eq!(operations[1].protocol_method, methods.start_new);
    assert_eq!(operations[0].protocol_method, "thread/resume");
    assert_eq!(operations[1].protocol_method, "thread/start");
    assert_ne!(operations[0].operation_id, operations[1].operation_id);
}

#[test]
fn pi_lost_resume_uses_session_resume_then_session_new() {
    let port = runtime(
        ProtocolFamily::Pi,
        CapabilityProfile::High,
        SessionPresence::Lost,
    );
    assert_eq!(
        port.exact_resume(&key("matter:docs")).unwrap_err().code,
        ContinuityFailureCode::NativeBindingLost
    );
    let _ = port.rehydrate(&key("matter:docs")).unwrap();
    let operations = port.operations().unwrap();
    assert_eq!(operations[0].protocol_method, "session/resume");
    assert_eq!(operations[1].protocol_method, "session/new");
    assert_eq!(operations[1].kind, OperationKind::Rehydrate);
}

#[test]
fn low_codex_queues_instead_of_pretending_parallel() {
    let port = runtime(
        ProtocolFamily::Codex,
        CapabilityProfile::Low,
        SessionPresence::Present,
    );
    port.claim_writer(&key("matter:a")).unwrap();
    assert_eq!(
        port.claim_writer(&key("matter:b")).unwrap_err().code,
        ContinuityFailureCode::WriterBusy
    );
    assert_eq!(port.queued_matters().unwrap(), vec!["matter:b".to_string()]);
    for reason in port.safe_log().unwrap() {
        assert!(!reason.as_str().contains("session"));
    }
}

#[test]
fn low_pi_does_not_claim_clean_isolation() {
    let port = runtime(
        ProtocolFamily::Pi,
        CapabilityProfile::Low,
        SessionPresence::Present,
    );
    let snapshot = port.negotiate(&key("matter:a")).unwrap();
    assert_eq!(
        snapshot.isolated_context,
        NativeCapabilitySupport::Unverified
    );
    assert!(!port.isolation_review().claims_clean());
}

#[test]
fn archived_codex_unarchive_stays_exact_resume() {
    let port = runtime(
        ProtocolFamily::Codex,
        CapabilityProfile::High,
        SessionPresence::Archived,
    );
    port.exact_resume(&key("matter:docs")).unwrap();
    let operations = port.operations().unwrap();
    assert_eq!(operations[0].protocol_method, "thread/unarchive");
    assert!(operations[0].succeeded);
    assert!(
        operations
            .iter()
            .all(|op| op.protocol_method != "thread/start")
    );
}

#[test]
fn unbound_preregistered_port_stays_unavailable() {
    let port = unavailable_work_context_port();
    assert_eq!(
        port.steer(&NativeControlRequest::steer(
            key("matter:docs"),
            "steer-guidance",
            "turn:host",
            "turn:native",
        ))
        .unwrap_err()
        .code,
        ContinuityFailureCode::UnsupportedCapability
    );
}

#[test]
fn eof_cancel_on_pi_is_not_success() {
    use licoup_agent_runtime::work_context::{ProtocolOutcome, reconciliation_required};
    let port = WorkContextRuntime::from_hermetic(
        HermeticProtocol::pi(CapabilityProfile::High).with_scripted_cancel(
            ProtocolOutcome::unknown("session/cancel", reconciliation_required()),
        ),
        config(false),
    );
    let failure = port
        .cancel(&NativeControlRequest::cancel(
            key("matter:docs"),
            "turn:host",
            "turn:native",
        ))
        .unwrap_err();
    assert_eq!(failure.code, ContinuityFailureCode::ReconciliationRequired);
    assert_eq!(
        failure.effect_class,
        licoup_agent_runtime::work_context::ContinuityEffectClass::Unknown
    );
}

fn compile_fake_codex() -> std::path::PathBuf {
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("fake_codex_app_server.rs");
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temp_dir = std::env::temp_dir().join(format!("lico-ca-fake-codex-{suffix}"));
    std::fs::create_dir_all(&temp_dir).unwrap();
    let executable = temp_dir.join(format!("fake-codex{}", std::env::consts::EXE_SUFFIX));
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
    let compile = Command::new(rustc)
        .arg("--edition=2024")
        .arg(&fixture)
        .arg("-o")
        .arg(&executable)
        .status()
        .expect("fake Codex fixture should compile");
    assert!(compile.success(), "fake Codex fixture failed to compile");
    executable
}

fn write_resume_mode(executable: &std::path::Path, mode: &str) {
    let mut path = executable.to_path_buf();
    path.set_extension("resume-mode");
    std::fs::write(path, mode).unwrap();
}

fn write_session_id(executable: &std::path::Path, session_id: &str) {
    let mut path = executable.to_path_buf();
    path.set_extension("session-id");
    std::fs::write(path, session_id).unwrap();
}

fn compile_fake_pi() -> std::path::PathBuf {
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("fake_pi_rpc.rs");
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temp_dir = std::env::temp_dir().join(format!("lico-ca-fake-pi-{suffix}"));
    std::fs::create_dir_all(&temp_dir).unwrap();
    let executable = temp_dir.join(format!("fake-pi{}", std::env::consts::EXE_SUFFIX));
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
    let compile = Command::new(rustc)
        .arg("--edition=2024")
        .arg(&fixture)
        .arg("-o")
        .arg(&executable)
        .status()
        .expect("fake Pi fixture should compile");
    assert!(compile.success(), "fake Pi fixture failed to compile");
    executable
}

fn write_pi_session(root: &std::path::Path, session_id: &str) -> std::path::PathBuf {
    let project = root.join("nested-project");
    std::fs::create_dir_all(&project).unwrap();
    let path = project.join("session.jsonl");
    std::fs::write(
        &path,
        format!("{{\"type\":\"session\",\"version\":3,\"id\":\"{session_id}\"}}\n"),
    )
    .unwrap();
    path
}

fn live_child(
    store: &licoup_conversation::ConversationStore,
) -> (ChildBinding, NativeWorkContextKey) {
    use licoup_native::domain::client_conversation::ConversationService;
    use serde_json::json;

    let service = ConversationService::from_store(store.clone());
    let group = service
        .execute(json!({
            "action": "conversation.create",
            "title": "Driver child",
            "owner": {"id": "human:local", "kind": "human", "displayName": "You"},
            "members": [{
                "principal": {
                    "id": "agent:codex",
                    "kind": "agent",
                    "displayName": "Codex",
                    "agentId": "codex"
                },
                "access": "member"
            }]
        }))
        .unwrap();
    let conversation_id = group["id"].as_str().unwrap().to_owned();
    let membership_id = group["memberships"]
        .as_array()
        .unwrap()
        .iter()
        .find(|membership| membership["principal"]["kind"] == "agent")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    (
        ChildBinding {
            child_conversation_id: conversation_id.clone(),
            membership_id: membership_id.clone(),
            source_task_id: "goal:source-task".into(),
            parent_conversation_id: "conversation:parent".into(),
        },
        NativeWorkContextKey {
            conversation_id,
            membership_id,
            matter_id: "matter:docs".into(),
            generation: 1,
        },
    )
}

fn bind_child_with_session(
    store: &licoup_conversation::ConversationStore,
    session_id: &str,
    executable: &std::path::Path,
) -> (WorkContextRuntime, NativeWorkContextKey) {
    bind_family_child(
        store,
        ProtocolFamily::Codex,
        Some(session_id),
        executable,
        None,
    )
}

fn bind_family_child(
    store: &licoup_conversation::ConversationStore,
    family: ProtocolFamily,
    session_id: Option<&str>,
    executable: &std::path::Path,
    working_directory: Option<&std::path::Path>,
) -> (WorkContextRuntime, NativeWorkContextKey) {
    use licoup_conversation::RuntimeBinding;
    use licoup_native::platform::work_context_ports::{
        bind_adapter_work_context, host_driver_transport,
    };
    use std::sync::Arc;

    let (binding, key) = live_child(store);
    if let Some(session_id) = session_id {
        store
            .runtime_binding_with_private_location(
                RuntimeBinding {
                    id: format!("binding:{}", key.membership_id),
                    conversation_id: key.conversation_id.clone(),
                    membership_id: key.membership_id.clone(),
                    lane: "conversation".into(),
                    availability: "available".into(),
                    safe_reason: None,
                },
                Some(session_id),
                None,
                working_directory
                    .map(|path| path.to_string_lossy().into_owned())
                    .as_deref(),
            )
            .unwrap();
    }
    let mut transport =
        host_driver_transport(family).with_executable(executable.to_string_lossy().into_owned());
    if let Some(cwd) = working_directory {
        transport = transport.with_working_directory(cwd.to_path_buf());
    }
    (
        bind_adapter_work_context(
            family,
            WorkContextConfig::child(binding),
            Arc::new(transport),
            Some(store.clone()),
        ),
        key,
    )
}

#[test]
fn host_driver_reaches_codex_execute_for_exact_resume_and_identity() {
    use licoup_conversation::ConversationStore;
    use licoup_native::platform::work_context_ports::{
        AdapterCall, AdapterTransport, bind_adapter_work_context, host_driver_transport,
        pi_session_id_missing,
    };
    use serde_json::json;

    let executable = compile_fake_codex();
    let store = ConversationStore::open_in_memory().unwrap();
    write_resume_mode(&executable, "exact");
    let (runtime, resume_key) = bind_child_with_session(&store, "exact-thread", &executable);
    runtime.exact_resume(&resume_key).unwrap();
    assert_eq!(
        runtime.negotiate(&resume_key).unwrap().exact_resume,
        NativeCapabilitySupport::Supported
    );

    write_resume_mode(&executable, "mismatch");
    let mismatch_store = ConversationStore::open_in_memory().unwrap();
    let (mismatched, mismatch_key) =
        bind_child_with_session(&mismatch_store, "exact-thread", &executable);
    assert_eq!(
        mismatched.exact_resume(&mismatch_key).unwrap_err().code,
        ContinuityFailureCode::NativeBindingLost
    );

    write_resume_mode(&executable, "archived");
    let archived_store = ConversationStore::open_in_memory().unwrap();
    let (archived, archived_key) =
        bind_child_with_session(&archived_store, "exact-thread", &executable);
    archived.exact_resume(&archived_key).unwrap();

    let lost_store = ConversationStore::open_in_memory().unwrap();
    let (lost_binding, lost_key) = live_child(&lost_store);
    let lost = bind_adapter_work_context(
        ProtocolFamily::Codex,
        WorkContextConfig::child(lost_binding),
        std::sync::Arc::new(
            host_driver_transport(ProtocolFamily::Codex)
                .with_executable(executable.to_string_lossy().into_owned()),
        ),
        Some(lost_store),
    );
    assert_eq!(
        lost.exact_resume(&lost_key).unwrap_err().code,
        ContinuityFailureCode::NativeBindingLost
    );

    let transport = host_driver_transport(ProtocolFamily::Pi);
    let missing = transport.invoke(&AdapterCall {
        method: "session/resume",
        params: json!({ "sessionId": "missing-session" }),
    });
    assert!(!missing.ok);
    assert!(missing.error_message.as_deref().is_some_and(|message| {
        message.contains("session/resume")
            || message.contains("pi_session_not_found")
            || message.contains("pi_session_id_missing")
    }));
    assert!(pi_session_id_missing(""));
}

#[test]
fn pi_resume_and_start_are_effect_free_without_managed_executable() {
    use licoup_conversation::ConversationStore;
    use licoup_conversation::continuity::list_unknown_effect_ids;
    use licoup_native::platform::work_context_ports::{
        AdapterCall, AdapterTransport, host_driver_transport,
    };
    use serde_json::json;

    let store = ConversationStore::open_in_memory().unwrap();
    let before = list_unknown_effect_ids(&store).unwrap();
    let transport = host_driver_transport(ProtocolFamily::Pi);
    let resume = transport.invoke(&AdapterCall {
        method: "session/resume",
        params: json!({ "sessionId": "session:pi-proof" }),
    });
    let working_directory =
        std::env::temp_dir().join(format!("lico-ca-unconfigured-{}", uuid::Uuid::new_v4()));
    let start = transport.invoke(&AdapterCall {
        method: "session/new",
        params: json!({ "workingDirectory": working_directory }),
    });
    assert!(!resume.ok);
    assert!(!start.ok);
    assert!(
        resume
            .error_message
            .as_deref()
            .is_some_and(|message| message.contains("session/resume")
                || message.contains("adapter-executable-unconfigured"))
    );
    assert_eq!(
        start.error_message.as_deref(),
        Some("adapter-executable-unconfigured")
    );
    assert_eq!(list_unknown_effect_ids(&store).unwrap(), before);
    assert_eq!(transport.invocation_count(), 2);
}

#[test]
fn control_payload_is_preserved_at_transport_and_scoped_lane() {
    use licoup_conversation::ConversationStore;
    use licoup_conversation::RuntimeBinding;
    use licoup_native::dispatch_agent_operation as dispatch;
    use licoup_native::platform::work_context_ports::{
        AdapterCall, AdapterResponse, CountingTransport, bind_adapter_work_context,
    };
    use serde_json::json;
    use std::sync::{Arc, Mutex};

    let store = ConversationStore::open_in_memory().unwrap();
    let (binding, key) = live_child(&store);
    store
        .runtime_binding_with_private_location(
            RuntimeBinding {
                id: format!("binding:{}", key.membership_id),
                conversation_id: key.conversation_id.clone(),
                membership_id: key.membership_id.clone(),
                lane: "conversation".into(),
                availability: "available".into(),
                safe_reason: None,
            },
            Some("exact-thread"),
            None,
            Some("/workspace/project"),
        )
        .unwrap();
    let captured = Arc::new(Mutex::new(Vec::<AdapterCall>::new()));
    let observed = Arc::clone(&captured);
    let transport = CountingTransport::new(move |call: &AdapterCall| {
        observed.lock().unwrap().push(call.clone());
        AdapterResponse::ok(json!({ "ok": true }))
    });
    let runtime = bind_adapter_work_context(
        ProtocolFamily::Codex,
        WorkContextConfig::child(binding),
        Arc::new(transport),
        Some(store),
    );
    runtime
        .bind_live_control(&key, "turn:host-a", "turn:native-a")
        .unwrap();
    runtime
        .steer(&NativeControlRequest::steer(
            key.clone(),
            "exact-steer-content",
            "turn:host-a",
            "turn:native-a",
        ))
        .unwrap();
    runtime
        .cancel(&NativeControlRequest::cancel(
            key.clone(),
            "turn:host-a",
            "turn:native-a",
        ))
        .unwrap();
    let calls = captured.lock().unwrap().clone();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].method, "turn/steer");
    assert_eq!(calls[1].method, "turn/interrupt");
    for call in &calls {
        assert_eq!(call.params["conversationId"], key.conversation_id);
        assert_eq!(call.params["membershipId"], key.membership_id);
        assert_eq!(call.params["matterId"], key.matter_id);
        assert_eq!(call.params["generation"], 1);
        assert_eq!(call.params["sessionId"], "exact-thread");
        assert_eq!(call.params["threadId"], "exact-thread");
        assert_eq!(call.params["turnHandle"], "turn:host-a");
        assert_eq!(call.params["turnId"], "turn:native-a");
        assert_eq!(call.params["workingDirectory"], "/workspace/project");
        assert!(call.params.get("sessionId").unwrap().as_str() != Some("caller-forged-session"));
    }
    assert_eq!(calls[0].params["text"], "exact-steer-content");

    let lane = dispatch("steer", &calls[0].params).unwrap();
    assert_eq!(lane["ok"], false);
    assert_ne!(lane["status"], "unsupported");
    assert_eq!(
        lane["error"]["code"],
        "dispatch_steer_transport_unavailable"
    );
    let cancel = dispatch("cancel", &calls[1].params).unwrap();
    assert_eq!(cancel["ok"], false);
    assert_ne!(
        cancel.get("error").and_then(|error| error.get("code")),
        None
    );
}

#[test]
fn swapped_child_member_turn_and_generation_are_rejected_before_effect() {
    use licoup_conversation::ConversationStore;
    use licoup_conversation::RuntimeBinding;
    use licoup_native::platform::work_context_ports::{
        AdapterCall, AdapterResponse, CountingTransport, bind_adapter_work_context,
    };
    use serde_json::json;
    use std::sync::{Arc, Mutex};

    let store = ConversationStore::open_in_memory().unwrap();
    let (binding_a, key_a) = live_child(&store);
    let (binding_b, key_b) = live_child(&store);
    for (key, session) in [(&key_a, "session-a"), (&key_b, "session-b")] {
        store
            .runtime_binding_with_private_location(
                RuntimeBinding {
                    id: format!("binding:{}", key.membership_id),
                    conversation_id: key.conversation_id.clone(),
                    membership_id: key.membership_id.clone(),
                    lane: "conversation".into(),
                    availability: "available".into(),
                    safe_reason: None,
                },
                Some(session),
                None,
                None,
            )
            .unwrap();
    }
    let invocations = Arc::new(Mutex::new(0u64));
    let counted = Arc::clone(&invocations);
    let transport = CountingTransport::new(move |_call: &AdapterCall| {
        *counted.lock().unwrap() += 1;
        AdapterResponse::ok(json!({ "ok": true }))
    });
    let runtime_a = bind_adapter_work_context(
        ProtocolFamily::Codex,
        WorkContextConfig::child(binding_a),
        Arc::new(transport),
        Some(store.clone()),
    );
    runtime_a
        .bind_live_control(&key_a, "turn:host-a", "turn:native-a")
        .unwrap();
    assert_eq!(
        runtime_a
            .steer(&NativeControlRequest::steer(
                key_b.clone(),
                "leaked-steer",
                "turn:host-b",
                "turn:native-b",
            ))
            .unwrap_err()
            .code,
        ContinuityFailureCode::IdentityConflict
    );
    assert_eq!(
        runtime_a
            .steer(&NativeControlRequest::steer(
                NativeWorkContextKey {
                    membership_id: key_b.membership_id.clone(),
                    ..key_a.clone()
                },
                "leaked-steer",
                "turn:host-a",
                "turn:native-a",
            ))
            .unwrap_err()
            .code,
        ContinuityFailureCode::IdentityConflict
    );
    assert_eq!(
        runtime_a
            .cancel(&NativeControlRequest::cancel(
                key_a.clone(),
                "turn:host-b",
                "turn:native-a",
            ))
            .unwrap_err()
            .code,
        ContinuityFailureCode::IdentityConflict
    );
    let mut swapped_generation = key_a.clone();
    swapped_generation.generation = 2;
    assert_eq!(
        runtime_a
            .steer(&NativeControlRequest::steer(
                swapped_generation,
                "leaked-steer",
                "turn:host-a",
                "turn:native-a",
            ))
            .unwrap_err()
            .code,
        ContinuityFailureCode::ReconciliationRequired
    );
    assert_eq!(*invocations.lock().unwrap(), 0);
    let _ = binding_b;
}

#[test]
fn invalid_timeout_is_typed_rejection_with_zero_driver_effects() {
    use licoup_native::platform::work_context_ports::{
        AdapterCall, AdapterTransport, host_driver_transport,
    };
    use serde_json::json;

    let transport = host_driver_transport(ProtocolFamily::Codex)
        .with_executable("/nonexistent/licoup-codex-missing");
    let working_directory =
        std::env::temp_dir().join(format!("lico-ca-invalid-timeout-{}", uuid::Uuid::new_v4()));
    let rejected = transport.invoke(&AdapterCall {
        method: "thread/resume",
        params: json!({
            "threadId": "exact-thread",
            "timeoutMs": 500,
            "workingDirectory": working_directory
        }),
    });
    assert!(!rejected.ok);
    assert_eq!(
        rejected.error_message.as_deref(),
        Some("invalid_request:timeout")
    );
    assert_eq!(rejected.result["code"], "invalid_request");
    assert!(
        !rejected
            .error_message
            .as_deref()
            .unwrap_or_default()
            .contains("not available")
    );
}

#[test]
fn transient_transport_failure_is_not_binding_loss() {
    use licoup_conversation::ConversationStore;
    use licoup_conversation::RuntimeBinding;
    use licoup_native::platform::work_context_ports::{
        AdapterCall, AdapterResponse, CountingTransport, bind_adapter_work_context,
    };
    use serde_json::json;
    use std::sync::Arc;

    let store = ConversationStore::open_in_memory().unwrap();
    let (binding, key) = live_child(&store);
    store
        .runtime_binding_with_private_location(
            RuntimeBinding {
                id: format!("binding:{}", key.membership_id),
                conversation_id: key.conversation_id.clone(),
                membership_id: key.membership_id.clone(),
                lane: "conversation".into(),
                availability: "available".into(),
                safe_reason: None,
            },
            Some("exact-thread"),
            None,
            None,
        )
        .unwrap();
    let transport = CountingTransport::new(|_call: &AdapterCall| AdapterResponse {
        ok: false,
        result: json!({
            "code": "codex_rpc_write_failed",
            "stage": "protocol/write",
        }),
        error_message: Some("codex_rpc_write_failed:protocol/write".to_owned()),
    });
    let runtime = bind_adapter_work_context(
        ProtocolFamily::Codex,
        WorkContextConfig::child(binding),
        Arc::new(transport),
        Some(store),
    );
    let failure = runtime.exact_resume(&key).unwrap_err();
    assert_eq!(failure.code, ContinuityFailureCode::ReconciliationRequired);
    assert_ne!(failure.code, ContinuityFailureCode::NativeBindingLost);
    assert!(failure.retryable);
}

#[test]
fn host_driver_reaches_pi_execute_for_resume_start_and_loss() {
    use licoup_conversation::ConversationStore;
    use licoup_native::platform::work_context_ports::{
        AdapterCall, AdapterTransport, bind_adapter_work_context, host_driver_transport,
    };
    use serde_json::json;

    let executable = compile_fake_pi();
    let session_root = executable.parent().unwrap().join("pi-sessions");
    std::fs::create_dir_all(&session_root).unwrap();
    write_pi_session(&session_root, "pi-exact-session");
    write_resume_mode(&executable, "exact");
    write_session_id(&executable, "pi-exact-session");
    let previous = std::env::var("PI_CODING_AGENT_SESSION_DIR").ok();
    unsafe {
        std::env::set_var("PI_CODING_AGENT_SESSION_DIR", &session_root);
    }

    let store = ConversationStore::open_in_memory().unwrap();
    let cwd = session_root.as_path();
    let (runtime, resume_key) = bind_family_child(
        &store,
        ProtocolFamily::Pi,
        Some("pi-exact-session"),
        &executable,
        Some(cwd),
    );
    runtime.exact_resume(&resume_key).unwrap();
    assert_eq!(
        runtime.negotiate(&resume_key).unwrap().exact_resume,
        NativeCapabilitySupport::Supported
    );

    write_resume_mode(&executable, "mismatch");
    let mismatch_store = ConversationStore::open_in_memory().unwrap();
    let (mismatched, mismatch_key) = bind_family_child(
        &mismatch_store,
        ProtocolFamily::Pi,
        Some("pi-exact-session"),
        &executable,
        Some(cwd),
    );
    assert_eq!(
        mismatched.exact_resume(&mismatch_key).unwrap_err().code,
        ContinuityFailureCode::NativeBindingLost
    );

    write_resume_mode(&executable, "");
    let start_store = ConversationStore::open_in_memory().unwrap();
    let (started, start_key) = bind_family_child(
        &start_store,
        ProtocolFamily::Pi,
        None,
        &executable,
        Some(cwd),
    );
    assert_eq!(started.start_new(&start_key).unwrap(), 2);

    let missing = host_driver_transport(ProtocolFamily::Pi)
        .with_executable(executable.to_string_lossy().into_owned())
        .with_working_directory(cwd.to_path_buf())
        .invoke(&AdapterCall {
            method: "session/resume",
            params: json!({
                "sessionId": "missing-session",
                "workingDirectory": cwd.to_string_lossy(),
            }),
        });
    assert!(!missing.ok);
    assert!(missing.error_message.as_deref().is_some_and(|message| {
        message.contains("pi_session_not_found") || message.contains("session/resume")
    }));

    std::fs::create_dir_all(session_root.join("a")).unwrap();
    std::fs::create_dir_all(session_root.join("b")).unwrap();
    let header = "{\"type\":\"session\",\"version\":3,\"id\":\"duplicate-session\"}\n";
    std::fs::write(session_root.join("a/first.jsonl"), header).unwrap();
    std::fs::write(session_root.join("b/second.jsonl"), header).unwrap();
    let ambiguous = host_driver_transport(ProtocolFamily::Pi)
        .with_executable(executable.to_string_lossy().into_owned())
        .with_working_directory(cwd.to_path_buf())
        .invoke(&AdapterCall {
            method: "session/resume",
            params: json!({
                "sessionId": "duplicate-session",
                "workingDirectory": cwd.to_string_lossy(),
            }),
        });
    assert!(!ambiguous.ok);
    assert!(
        ambiguous
            .error_message
            .as_deref()
            .is_some_and(|message| { message.contains("pi_session_identity_ambiguous") })
    );

    let lost_store = ConversationStore::open_in_memory().unwrap();
    let (lost_binding, lost_key) = live_child(&lost_store);
    let lost = bind_adapter_work_context(
        ProtocolFamily::Pi,
        WorkContextConfig::child(lost_binding),
        std::sync::Arc::new(
            host_driver_transport(ProtocolFamily::Pi)
                .with_executable(executable.to_string_lossy().into_owned())
                .with_working_directory(cwd.to_path_buf()),
        ),
        Some(lost_store),
    );
    assert_eq!(
        lost.exact_resume(&lost_key).unwrap_err().code,
        ContinuityFailureCode::NativeBindingLost
    );

    match previous {
        Some(value) => unsafe {
            std::env::set_var("PI_CODING_AGENT_SESSION_DIR", value);
        },
        None => unsafe {
            std::env::remove_var("PI_CODING_AGENT_SESSION_DIR");
        },
    }
}

#[test]
fn host_driver_transport_forwards_developer_instructions_to_fake_codex_process() {
    use licoup_native::platform::work_context_ports::{
        AdapterCall, AdapterTransport, host_driver_transport,
    };
    use serde_json::json;

    let executable = compile_fake_codex();
    let mut result_path = executable.clone();
    result_path.set_extension("result.json");
    std::fs::write(&result_path, "\"ok\"").unwrap();
    let cwd = std::env::temp_dir().join(format!(
        "lico-ca-driver-cwd-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&cwd).unwrap();
    let transport = host_driver_transport(ProtocolFamily::Codex)
        .with_executable(executable.to_string_lossy().into_owned())
        .with_working_directory(cwd.clone());
    let started = transport.invoke(&AdapterCall {
        method: "thread/start",
        params: json!({
            "text": "PRODUCTION-GRANT-SENTINEL prepare notes now",
            "developerInstructions": "PRODUCTION-GRANT-SENTINEL stay in the instruction field",
            "workingDirectory": cwd.to_string_lossy(),
        }),
    });
    assert!(
        started.ok,
        "HostDriverTransport must start the fake process"
    );
    let mut seen = executable.clone();
    seen.set_extension("turn-start.seen");
    let seen = std::fs::read_to_string(&seen).unwrap_or_default();
    assert_eq!(
        seen.trim(),
        "1",
        "fake Codex must observe developerInstructions without rewriting text"
    );
    let mut leak = executable.clone();
    leak.set_extension("leak.seen");
    assert!(
        !leak.exists(),
        "driver-layer call must not invent out-of-scope material"
    );
    assert!(transport.invocation_count() >= 1);
    let _ = std::fs::remove_file(result_path);
    let _ = std::fs::remove_dir_all(executable.parent().unwrap());
    let _ = std::fs::remove_dir_all(cwd);
}

/// The manual-stop control plane records its request and its unconfirmed
/// outcome through the private diagnostic owner, and a reader opened after the
/// writing process finished retrieves the same redacted record.
#[test]
fn stop_anomaly_records_survive_the_writing_process_and_stay_redacted() {
    let root =
        std::env::temp_dir().join(format!("licoup-stop-control-native-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let previous =
        licoup_foundation::platform::paths::set_portable_data_dir_override(Some(root.clone()));

    // A lane session with no live owner is a real manual-stop request whose
    // owner cannot be reached: the request is recorded, and the outcome is an
    // anomaly rather than a fabricated stop.
    let response = licoup_native::platform::stop_control::stop_work(
        &serde_json::json!({
            "agent": "codex",
            "sessionId": "session:not-bound",
            "reason": "user-stop",
        }),
        &licoup_native::platform::stop_control::WorkStopPorts::default(),
    );
    assert_eq!(response["ok"], false, "{response}");
    assert_eq!(response["status"], "owner-unavailable");
    assert_eq!(response["diagnostics"]["recorded"], true);
    let correlation_id = response["correlationId"].as_str().unwrap().to_owned();
    assert!(correlation_id.starts_with("stop-"));

    // A later reader of the durable diagnostic store sees the same record.
    let listed =
        licoup_native::platform::client_state::activity_list(&serde_json::json!({"limit": 100}))
            .unwrap();
    let events = listed["events"].as_array().unwrap();
    let recorded = events
        .iter()
        .find(|event| event["payload"]["correlationId"] == serde_json::json!(correlation_id))
        .expect("the stop request is durable");
    assert_eq!(recorded["type"], "work.stop.requested");
    assert!(
        events.iter().any(|event| {
            event["type"] == "work.stop.anomaly"
                && event["payload"]["correlationId"] == serde_json::json!(correlation_id)
        }),
        "the unreachable owner is recorded as the anomaly it is"
    );
    let encoded = serde_json::to_string(&events).unwrap();
    assert!(!encoded.contains("session:not-bound"), "{encoded}");
    assert!(
        !encoded.contains(&root.to_string_lossy().to_string()),
        "{encoded}"
    );

    licoup_foundation::platform::paths::set_portable_data_dir_override(previous);
    let _ = std::fs::remove_dir_all(&root);
}
