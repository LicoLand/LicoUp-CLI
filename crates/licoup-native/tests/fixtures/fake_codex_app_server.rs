use std::fs;
use std::io::{self, BufRead, Write};

const EXPECTED_PROMPT: &str = "fake-child-private-prompt";
const STEER_PROMPT: &str = "fake-codex-steer-prompt";
const STEER_GUIDANCE: &str = "fake-codex-steer-guidance";

fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args != ["app-server", "--stdio"] {
        std::process::exit(2);
    }
    let configured_output = std::env::current_exe().ok().and_then(|mut path| {
        path.set_extension("result.json");
        fs::read_to_string(path).ok()
    });
    let steering_mode = std::env::current_exe()
        .map(|mut path| {
            path.set_extension("steer-mode");
            path.is_file()
        })
        .unwrap_or(false);
    let cancel_mode = std::env::var("LICO_FAKE_CODEX_CANCEL_MODE")
        .ok()
        .is_some_and(|value| value == "1")
        || std::env::current_exe()
            .map(|mut path| {
                path.set_extension("cancel-mode");
                path.is_file()
            })
            .unwrap_or(false);
    let configured_identity = std::env::current_exe().ok().and_then(|mut path| {
        path.set_extension("identity");
        fs::read_to_string(path).ok()
    });
    let configured_identity = configured_identity.and_then(|value| {
        let mut lines = value.lines().map(str::trim).filter(|line| !line.is_empty());
        let thread_id = lines.next()?.to_owned();
        let turn_id = lines.next()?.to_owned();
        Some((thread_id, turn_id))
    });
    let (thread_id, turn_id) = if let Some(identity) = configured_identity {
        identity
    } else if steering_mode && cancel_mode {
        ("fake-cancel-thread".to_owned(), "fake-cancel-turn".to_owned())
    } else if steering_mode {
        ("fake-steer-thread".to_owned(), "fake-steer-turn".to_owned())
    } else {
        ("fake-thread".to_owned(), "fake-turn".to_owned())
    };
    let chunk_mode = std::env::current_exe()
        .map(|mut path| {
            path.set_extension("chunk-mode");
            path.is_file()
        })
        .unwrap_or(false);

    let resume_mode = std::env::current_exe()
        .ok()
        .and_then(|mut path| {
            path.set_extension("resume-mode");
            fs::read_to_string(path).ok()
        })
        .unwrap_or_default();
    let resume_mode = resume_mode.trim().to_owned();

    let stdin = io::stdin();
    let mut stdout = io::stdout().lock();
    let mut awaiting_steer = false;
    let mut unarchived = false;
    for line in stdin.lock().lines() {
        let Ok(line) = line else {
            std::process::exit(3);
        };
        if line.contains("\"method\":\"initialize\"") {
            send(
                &mut stdout,
                r#"{"id":1,"result":{"codexHome":"/redacted","platformFamily":"test","platformOs":"test","userAgent":"fake-codex"}}"#,
            );
        } else if line.contains("\"method\":\"account/rateLimits/read\"") {
            let request_id = json_request_id(&line).unwrap_or_else(|| "4".to_owned());
            send(
                &mut stdout,
                &format!(
                    r#"{{"id":{request_id},"result":{{"ordinaryUsageAllowed":true,"rateLimits":{{"primary":{{"usedPercent":0}}}}}}}}"#
                ),
            );
        } else if line.contains("\"method\":\"thread/resume\"") {
            let thread_id = json_string_field(&line, "threadId").unwrap_or_default();
            let request_id = json_request_id(&line).unwrap_or_else(|| "2".to_owned());
            if resume_mode == "archived" && !unarchived {
                send(
                    &mut stdout,
                    &format!(
                        r#"{{"id":{request_id},"error":{{"message":"session {thread_id} is archived"}}}}"#
                    ),
                );
            } else if resume_mode == "mismatch" {
                send(
                    &mut stdout,
                    &format!(
                        r#"{{"id":{request_id},"result":{{"cwd":"/workspace/project","thread":{{"id":"other-thread","sessionId":"other-session","cwd":"/workspace/project"}}}}}}"#
                    ),
                );
            } else {
                send(
                    &mut stdout,
                    &format!(
                        r#"{{"id":{request_id},"result":{{"cwd":"/workspace/project","thread":{{"id":"{thread_id}","sessionId":"{thread_id}","cwd":"/workspace/project"}}}}}}"#
                    ),
                );
            }
        } else if line.contains("\"method\":\"thread/unarchive\"") {
            let thread_id = json_string_field(&line, "threadId").unwrap_or_default();
            let request_id = json_request_id(&line).unwrap_or_else(|| "5".to_owned());
            unarchived = true;
            send(
                &mut stdout,
                &format!(r#"{{"id":{request_id},"result":{{"thread":{{"id":"{thread_id}"}}}}}}"#),
            );
        } else if line.contains("\"method\":\"thread/start\"") {
            if let Ok(mut path) = std::env::current_exe() {
                path.set_extension("turn-start.seen");
                let seen = if line.contains("PRODUCTION-GRANT-SENTINEL") {
                    "1"
                } else {
                    "0"
                };
                let _ = fs::write(path, seen);
                if line.contains("OUT-OF-SCOPE-SECRET") {
                    let mut leak = std::env::current_exe().expect("fake exe");
                    leak.set_extension("leak.seen");
                    let _ = fs::write(leak, "1");
                }
            }
            send(
                &mut stdout,
                &format!(
                    r#"{{"id":2,"result":{{"approvalPolicy":"never","approvalsReviewer":"user","cwd":"/workspace/project","model":"fake-default","modelProvider":"openai","reasoningEffort":"medium","sandbox":{{"type":"workspaceWrite","writableRoots":[]}},"thread":{{"id":"{thread_id}","sessionId":"fake-session","cwd":"/workspace/project"}}}}}}"#
                ),
            );
        } else if line.contains("\"method\":\"turn/start\"") {
            if let Ok(mut path) = std::env::current_exe() {
                path.set_extension("turn-start.seen");
                let seen = if line.contains("PRODUCTION-GRANT-SENTINEL") {
                    "1"
                } else {
                    "0"
                };
                let _ = fs::write(path, seen);
                if line.contains("OUT-OF-SCOPE-SECRET") {
                    let mut leak = std::env::current_exe().expect("fake exe");
                    leak.set_extension("leak.seen");
                    let _ = fs::write(leak, "1");
                }
            }
            if configured_output.is_none()
                && !line.contains(EXPECTED_PROMPT)
                && !line.contains(STEER_PROMPT)
            {
                std::process::exit(4);
            }
            io::stderr()
                .lock()
                .write_all(&vec![b'x'; 128 * 1024])
                .unwrap();
            send(
                &mut stdout,
                &format!(
                    r#"{{"id":3,"result":{{"turn":{{"id":"{turn_id}","items":[],"status":"inProgress"}}}}}}"#
                ),
            );
            if line.contains(STEER_PROMPT) {
                awaiting_steer = true;
                continue;
            }
            let final_answer = configured_output
                .as_deref()
                .unwrap_or("fake child final answer");
            if chunk_mode {
                for delta in split_text_chunks(final_answer) {
                    send(
                        &mut stdout,
                        &format!(
                            r#"{{"method":"item/agentMessage/delta","params":{{"threadId":"fake-thread","turnId":"fake-turn","delta":{}}}}}"#,
                            json_string(&delta)
                        ),
                    );
                }
                send(
                    &mut stdout,
                    &format!(
                        r#"{{"method":"item/completed","params":{{"threadId":"fake-thread","turnId":"fake-turn","item":{{"id":"fake-agent-message","type":"agentMessage","text":{}}}}}}}"#,
                        json_string(final_answer)
                    ),
                );
            }
            send(
                &mut stdout,
                &format!(
                    r#"{{"method":"turn/completed","params":{{"threadId":"fake-thread","turn":{{"id":"fake-turn","items":[{{"id":"fake-agent-message","type":"agentMessage","text":{}}}],"status":"completed"}}}}}}"#,
                    json_string(final_answer)
                ),
            );
        } else if line.contains("\"method\":\"turn/steer\"") {
            if !awaiting_steer
                || !line.contains(STEER_GUIDANCE)
                || json_string_field(&line, "threadId").as_deref() != Some(&thread_id)
                || json_string_field(&line, "expectedTurnId").as_deref() != Some(&turn_id)
            {
                std::process::exit(5);
            }
            let Some(request_id) = json_string_field(&line, "id") else {
                std::process::exit(6);
            };
            send(
                &mut stdout,
                &format!(r#"{{"id":{},"result":{{}}}}"#, json_string(&request_id)),
            );
            if cancel_mode {
                continue;
            }
            send(
                &mut stdout,
                &format!(
                    r#"{{"method":"turn/completed","params":{{"threadId":"{thread_id}","turn":{{"id":"{turn_id}","items":[{{"id":"fake-agent-message","type":"agentMessage","text":"fake child guided answer"}}],"status":"completed"}}}}}}"#
                ),
            );
            awaiting_steer = false;
        } else if line.contains("\"method\":\"turn/interrupt\"") {
            if !steering_mode {
                std::process::exit(7);
            }
            let Some(request_id) =
                json_string_field(&line, "id").or_else(|| json_request_id(&line))
            else {
                std::process::exit(8);
            };
            send(
                &mut stdout,
                &format!(r#"{{"id":{},"result":{{}}}}"#, json_string(&request_id)),
            );
            if cancel_mode {
                let thread_id =
                    json_string_field(&line, "threadId").unwrap_or_else(|| thread_id.clone());
                let turn_id = json_string_field(&line, "turnId")
                    .or_else(|| json_string_field(&line, "expectedTurnId"))
                    .unwrap_or_else(|| turn_id.clone());
                if let Ok(mut path) = std::env::current_exe() {
                    path.set_extension("interrupt.json");
                    let _ = fs::write(
                        path,
                        format!(
                            r#"{{"method":"turn/interrupt","threadId":"{thread_id}","turnId":"{turn_id}"}}"#
                        ),
                    );
                }
                send(
                    &mut stdout,
                    &format!(
                        r#"{{"method":"turn/completed","params":{{"threadId":"{thread_id}","turn":{{"id":"{turn_id}","items":[],"status":"cancelled"}}}}}}"#
                    ),
                );
                awaiting_steer = false;
            }
        }
    }
}

fn json_request_id(value: &str) -> Option<String> {
    let marker = "\"id\":";
    let tail = value.split_once(marker)?.1.trim_start();
    if let Some(rest) = tail.strip_prefix('"') {
        return Some(rest.split_once('"')?.0.to_string());
    }
    let end = tail
        .find(|character: char| !character.is_ascii_digit())
        .unwrap_or(tail.len());
    let id = &tail[..end];
    if id.is_empty() {
        None
    } else {
        Some(id.to_owned())
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

fn split_text_chunks(value: &str) -> Vec<String> {
    if value.is_empty() {
        return Vec::new();
    }
    let bytes = value.as_bytes();
    let first = split_after_escape(bytes, bytes.len() / 3);
    let second = split_after_escape(bytes, (bytes.len() * 2) / 3).max(first);
    let mut chunks = Vec::new();
    if first > 0 {
        chunks.push(value[..first].to_owned());
    }
    if second > first {
        chunks.push(value[first..second].to_owned());
    }
    if second < value.len() {
        chunks.push(value[second..].to_owned());
    }
    if chunks.len() < 2 {
        let mid = value.len() / 2;
        vec![value[..mid].to_owned(), value[mid..].to_owned()]
    } else {
        chunks
    }
}

fn split_after_escape(bytes: &[u8], hint: usize) -> usize {
    let mut index = hint.min(bytes.len());
    while index > 0 && !value_is_char_boundary(bytes, index) {
        index -= 1;
    }
    if let Some(slash) = bytes.get(..index).and_then(|prefix| {
        prefix
            .iter()
            .rposition(|byte| *byte == b'\\')
            .map(|pos| pos + 1)
    }) {
        if value_is_char_boundary(bytes, slash) && slash > 0 && slash < bytes.len() {
            return slash;
        }
    }
    index
}

fn value_is_char_boundary(bytes: &[u8], index: usize) -> bool {
    index == bytes.len()
        || bytes
            .get(index)
            .is_some_and(|byte| (byte & 0b1100_0000) != 0b1000_0000)
}

fn json_string(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len() + 2);
    encoded.push('"');
    for character in value.chars() {
        match character {
            '"' => encoded.push_str("\\\""),
            '\\' => encoded.push_str("\\\\"),
            '\n' => encoded.push_str("\\n"),
            '\r' => encoded.push_str("\\r"),
            '\t' => encoded.push_str("\\t"),
            character if character.is_control() => {
                use std::fmt::Write as _;
                write!(&mut encoded, "\\u{:04x}", character as u32).unwrap();
            }
            character => encoded.push(character),
        }
    }
    encoded.push('"');
    encoded
}
