use std::fs;
use std::io::{self, BufRead, Write};

fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args != ["--mode", "rpc", "--offline"] {
        std::process::exit(2);
    }
    let resume_mode = std::env::current_exe()
        .ok()
        .and_then(|mut path| {
            path.set_extension("resume-mode");
            fs::read_to_string(path).ok()
        })
        .unwrap_or_default();
    let resume_mode = resume_mode.trim().to_owned();
    let expected_session = std::env::current_exe()
        .ok()
        .and_then(|mut path| {
            path.set_extension("session-id");
            fs::read_to_string(path).ok()
        })
        .unwrap_or_else(|| "pi-exact-session".to_owned());
    let expected_session = expected_session.trim().to_owned();

    let stdin = io::stdin();
    let mut stdout = io::stdout().lock();
    for line in stdin.lock().lines() {
        let Ok(line) = line else {
            std::process::exit(3);
        };
        let request_id = json_string_field(&line, "id").unwrap_or_else(|| "lico-pi".to_owned());
        if line.contains("\"type\":\"switch_session\"") {
            send(
                &mut stdout,
                &format!(
                    r#"{{"id":"{request_id}","type":"response","command":"switch_session","success":true,"data":{{"cancelled":false}}}}"#
                ),
            );
        } else if line.contains("\"type\":\"get_state\"") {
            let session_id = if resume_mode == "mismatch" {
                "pi-other-session"
            } else if resume_mode == "exact" {
                expected_session.as_str()
            } else {
                "pi-started-session"
            };
            send(
                &mut stdout,
                &format!(
                    r#"{{"id":"{request_id}","type":"response","command":"get_state","success":true,"data":{{"sessionId":"{session_id}"}}}}"#
                ),
            );
        } else if line.contains("\"type\":\"prompt\"") {
            send(
                &mut stdout,
                &format!(
                    r#"{{"id":"{request_id}","type":"response","command":"prompt","success":true}}"#
                ),
            );
            send(
                &mut stdout,
                r#"{"type":"message_update","assistantMessageEvent":{"type":"text_delta","delta":"pi-ok"}}"#,
            );
            send(&mut stdout, r#"{"type":"agent_settled"}"#);
        } else if line.contains("\"type\":\"get_last_assistant_text\"") {
            send(
                &mut stdout,
                &format!(
                    r#"{{"id":"{request_id}","type":"response","command":"get_last_assistant_text","success":true,"data":{{"text":"pi-ok"}}}}"#
                ),
            );
        }
    }
}

fn json_string_field(value: &str, field: &str) -> Option<String> {
    let marker = format!("\"{field}\":\"");
    let tail = value.split_once(&marker)?.1;
    Some(tail.split_once('"')?.0.to_string())
}

fn send(stdout: &mut impl Write, message: &str) {
    stdout.write_all(message.as_bytes()).unwrap();
    stdout.write_all(b"\n").unwrap();
    stdout.flush().unwrap();
}
