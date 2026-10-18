use super::*;

#[test]
fn fake_child_streams_redacted_events_and_drains_stderr() {
    let (directory, executable) = compile_fake_openclaw("lico-openclaw-execution");
    let result = execute(
        executable.to_string_lossy().as_ref(),
        &json!({
            "reasoningEffort": "medium",
            "gatewayWsUrl": "ws://127.0.0.1:9"
        }),
        "private-openclaw-prompt",
        "",
        Some(directory.as_path()),
        10_000,
        Some(128 * 1024),
        8 * 1024,
    );
    assert!(result.ok, "OpenClaw fake failure: {:?}", result.error);
    assert_eq!(result.output, "native answer");
    assert_eq!(result.session_id, "agent:main:acp:native-session");
    assert_eq!(result.turn_status, "end_turn");
    assert!(matches!(
        result.transitions.last(),
        Some(licoup_agent_adapter_sdk::Transition::Lifecycle(
            licoup_agent_adapter_sdk::LifecycleStage::Completed
        ))
    ));
    assert!(result.stderr_truncated);
    let _ = fs::remove_dir_all(directory);
}

#[test]
fn active_gateway_session_accepts_acp_cancel_before_exact_resume() {
    let (directory, executable) =
        compile_fake_openclaw_source("lico-openclaw-cancel", FAKE_OPENCLAW_CANCEL_SOURCE);
    let run_directory = directory.clone();
    let run_executable = executable.clone();
    let run = std::thread::spawn(move || {
        execute(
            run_executable.to_string_lossy().as_ref(),
            &json!({"gatewayWsUrl": "ws://127.0.0.1:9"}),
            "cancel active gateway turn",
            "",
            Some(run_directory.as_path()),
            30_000,
            Some(128 * 1024),
            8 * 1024,
        )
    });
    // The turn is cancellable once the transport has bound the Gateway session
    // to the ACP protocol session; until then the control plane answers that no
    // turn is active, which is why the attempt is repeated rather than assumed.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    let mut disposition =
        licoup_agent_drivers::acp_driver_runtime::ControlDisposition::NoActiveTurn;
    while std::time::Instant::now() < deadline {
        disposition = crate::driver::cancel("agent:main:acp:cancel-session");
        if disposition == licoup_agent_drivers::acp_driver_runtime::ControlDisposition::Accepted {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert_eq!(
        disposition,
        licoup_agent_drivers::acp_driver_runtime::ControlDisposition::Accepted,
    );
    let result = run.join().unwrap();
    assert!(!result.ok);
    assert_eq!(result.session_id, "agent:main:acp:cancel-session");
    assert_eq!(result.turn_status, "cancelled");
    let _ = fs::remove_dir_all(directory);
}

const FAKE_OPENCLAW_CANCEL_SOURCE: &str = r###"
use std::io::{self, BufRead, Write};

fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    assert_eq!(args, ["acp", "--url", "ws://127.0.0.1:9"]);
    let stdin = io::stdin();
    let mut lines = stdin.lock().lines();

    let initialize = lines.next().unwrap().unwrap();
    assert!(initialize.contains("\"method\":\"initialize\""));
    println!(r#"{{"jsonrpc":"2.0","id":1,"result":{{"protocolVersion":1,"agentCapabilities":{{"loadSession":true}},"agentInfo":{{"name":"openclaw-acp","version":"test"}}}}}}"#);
    io::stdout().flush().unwrap();

    let session = lines.next().unwrap().unwrap();
    assert!(session.contains("\"method\":\"session/new\""));
    println!(r#"{{"jsonrpc":"2.0","method":"session/update","params":{{"sessionId":"protocol-cancel-session","update":{{"sessionUpdate":"session_info_update","_meta":{{"sessionKey":"agent:main:acp:cancel-session"}}}}}}}}"#);
    println!(r#"{{"jsonrpc":"2.0","id":2,"result":{{"sessionId":"protocol-cancel-session"}}}}"#);
    io::stdout().flush().unwrap();

    let prompt = lines.next().unwrap().unwrap();
    assert!(prompt.contains("\"method\":\"session/prompt\""));
    assert!(prompt.contains("\"sessionId\":\"protocol-cancel-session\""));
    let cancel = lines.next().unwrap().unwrap();
    assert!(cancel.contains("\"method\":\"session/cancel\""));
    assert!(cancel.contains("\"sessionId\":\"protocol-cancel-session\""));
    println!(r#"{{"jsonrpc":"2.0","id":4,"result":{{"stopReason":"cancelled"}}}}"#);
    io::stdout().flush().unwrap();
}
"###;
