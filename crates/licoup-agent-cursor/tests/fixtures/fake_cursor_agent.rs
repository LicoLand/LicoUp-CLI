use std::env;
use std::io::{self, Write};

fn emit(value: &str) {
    println!("{value}");
    io::stdout().flush().unwrap();
}

fn help_text() {
    println!("Cursor Agent CLI");
    println!("  create-chat");
    println!("  --print");
    println!("  --resume");
    println!("  --output-format stream-json");
    println!("  --stream-partial-output");
    println!("  --trust");
    println!("  --force");
    println!("  --approve-mcps");
    println!("  --workspace");
}

fn create_session_id(counter: u64) -> String {
    // Test hook: a per-test tag keeps session ids unique so a parallel test
    // can cancel its own turn without hitting another test's registered
    // session (the active-turn registry is keyed by session id).
    let tag = env::var("LICO_FAKE_CURSOR_AGENT_SESSION_TAG").unwrap_or_default();
    if tag.is_empty() {
        format!("fake-cursor-session-{counter:012x}")
    } else {
        format!("fake-cursor-session-{tag}-{counter:012x}")
    }
}

fn turn_output(prompt: &str) -> (&'static str, &'static str) {
    if prompt.contains("private first prompt") {
        ("first response", "first response")
    } else if prompt.contains("private follow-up prompt") {
        ("second response", "second response")
    } else if prompt.contains("41") {
        ("41", "41")
    } else if prompt.contains("43") {
        ("43", "43")
    } else {
        ("fake Cursor final answer", "fake Cursor final answer")
    }
}

fn split_fragments(reply: &str) -> (String, String) {
    // Progressive stream: the reply arrives as two assistant delta frames.
    // Split at the first space so every tested reply keeps an exact
    // first + second concatenation budget, then fall back to the midpoint.
    if let Some(index) = reply.find(' ') {
        if index > 0 {
            return (reply[..index].to_owned(), reply[index..].to_owned());
        }
    }
    let mid = reply.len() / 2;
    (reply[..mid].to_owned(), reply[mid..].to_owned())
}

fn json_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(ch),
        }
    }
    out.push('"');
    out
}

#[cfg(unix)]
fn close_turn_stream() {
    unsafe extern "C" {
        fn close(fd: i32) -> i32;
    }
    // The Cursor turn transport uses one pty slave for stdin/stdout. Closing
    // both descriptors produces a real EOF on the driver's pty reader while
    // this synthetic process can remain alive for the test to release.
    unsafe {
        close(0);
        close(1);
    }
}

