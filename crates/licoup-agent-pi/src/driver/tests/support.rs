use super::*;

pub(super) fn absolute_test_cwd() -> PathBuf {
    std::env::current_dir().expect("test working directory")
}

pub(super) fn resume_config(
    prompt: &str,
    session_id: &str,
    session_path: PathBuf,
) -> ProtocolConfig {
    ProtocolConfig {
        prompt: prompt.to_string(),
        requested_session_id: session_id.to_string(),
        resume_session_path: Some(session_path),
        cwd: absolute_test_cwd().to_string_lossy().to_string(),
        model: None,
        model_provider: None,
        model_id: None,
        thinking_level: None,
        turn_id: "test-turn".to_string(),
    }
}

pub(super) fn temporary_directory(prefix: &str) -> PathBuf {
    let nonce = uuid::Uuid::new_v4().simple();
    let path = std::env::temp_dir().join(format!("{prefix}-{nonce}"));
    fs::create_dir_all(&path).unwrap();
    path
}

pub(super) fn compile_fake_pi(prefix: &str) -> (PathBuf, PathBuf) {
    let directory = temporary_directory(prefix);
    let source = directory.join("fake_pi.rs");
    let executable = directory.join(format!("fake-pi{}", std::env::consts::EXE_SUFFIX));
    fs::write(&source, FAKE_PI_SOURCE).unwrap();
    let status = std::process::Command::new("rustc")
        .args(["--edition", "2021"])
        .arg(&source)
        .arg("-o")
        .arg(&executable)
        .status()
        .unwrap();
    assert!(status.success());
    (directory, executable)
}

pub(super) const FAKE_PI_SOURCE: &str = r#"
use std::io::{self, BufRead, Write};

fn request_id(line: &str) -> &str {
    let marker = "\"id\":\"";
    let start = line.find(marker).unwrap() + marker.len();
    let tail = &line[start..];
    &tail[..tail.find('\"').unwrap()]
}

fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    assert_eq!(args, vec!["--mode", "rpc", "--offline"]);
    assert!(!args.iter().any(|arg| arg.contains("private") || arg.contains("session")));
    let fixture = std::env::current_dir()
        .ok()
        .and_then(|path| path.file_name().map(|name| name.to_string_lossy().into_owned()))
        .unwrap_or_default();
    if fixture.contains("rpc-fake") {
        std::thread::spawn(|| {
            let mut stderr = io::stderr();
            let _ = stderr.write_all(&vec![b'x'; 128 * 1024]);
        });
    }

    let session_id = if fixture.contains("native-steer") {
        "pi-native-steer-1"
    } else if fixture.contains("interaction-exit") {
        "pi-native-interaction-exit-1"
    } else if fixture.contains("interaction") {
        "pi-native-interaction-1"
    } else if fixture.contains("credential") {
        "pi-native-credential-1"
    } else if fixture.contains("timeout") {
        "pi-native-timeout-1"
    } else if fixture.contains("stream") {
        "pi-native-stream-1"
    } else {
        "pi-native-fake-1"
    };
    let mut state_requests = 0usize;
    let mut stream = false;
    let mut awaiting_steer = false;
    let mut guided = false;
    let mut credential_error = false;
    let mut interaction_case = false;
    let mut interaction_completed = false;
    for line in io::stdin().lock().lines() {
        let line = line.unwrap();
        let id = request_id(&line);
        if line.contains("\"type\":\"get_state\"") {
            state_requests += 1;
            println!("{{\"id\":\"{}\",\"type\":\"response\",\"command\":\"get_state\",\"success\":true,\"data\":{{\"sessionId\":\"{}\"}}}}", id, session_id);
            io::stdout().flush().unwrap();
            if interaction_completed {
                break;
            }
            if state_requests > 1 && !credential_error && !interaction_case {
                std::thread::sleep(std::time::Duration::from_secs(5));
            }
        } else if line.contains("\"type\":\"prompt\"") {
            if line.contains("timeout-case") {
                std::thread::sleep(std::time::Duration::from_secs(5));
                continue;
            }
            stream = line.contains("\"message\":\"stream\"");
            println!("{{\"id\":\"{}\",\"type\":\"response\",\"command\":\"prompt\",\"success\":true}}", id);
            if line.contains("steer-case") {
                awaiting_steer = true;
                io::stdout().flush().unwrap();
                continue;
            }
            if line.contains("credential-case") {
                credential_error = true;
                println!("{{\"type\":\"message_end\",\"message\":{{\"role\":\"assistant\",\"content\":[],\"stopReason\":\"error\",\"errorMessage\":\"503: {{\\\"code\\\":\\\"gateway_credential_unavailable\\\"}}\"}}}}");
                println!("{{\"type\":\"agent_settled\"}}");
                io::stdout().flush().unwrap();
                continue;
            }
            if line.contains("interaction-exit-case") {
                println!("{{\"type\":\"extension_ui_request\",\"id\":\"ui-confirm\",\"method\":\"confirm\",\"title\":\"Synthetic confirmation\"}}");
                io::stdout().flush().unwrap();
                let release = std::env::current_dir().unwrap().join("release-interaction-exit");
                let mut released = false;
                for _ in 0..10_000 {
                    if release.exists() {
                        released = true;
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                if !released {
                    std::process::exit(6);
                }
                break;
            }
            if line.contains("interaction-case") {
                interaction_case = true;
                println!("{{\"type\":\"extension_ui_request\",\"id\":\"ui-confirm\",\"method\":\"confirm\",\"title\":\"Synthetic confirmation\"}}");
                io::stdout().flush().unwrap();
                continue;
            }
            if stream {
                println!("{{\"type\":\"message_update\",\"assistantMessageEvent\":{{\"type\":\"text_delta\",\"delta\":\"one\"}}}}");
                println!("{{\"type\":\"message_update\",\"assistantMessageEvent\":{{\"type\":\"text_delta\",\"delta\":\"-two\"}}}}");
            } else {
                println!("{{\"type\":\"message_update\",\"assistantMessageEvent\":{{\"type\":\"text_delta\",\"delta\":\"pi-ok\"}}}}");
            }
            println!("{{\"type\":\"agent_settled\"}}");
            io::stdout().flush().unwrap();
        } else if line.contains("\"type\":\"extension_ui_response\"") {
            if !interaction_case || id != "ui-confirm" || !line.contains("\"confirmed\":true") {
                std::process::exit(5);
            }
            interaction_completed = true;
            println!("{{\"type\":\"message_update\",\"assistantMessageEvent\":{{\"type\":\"text_end\",\"contentIndex\":0,\"content\":\"pi-interaction-ok\"}}}}");
            println!("{{\"type\":\"agent_settled\"}}");
            io::stdout().flush().unwrap();
        } else if line.contains("\"type\":\"steer\"") {
            if !awaiting_steer || !line.contains("pi-native-steer-guidance") {
                std::process::exit(4);
            }
            guided = true;
            awaiting_steer = false;
            println!("{{\"id\":\"{}\",\"type\":\"response\",\"command\":\"steer\",\"success\":true}}", id);
            println!("{{\"type\":\"message_update\",\"assistantMessageEvent\":{{\"type\":\"text_delta\",\"delta\":\"pi-guided\"}}}}");
            println!("{{\"type\":\"agent_settled\"}}");
            io::stdout().flush().unwrap();
        } else if line.contains("\"type\":\"get_last_assistant_text\"") {
            if credential_error {
                println!("{{\"id\":\"{}\",\"type\":\"response\",\"command\":\"get_last_assistant_text\",\"success\":true,\"data\":{{\"text\":null}}}}", id);
            } else {
                let text = if guided { "pi-guided" } else if interaction_case { "pi-interaction-ok" } else if stream { "one-two" } else { "pi-ok" };
                println!("{{\"id\":\"{}\",\"type\":\"response\",\"command\":\"get_last_assistant_text\",\"success\":true,\"data\":{{\"text\":\"{}\"}}}}", id, text);
            }
            io::stdout().flush().unwrap();
        }
    }
}
"#;
