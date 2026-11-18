use std::env;
use std::io::{self, BufRead, Write};

fn write_json(value: &str) {
    println!("{value}");
    io::stdout().flush().unwrap();
}

fn id_of(line: &str) -> &str {
    line.split("\"id\":\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap_or("")
}

fn main() {
    let hang = env::var("LICO_FAKE_LICO_AGENT_HANG").is_ok();
    let reject = env::var("LICO_FAKE_LICO_AGENT_REJECT").is_ok();
    let stdin = io::stdin();
    for line in stdin.lock().lines() {
        let Ok(line) = line else {
            return;
        };
        if line.trim().is_empty() {
            continue;
        }
        if hang {
            // Never answer the readiness handshake; keep the process alive so
            // the driver's handshake bound is exercised.
            continue;
        }
        let id = id_of(&line);
        if line.contains("\"get_state\"") {
            if reject {
                write_json(&format!(
                    r#"{{"id":"{id}","type":"response","success":false,"error":"unsupported_request"}}"#
                ));
            } else {
                write_json(&format!(
                    r#"{{"id":"{id}","type":"response","success":true,"data":{{"isRunning":false,"profile":"base"}}}}"#
                ));
            }
        } else if line.contains("\"prompt\"") {
            write_json(&format!(r#"{{"id":"{id}","type":"response","success":true}}"#));
            write_json(r#"{"type":"agent_end"}"#);
        }
    }
}