fn run_turn(args: &[String]) {
    if env::var("LICO_FAKE_CURSOR_AGENT_REQUIRE_CALLER_CONTEXT").is_ok() {
        assert!(
            env::var("LICOUP_HOME")
                .map(std::path::PathBuf::from)
                .is_ok_and(|path| path.is_absolute())
        );
    }
    let resume_index = args
        .iter()
        .position(|arg| arg == "--resume")
        .expect("--resume");
    let session_id = args
        .get(resume_index + 1)
        .filter(|value| !value.is_empty())
        .expect("session id");
    let prompt = args.last().expect("prompt");
    // Test hook: the turn stream reports a different native conversation than
    // the one the driver launched. Gated on a prompt marker and an explicit
    // env var so no other fake invocation is affected.
    let observed_session = if env::var("LICO_FAKE_CURSOR_AGENT_DRIFT_SESSION_ID").is_ok()
        && prompt.contains("__lico_drift__")
    {
        "drifted-cursor-session".to_string()
    } else {
        session_id.clone()
    };
    // Test hook: hold the turn open until the update-watcher fixture has
    // observed its completion transition. The wait is bounded so a broken
    // fixture cannot strand the test process.
    if let Ok(release_path) = env::var("LICO_FAKE_CURSOR_AGENT_UPDATE_RELEASE_PATH") {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        while !std::path::Path::new(&release_path).is_file() && std::time::Instant::now() < deadline
        {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    } else if let Ok(delay_ms) = env::var("LICO_FAKE_CURSOR_AGENT_UPDATE_DELAY_MS") {
        if let Ok(delay) = delay_ms.parse::<u64>() {
            std::thread::sleep(std::time::Duration::from_millis(delay));
        }
    }
    // Test hook: delay the turn output independently of the update watcher.
    // Gated on a prompt marker so a concurrent test's env vars never delay a
    // fake spawned by another test.
    if prompt.contains("__lico_test__") {
        if let Ok(delay_ms) = env::var("LICO_FAKE_CURSOR_AGENT_TURN_DELAY_MS") {
            if let Ok(delay) = delay_ms.parse::<u64>() {
                std::thread::sleep(std::time::Duration::from_millis(delay));
            }
        }
    }
    let (reply, _) = turn_output(prompt);
    let (first_fragment, second_fragment) = split_fragments(reply);
    // Real Cursor stream-json: system init, then a user prompt echo, then
    // progressive assistant fragments, then the cumulative result.
    emit(&format!(
        r#"{{"type":"system","subtype":"init","apiKeySource":"synthetic","cwd":"/tmp","session_id":{},"model":"fake-model","permissionMode":"default"}}"#,
        json_string(&observed_session)
    ));
    emit(&format!(
        r#"{{"type":"user","session_id":{},"message":{{"role":"user","content":[{{"type":"text","text":{}}}]}}}}"#,
        json_string(&observed_session),
        json_string(prompt)
    ));
    if prompt.contains("__lico_stderr_usage__") {
        eprintln!("Usage limit exceeded for private fixture account context");
        std::process::exit(4);
    }
    emit(&format!(
        r#"{{"type":"assistant","session_id":{},"timestamp_ms":1,"message":{{"role":"assistant","content":[{{"type":"text","text":{}}}]}}}}"#,
        json_string(&observed_session),
        json_string(&first_fragment)
    ));
    #[cfg(unix)]
    if prompt.contains("__lico_stdout_eof__") {
        if let Ok(ready_path) = env::var("LICO_FAKE_CURSOR_AGENT_EOF_READY_PATH") {
            std::fs::write(ready_path, b"ready").unwrap();
        }
        close_turn_stream();
        if let Ok(release_path) = env::var("LICO_FAKE_CURSOR_AGENT_EOF_RELEASE_PATH") {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
            while !std::path::Path::new(&release_path).is_file()
                && std::time::Instant::now() < deadline
            {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
        return;
    }
    // Test hook: crash after the partial chunk, before any terminal result.
    // Gated on a prompt marker so a concurrent test's env vars can never
    // crash a fake spawned by another test.
    if env::var("LICO_FAKE_CURSOR_AGENT_CRASH_AFTER_CHUNK").is_ok()
        && prompt.contains("__lico_crash__")
    {
        std::process::exit(3);
    }
    emit(&format!(
        r#"{{"type":"assistant","session_id":{},"timestamp_ms":2,"message":{{"role":"assistant","content":[{{"type":"text","text":{}}}]}}}}"#,
        json_string(&observed_session),
        json_string(&second_fragment)
    ));
    // Current Cursor emits one timestamp-free cumulative assistant snapshot
    // after the timestamped partial deltas and before the cumulative result.
    emit(&format!(
        r#"{{"type":"assistant","session_id":{},"message":{{"role":"assistant","content":[{{"type":"text","text":{}}}]}}}}"#,
        json_string(&observed_session),
        json_string(reply)
    ));
    let terminal_subtype = if prompt.contains("__lico_non_error_subtype__") {
        "error_during_execution"
    } else {
        "success"
    };
    emit(&format!(
        r#"{{"type":"result","subtype":{},"is_error":false,"session_id":{},"request_id":"req-{}","result":{}}}"#,
        json_string(terminal_subtype),
        json_string(&observed_session),
        session_id,
        json_string(reply)
    ));
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    if args == ["--version"] {
        println!("fake-cursor-agent 1.0.0");
        return;
    }
    if args == ["--help"] {
        help_text();
        return;
    }
    if args == ["create-chat"] {
        if env::var("LICO_FAKE_CURSOR_AGENT_REQUIRE_CALLER_CONTEXT").is_ok() {
            assert!(
                env::var("LICOUP_HOME")
                    .map(std::path::PathBuf::from)
                    .is_ok_and(|path| path.is_absolute())
            );
            assert_eq!(
                env::var("LICOUP_MCP_CONVERSATION_ID").as_deref(),
                Ok("conversation:fixture")
            );
            assert_eq!(
                env::var("LICOUP_MCP_MEMBERSHIP_ID").as_deref(),
                Ok("membership:cursor")
            );
            assert_eq!(
                env::var("LICOUP_MCP_PARENT_DISPATCH_ID").as_deref(),
                Ok("subagent:parent")
            );
        }
        // Test hook: delay session creation so the driver can be exercised
        // with a create-chat phase that consumes part of the turn window.
        if let Ok(delay_ms) = env::var("LICO_FAKE_CURSOR_AGENT_CREATE_CHAT_DELAY_MS") {
            if let Ok(delay) = delay_ms.parse::<u64>() {
                std::thread::sleep(std::time::Duration::from_millis(delay));
            }
        }
        let session_id = create_session_id(1);
        println!("{session_id}");
        return;
    }
    if args.first().map(String::as_str) == Some("acp") {
        eprintln!("ACP entrypoint is not supported");
        std::process::exit(2);
    }
    if args.iter().any(|arg| arg == "--print")
        && args.iter().any(|arg| arg == "--resume")
        && args.iter().any(|arg| arg == "stream-json")
        && args.iter().any(|arg| arg == "--approve-mcps")
        && args.iter().any(|arg| arg == "--stream-partial-output")
    {
        run_turn(&args);
        return;
    }
    std::process::exit(2);
}
