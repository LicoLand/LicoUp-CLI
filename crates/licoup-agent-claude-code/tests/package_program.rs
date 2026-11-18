//! The package program's own observable behaviour.
//!
//! The entry the manifest declares is the process an extension host starts, so
//! what it answers is part of this package's contract rather than an internal
//! detail: a description produced by the same constants the crate registers, and
//! an explicit refusal for a request it does not implement. Both are checked
//! here against the real compiled binary, not a re-implementation of it.

use std::io::{Read, Write};
use std::process::{Command, Stdio};

/// The entry the package manifest declares.
const PROGRAM: &str = env!("CARGO_BIN_EXE_lico-agent-claude-code");

fn spawn() -> std::process::Child {
    Command::new(PROGRAM)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the package program starts")
}

fn read_stdout(child: &mut std::process::Child) -> String {
    let mut output = String::new();
    child
        .stdout
        .as_mut()
        .expect("the program pipes its answers")
        .read_to_string(&mut output)
        .expect("the program's answers are readable");
    output
}

#[test]
fn the_package_program_describes_the_adapter_this_crate_registers() {
    let mut child = Command::new(PROGRAM)
        .arg("--describe")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the package program starts");
    let output = read_stdout(&mut child);
    let status = child.wait().expect("the program exits");
    assert!(status.success(), "the program answers once and exits");

    let described: serde_json::Value =
        serde_json::from_str(&output).expect("the answer is one JSON document");
    assert_eq!(described["packageId"], "org.licoland.adapter.claude-code");
    assert_eq!(
        described["adapterId"],
        licoup_agent_claude_code::registration::ADAPTER_ID
    );
    assert_eq!(
        described["framing"],
        licoup_agent_claude_code::registration::FRAMING
    );
    assert_eq!(
        described["contract"],
        licoup_agent_claude_code::registration::CONTRACT.inventory_json()
    );
    assert_eq!(described["hostProtocol"]["major"], 1);
}

#[test]
fn the_package_program_refuses_a_request_it_does_not_implement() {
    let mut child = spawn();
    {
        let stdin = child
            .stdin
            .as_mut()
            .expect("the program reads its requests");
        stdin
            .write_all(
                b"{\"method\":\"describe\"}\n{\"method\":\"not-implemented\"}\nnot-json\n{\"method\":\"shutdown\"}\n{\"method\":\"describe\"}\n",
            )
            .expect("the requests are writable");
    }
    let output = read_stdout(&mut child);
    let status = child.wait().expect("the program exits");
    assert!(status.success());

    let answers: Vec<serde_json::Value> = output
        .lines()
        .map(|line| serde_json::from_str(line).expect("each answer is one JSON document"))
        .collect();
    assert_eq!(
        answers.len(),
        4,
        "the shutdown ends the stream: {answers:?}"
    );
    assert_eq!(answers[0]["ok"], true);
    assert_eq!(
        answers[0]["result"]["adapterId"],
        licoup_agent_claude_code::registration::ADAPTER_ID
    );
    assert_eq!(answers[1]["ok"], false);
    assert_eq!(answers[1]["code"], "claude_code_package_method_unsupported");
    assert_eq!(answers[2]["code"], "claude_code_package_request_invalid");
    assert_eq!(answers[3]["result"]["stopped"], true);
}

#[test]
fn the_package_program_refuses_an_option_it_does_not_implement() {
    let mut child = Command::new(PROGRAM)
        .arg("--not-an-option")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the package program starts");
    let output = read_stdout(&mut child);
    let _ = child.wait().expect("the program exits");
    let answer: serde_json::Value =
        serde_json::from_str(&output).expect("the answer is one JSON document");
    assert_eq!(answer["ok"], false);
    assert_eq!(answer["code"], "claude_code_package_option_unsupported");
}
