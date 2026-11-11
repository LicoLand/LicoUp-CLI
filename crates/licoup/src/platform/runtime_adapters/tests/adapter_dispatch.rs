use super::super::adapter::adapter_for_agent;
use super::super::dispatch::params_with_workspace;
// The composition's entry point, not the moved crate's raw one: this one
// installs the host before it dispatches, exactly as production does.
use super::super::params::message_param;
use super::super::send_message;
use super::super::{RuntimeAdapter, RuntimeAdapterError};
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

struct AttachmentFixture {
    directory: PathBuf,
    png: PathBuf,
    jpeg: PathBuf,
    gif: PathBuf,
    webp: PathBuf,
}

impl AttachmentFixture {
    fn new() -> Self {
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let suffix = format!(
            "{}-{sequence}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
        );
        let directory =
            std::env::temp_dir().join(format!("lico-attachment-{}-{suffix}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let png = directory.join("synthetic.png");
        let jpeg = directory.join("synthetic.jpg");
        let gif = directory.join("synthetic.gif");
        let webp = directory.join("synthetic.webp");
        fs::write(
            &png,
            [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0],
        )
        .unwrap();
        fs::write(&jpeg, [0xFF, 0xD8, 0xFF, 0xE0, 0, 0, 0, 0]).unwrap();
        fs::write(&gif, b"GIF89a-synthetic").unwrap();
        fs::write(&webp, b"RIFF\x10\x00\x00\x00WEBPVP8 ").unwrap();
        Self {
            directory,
            png,
            jpeg,
            gif,
            webp,
        }
    }

    fn attachment(&self, id: &str, path: &Path) -> serde_json::Value {
        let media_type = match path.extension().and_then(|value| value.to_str()) {
            Some("png") => "image/png",
            Some("jpg") | Some("jpeg") => "image/jpeg",
            Some("gif") => "image/gif",
            Some("webp") => "image/webp",
            _ => "image/png",
        };
        json!({
            "id": id,
            "name": path.file_name().unwrap().to_string_lossy(),
            "mediaType": media_type,
            "path": path.to_string_lossy()
        })
    }
}

impl Drop for AttachmentFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

fn codex_params(
    fixture: &AttachmentFixture,
    attachments: Vec<serde_json::Value>,
) -> serde_json::Value {
    json!({
        "agent": "codex",
        "text": "describe the images",
        "binaryPath": "/bin/sh",
        "attachments": attachments,
        "cwd": fixture.directory.to_string_lossy()
    })
}

/// Write one synthetic executable that records its own launch. A refusal that
/// leaves no record proves the driver was never started.
#[cfg(unix)]
fn executable_script(path: &Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    fs::write(path, body).unwrap();
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

#[test]
fn deepseek_send_reaches_native_validation_without_claiming_release_readiness() {
    let result = send_message(
        &crate::target_port::agent_target_port(),
        &json!({
            "agent": "deepseek-harness",
            "text": "synthetic prompt",
            "workingDirectory": "/synthetic",
            "binaryPath": std::env::current_exe().unwrap()
        }),
    )
    .unwrap();

    assert_eq!(result["ok"], false);
    assert_eq!(result["error"]["code"], "deepseek_harness_model_required");
}

#[test]
fn deepseek_assistant_guidance_uses_ordinary_wire_without_changing_user_text() {
    let source = "exact synthetic user message";
    let delivery = super::super::compose_generated_instruction_delivery(
        "deepseek-harness",
        source,
        Some("synthetic Assistant guidance"),
    )
    .unwrap();
    assert_eq!(
        delivery.text,
        "synthetic Assistant guidance\n\nexact synthetic user message"
    );
    assert!(delivery.field.is_none());
    assert!(delivery.guidance.is_none());
}

#[cfg(unix)]
#[test]
fn codex_attachments_pass_admission_and_reach_the_driver() {
    let fixture = AttachmentFixture::new();
    let result = send_message(
        &crate::target_port::agent_target_port(),
        &codex_params(
            &fixture,
            vec![
                fixture.attachment("sel-1", &fixture.png),
                fixture.attachment("sel-2", &fixture.jpeg),
                fixture.attachment("sel-3", &fixture.gif),
                fixture.attachment("sel-4", &fixture.webp),
            ],
        ),
    )
    .expect("admission must accept four valid local images");

    // The shell fixture cannot speak the app-server protocol, so the turn
    // fails at the protocol stage — proving admission passed and a process
    // was actually launched (no attachment error surfaced).
    assert_eq!(result["ok"], false);
    let code = result["error"]["code"].as_str().unwrap_or_default();
    assert!(
        code.starts_with("codex_"),
        "expected a Codex protocol failure, got {code}"
    );
}

#[cfg(unix)]
#[test]
fn attachment_only_send_is_accepted_for_codex() {
    let fixture = AttachmentFixture::new();
    let params = json!({
        "agent": "codex",
        "text": "",
        "binaryPath": "/bin/sh",
        "attachments": [fixture.attachment("sel-1", &fixture.png)],
        "cwd": fixture.directory.to_string_lossy()
    });
    let result = send_message(&crate::target_port::agent_target_port(), &params)
        .expect("attachment-only sends must not be rejected as missing text");
    assert_eq!(result["ok"], false);
    let code = result["error"]["code"].as_str().unwrap_or_default();
    assert!(code.starts_with("codex_"), "got {code}");
}

#[test]
fn non_codex_adapter_rejects_attachments_before_launch() {
    let fixture = AttachmentFixture::new();
    let error = send_message(
        &crate::target_port::agent_target_port(),
        &json!({
            "agent": "claude-code",
            "text": "hello",
            "attachments": [fixture.attachment("sel-1", &fixture.png)],
            "binary": "/definitely/not/a/claude-binary"
        }),
    )
    .unwrap_err();
    assert_eq!(
        error,
        RuntimeAdapterError::AttachmentUnsupportedForAdapter {
            agent_label: "claude-code".to_string()
        }
    );
}

#[test]
fn virtual_machine_transport_rejects_attachments_before_launch() {
    let fixture = AttachmentFixture::new();
    let error = send_message(
        &crate::target_port::agent_target_port(),
        &json!({
            "agent": "openclaw",
            "text": "hello",
            "attachments": [fixture.attachment("sel-1", &fixture.png)],
            "runtimeConnection": {
                "kind": "ssh",
                "host": "guest.example.test",
                "user": "fixture",
                "remoteExecutable": "/usr/bin/openclaw",
                "workingDirectory": "/workspace/project"
            }
        }),
    )
    .unwrap_err();
    assert_eq!(
        error,
        RuntimeAdapterError::AttachmentUnsupportedForAdapter {
            agent_label: "openclaw".to_string()
        }
    );
}

#[test]
fn excessive_attachment_list_is_rejected_before_launch() {
    let fixture = AttachmentFixture::new();
    let attachments = (0..5)
        .map(|index| fixture.attachment(&format!("sel-{index}"), &fixture.png))
        .collect();
    let error = send_message(
        &crate::target_port::agent_target_port(),
        &codex_params(&fixture, attachments),
    )
    .unwrap_err();
    assert_eq!(error, RuntimeAdapterError::AttachmentListExceeded);
}

#[test]
fn malformed_attachment_shapes_are_rejected_before_launch() {
    let cases: Vec<(serde_json::Value, RuntimeAdapterError)> = vec![
        (
            json!({"agent": "codex", "text": "hello", "attachments": "not-an-array"}),
            RuntimeAdapterError::AttachmentInvalid,
        ),
        (
            json!({"agent": "codex", "text": "hello", "attachments": [{"id": "1"}]}),
            RuntimeAdapterError::AttachmentInvalid,
        ),
        (
            json!({
                "agent": "codex",
                "text": "hello",
                "attachments": [{
                    "id": "1",
                    "name": "image.png",
                    "mediaType": "image/png",
                    "path": "attachment-fixtures/image.png",
                    "unexpected": true
                }]
            }),
            RuntimeAdapterError::AttachmentInvalid,
        ),
        (
            json!({
                "agent": "codex",
                "text": "hello",
                "attachments": [{
                    "id": "1",
                    "name": "",
                    "mediaType": "image/png",
                    "path": "attachment-fixtures/image.png"
                }]
            }),
            RuntimeAdapterError::AttachmentInvalid,
        ),
        (
            json!({
                "agent": "codex",
                "text": "hello",
                "attachments": [{
                    "id": "1",
                    "name": "image.heic",
                    "mediaType": "image/heic",
                    "path": "attachment-fixtures/image.heic"
                }]
            }),
            RuntimeAdapterError::AttachmentMediaUnsupported,
        ),
        (
            json!({
                "agent": "codex",
                "text": "hello",
                "attachments": [{
                    "id": "1",
                    "name": "image.png",
                    "mediaType": "image/png",
                    "path": "https://example.test/image.png"
                }]
            }),
            RuntimeAdapterError::AttachmentRemoteUnsupported,
        ),
    ];
    for (params, expected) in cases {
        assert_eq!(
            send_message(&crate::target_port::agent_target_port(), &params).unwrap_err(),
            expected
        );
    }
}

#[test]
fn symlink_and_non_regular_attachments_are_rejected_before_launch() {
    let fixture = AttachmentFixture::new();
    #[cfg(unix)]
    {
        let link = fixture.directory.join("linked.png");
        std::os::unix::fs::symlink(&fixture.png, &link).unwrap();
        let error = send_message(
            &crate::target_port::agent_target_port(),
            &json!({
                "agent": "codex",
                "text": "hello",
                "attachments": [fixture.attachment("sel-1", &link)]
            }),
        )
        .unwrap_err();
        assert_eq!(error, RuntimeAdapterError::AttachmentSymlinkRejected);
    }

    let error = send_message(
        &crate::target_port::agent_target_port(),
        &json!({
            "agent": "codex",
            "text": "hello",
            "attachments": [fixture.attachment("sel-1", &fixture.directory)]
        }),
    )
    .unwrap_err();
    assert_eq!(error, RuntimeAdapterError::AttachmentFileUnavailable);
}

#[test]
fn missing_attachment_file_is_rejected_before_launch() {
    let fixture = AttachmentFixture::new();
    let missing = fixture.directory.join("missing.png");
    let error = send_message(
        &crate::target_port::agent_target_port(),
        &json!({
            "agent": "codex",
            "text": "hello",
            "attachments": [fixture.attachment("sel-1", &missing)]
        }),
    )
    .unwrap_err();
    assert_eq!(error, RuntimeAdapterError::AttachmentFileUnavailable);
}

#[test]
fn mismatched_signature_is_rejected_before_launch() {
    let fixture = AttachmentFixture::new();
    let wrong = fixture.directory.join("wrong.png");
    fs::write(&wrong, b"not-an-image").unwrap();
    let error = send_message(
        &crate::target_port::agent_target_port(),
        &json!({
            "agent": "codex",
            "text": "hello",
            "attachments": [fixture.attachment("sel-1", &wrong)]
        }),
    )
    .unwrap_err();
    assert_eq!(error, RuntimeAdapterError::AttachmentSignatureMismatch);
}

#[test]
fn oversized_attachment_file_is_rejected_before_launch() {
    let fixture = AttachmentFixture::new();
    let oversized = fixture.directory.join("oversized.png");
    fs::write(&oversized, [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]).unwrap();
    let file = fs::OpenOptions::new().write(true).open(&oversized).unwrap();
    file.set_len(4 * 1024 * 1024 + 1).unwrap();
    drop(file);
    let error = send_message(
        &crate::target_port::agent_target_port(),
        &json!({
            "agent": "codex",
            "text": "hello",
            "attachments": [fixture.attachment("sel-1", &oversized)]
        }),
    )
    .unwrap_err();
    assert_eq!(error, RuntimeAdapterError::AttachmentSizeLimit);
}

#[test]
fn attachment_total_byte_budget_is_enforced() {
    use super::super::params::{
        MAX_IMAGE_ATTACHMENT_BYTES_PER_FILE, MAX_IMAGE_ATTACHMENT_BYTES_TOTAL,
    };
    assert_eq!(MAX_IMAGE_ATTACHMENT_BYTES_PER_FILE, 4 * 1024 * 1024);
    assert_eq!(
        MAX_IMAGE_ATTACHMENT_BYTES_TOTAL,
        4 * MAX_IMAGE_ATTACHMENT_BYTES_PER_FILE
    );
}
#[test]
fn adapter_aliases_resolve_to_canonical_ids() {
    assert_eq!(
        adapter_for_agent("claude").map(RuntimeAdapter::id),
        Some("claude-code")
    );
    assert_eq!(
        adapter_for_agent("github-copilot").map(RuntimeAdapter::id),
        Some("copilot")
    );
    assert_eq!(
        adapter_for_agent("kilocode").map(RuntimeAdapter::id),
        Some("kilo-code")
    );
    assert_eq!(
        adapter_for_agent("cursor-agent").map(RuntimeAdapter::id),
        Some("cursor")
    );
}

#[test]
fn message_body_is_not_normalized() {
    let body = "\n  indented code  \n";
    assert_eq!(
        message_param(&json!({"text": body}), &["text"]),
        Some(body.to_string())
    );
}

#[test]
fn message_beyond_the_removed_shared_cap_reaches_native_admission() {
    let oversized = "x".repeat(1024 * 1024 + 1);
    let error = send_message(
        &crate::target_port::agent_target_port(),
        &json!({
            "agent": "codex",
            "text": oversized,
            "binaryPath": "/runtime/must-not-launch"
        }),
    )
    .unwrap_err();

    assert_eq!(error.to_string(), "native agent executable is unavailable");
}

#[test]
fn configured_command_fallback_has_been_removed() {
    let error = send_message(
        &crate::target_port::agent_target_port(),
        &json!({
            "agent": "claude-code",
            "text": "private prompt",
            "binary": "/definitely/not/a/claude-binary",
            "command": "/bin/echo",
            "args": ["{prompt}"]
        }),
    )
    .unwrap_err();

    assert_eq!(error, RuntimeAdapterError::LegacyLaunchConfiguration);
}

#[test]
fn invalid_explicit_runtime_bounds_are_rejected_instead_of_clamped() {
    for (field, value) in [
        ("timeoutMs", json!(999)),
        ("timeoutMs", json!(30 * 60 * 1_000_u64 + 1)),
        ("maxStdoutBytes", json!(0)),
        ("maxStdoutBytes", json!(64 * 1024 * 1024_u64 + 1)),
        ("maxStderrBytes", json!(0)),
    ] {
        let mut request = json!({
            "agent": "claude-code",
            "text": "synthetic prompt",
            "binary": "/runtime/must-not-launch"
        });
        request[field] = value;
        assert_eq!(
            send_message(&crate::target_port::agent_target_port(), &request).unwrap_err(),
            RuntimeAdapterError::InvalidRuntimeSetting { field }
        );
    }
}

/// Every driver reads its working directory from the request, so the resolved
/// workspace has to replace the requested one under both keys.
#[test]
fn a_local_turn_republishes_only_the_resolved_workspace() {
    let resolved = params_with_workspace(
        &json!({
            "agent": "cursor",
            "text": "hello",
            "cwd": "/fixture-root/resident",
            "workingDirectory": "/fixture-root/resident"
        }),
        Path::new("/synthetic/state/agent-workspace"),
    );

    assert_eq!(resolved["cwd"], "/synthetic/state/agent-workspace");
    assert_eq!(
        resolved["workingDirectory"],
        "/synthetic/state/agent-workspace"
    );
    assert_eq!(resolved["text"], "hello");
}

#[test]
fn unknown_runtime_adapter_is_rejected() {
    let error = send_message(
        &crate::target_port::agent_target_port(),
        &json!({"agent": "unknown", "text": "hello"}),
    )
    .unwrap_err();
    assert!(error.to_string().contains("unsupported runtime adapter"));
}

/// The image admission reads the projected fact instead of naming an adapter:
/// an Agent whose owner was read and does not list image input is refused
/// before any process starts, and the arranged request is not rewritten.
#[cfg(unix)]
#[test]
fn an_agent_without_declared_image_input_is_refused_before_any_launch() {
    let fixture = AttachmentFixture::new();
    let marker = fixture.directory.join("launched.marker");
    let binary = fixture.directory.join("claude");
    executable_script(&binary, &format!("#!/bin/sh\ntouch {}\n", marker.display()));
    let text = "  exact synthetic prompt  \n";
    let params = json!({
        "agent": "claude-code",
        "text": text,
        "sessionId": "native-session:exact-1",
        "binary": binary.to_string_lossy(),
        "attachments": [fixture.attachment("sel-1", &fixture.png)],
    });
    let arranged = serde_json::to_string(&params).unwrap();

    let error = send_message(&crate::target_port::agent_target_port(), &params).unwrap_err();
    assert_eq!(
        error,
        RuntimeAdapterError::AttachmentUnsupportedForAdapter {
            agent_label: "claude-code".to_owned()
        }
    );
    assert!(
        !marker.exists(),
        "the driver must not be launched for an undeclared image action"
    );
    assert_eq!(serde_json::to_string(&params).unwrap(), arranged);
    assert_eq!(
        message_param(&params, &["text"]).as_deref(),
        Some(text),
        "the original text is preserved byte for byte"
    );
    assert_eq!(params["sessionId"], json!("native-session:exact-1"));
}

/// A Profile requirement that no declared fact satisfies is refused at the
/// dispatch entry, names the fact, and never launches the driver.
#[cfg(unix)]
#[test]
fn an_unsatisfied_capability_requirement_is_refused_before_any_launch() {
    let fixture = AttachmentFixture::new();
    let marker = fixture.directory.join("launched.marker");
    let binary = fixture.directory.join("claude");
    executable_script(&binary, &format!("#!/bin/sh\ntouch {}\n", marker.display()));
    let text = "  exact synthetic prompt  \n";
    let params = json!({
        "agent": "claude-code",
        "text": text,
        "sessionId": "native-session:exact-1",
        "binary": binary.to_string_lossy(),
        "requiredCapabilities": ["image-input"],
    });
    let arranged = serde_json::to_string(&params).unwrap();

    let error = send_message(&crate::target_port::agent_target_port(), &params).unwrap_err();
    assert_eq!(
        error,
        RuntimeAdapterError::CapabilityRequirementUnsatisfied {
            capability: "image-input".to_owned()
        }
    );
    assert!(error.to_string().contains("capability"));
    assert!(
        !marker.exists(),
        "the driver must not be launched for an unsatisfied requirement"
    );
    assert_eq!(serde_json::to_string(&params).unwrap(), arranged);
    assert_eq!(message_param(&params, &["text"]).as_deref(), Some(text));
    assert_eq!(params["sessionId"], json!("native-session:exact-1"));
}

/// A requirement only a declared fact satisfies; the identity of the Agent is
/// irrelevant, the fact is not.
#[test]
fn declared_capability_requirements_pass_admission() {
    // Codex declares image input, so the requirement is satisfied and the turn
    // reaches executable resolution instead of being refused.
    let error = send_message(
        &crate::target_port::agent_target_port(),
        &json!({
            "agent": "codex",
            "text": "hello",
            "sessionId": "native-session:exact-1",
            "requiredCapabilities": ["image-input", "conversationDriver:supported"],
            "binaryPath": "/runtime/must-not-launch",
        }),
    )
    .unwrap_err();
    assert_eq!(error, RuntimeAdapterError::ExecutableUnavailable);

    // The same requirement on an Agent whose owner does not list it is refused.
    let error = send_message(
        &crate::target_port::agent_target_port(),
        &json!({
            "agent": "claude-code",
            "text": "hello",
            "requiredCapabilities": ["image-input"],
            "binary": "/runtime/must-not-launch",
        }),
    )
    .unwrap_err();
    assert_eq!(
        error,
        RuntimeAdapterError::CapabilityRequirementUnsatisfied {
            capability: "image-input".to_owned()
        }
    );

    // A name outside the closed fact vocabulary cannot be satisfied either.
    let error = send_message(
        &crate::target_port::agent_target_port(),
        &json!({
            "agent": "codex",
            "text": "hello",
            "requiredCapabilities": ["not-a-capability"],
            "binaryPath": "/runtime/must-not-launch",
        }),
    )
    .unwrap_err();
    assert_eq!(
        error,
        RuntimeAdapterError::CapabilityRequirementUnsatisfied {
            capability: "not-a-capability".to_owned()
        }
    );

    // No packaged Agent has verified readiness, so the readiness fact is
    // not-declared rather than silently treated as satisfied.
    let error = send_message(
        &crate::target_port::agent_target_port(),
        &json!({
            "agent": "codex",
            "text": "hello",
            "requiredCapabilities": ["conversationDriver:ready"],
            "binaryPath": "/runtime/must-not-launch",
        }),
    )
    .unwrap_err();
    assert_eq!(
        error,
        RuntimeAdapterError::CapabilityRequirementUnsatisfied {
            capability: "conversationDriver:ready".to_owned()
        }
    );
}

/// The admitted path hands the driver the arranged text and native session
/// identity byte for byte; admission adds no normalization and no identity
/// replacement.
#[cfg(unix)]
#[test]
fn an_admitted_turn_carries_the_exact_text_and_native_identity() {
    let fixture = AttachmentFixture::new();
    let capture = fixture.directory.join("argv.bin");
    let binary = fixture.directory.join("cursor-agent");
    executable_script(
        &binary,
        &format!(
            "#!/bin/sh\nfor argument in \"$@\"; do printf '%s\\0' \"$argument\"; done > {}\n",
            capture.display()
        ),
    );
    let text = "  exact synthetic prompt  \n";
    let session = "native-session:exact-1";
    let result = send_message(
        &crate::target_port::agent_target_port(),
        &json!({
            "agent": "cursor",
            "text": text,
            "sessionId": session,
            "binary": binary.to_string_lossy(),
            "cwd": fixture.directory.to_string_lossy(),
        }),
    )
    .expect("the synthetic turn must pass dispatch admission");

    assert!(
        capture.exists(),
        "the synthetic driver was not launched: {}",
        result["error"]["code"]
    );
    let recorded = fs::read(&capture).expect("the driver must have been launched");
    let arguments = recorded
        .split(|byte| *byte == 0)
        .filter(|argument| !argument.is_empty())
        .map(|argument| String::from_utf8_lossy(argument).into_owned())
        .collect::<Vec<_>>();
    assert!(
        arguments.iter().any(|argument| argument == text),
        "the dispatched text must match the arranged text byte for byte: {arguments:?}"
    );
    assert!(
        arguments.iter().any(|argument| argument == session),
        "the bound native session identity must be dispatched unchanged: {arguments:?}"
    );
}
