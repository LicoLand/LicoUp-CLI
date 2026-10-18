//! The selection command surface over the real stdio RPC frame loop.
//!
//! The routing tests in `stdio_rpc/request.rs` prove the method table reaches
//! the operations. This one proves the same surface answers over the transport
//! the desktop uses: a bounded request frame in, one bounded result or error
//! frame out, the transition visible to the next read, and the loop still
//! serving after a refusal.

use super::super::support::temp_cli_dir;
use super::*;
use licoup_native::ffi::commands::CliExecution;
use std::io::Cursor;

/// One frame line per request, as the transport receives them.
fn rpc_input(requests: &[Value]) -> Cursor<Vec<u8>> {
    let mut bytes = Vec::new();
    for request in requests {
        serde_json::to_writer(&mut bytes, request).unwrap();
        bytes.push(b'\n');
    }
    Cursor::new(bytes)
}

/// The response frames, decoded.
fn rpc_output(bytes: Vec<u8>) -> Vec<Value> {
    String::from_utf8(bytes)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn request_frame(
    id: &str,
    workflow_id: &str,
    method: &str,
    params: Value,
    portable_data_dir: &Path,
) -> Value {
    json!({
        "protocol": STDIO_RPC_PROTOCOL,
        "id": id,
        "workflowId": workflow_id,
        "method": method,
        "params": params,
        "portableDataDir": portable_data_dir,
    })
}

#[test]
fn selection_frames_answer_over_stdio_rpc_and_a_refusal_keeps_the_loop_serving() {
    let root = temp_cli_dir("selection-rpc");
    let cases = [
        // A parameter the operation does not define, refused before any owner.
        (
            "request-refused",
            "selection.matrix",
            json!({"agent": "codex", "environment": "work"}),
        ),
        // Nothing adopted in a fresh data home.
        ("request-empty", "selection.policy.get", json!({})),
        // A complete revision, adopted through this surface.
        (
            "request-adopt",
            "selection.policy.adopt",
            json!({"revision": {
                "revisionId": "policy:rpc",
                "provenance": "feedback:outcome/policy:rpc",
                "preferences": {"preferredModel": "model-a"}
            }}),
        ),
        // The revision the owner refuses: a second adoption of the same record.
        (
            "request-conflict",
            "selection.policy.adopt",
            json!({"revision": {
                "revisionId": "policy:rpc",
                "provenance": "feedback:outcome/policy:rpc",
                "preferences": {"preferredModel": "model-b"}
            }}),
        ),
        // The read after the transition, from the same data home.
        ("request-read", "selection.policy.get", json!({})),
        // Revoking the revision in force restores the unadopted state.
        (
            "request-revoke",
            "selection.policy.revoke",
            json!({"revisionId": "policy:rpc"}),
        ),
    ];
    let requests: Vec<Value> = cases
        .iter()
        .map(|(id, method, params)| {
            request_frame(id, "workflow-selection", method, params.clone(), &root)
        })
        .collect();

    let output = serve_stdio_rpc(
        rpc_input(&requests),
        Vec::new(),
        |_, _| -> anyhow::Result<CliExecution> {
            panic!("a selection method must not fall through to the CLI lane")
        },
    )
    .unwrap();
    let frames = rpc_output(output);
    assert_eq!(frames.len(), cases.len());

    assert_eq!(frames[0]["error"]["code"], "invalid_params");
    assert_eq!(frames[0]["id"], "request-refused");
    assert_eq!(frames[0]["workflowId"], "workflow-selection");

    // A fresh data home reports the owner's own name for "nothing is adopted"
    // instead of an absent field the client would have to interpret.
    assert_eq!(frames[1]["ok"], true);
    assert_eq!(frames[1]["result"]["policy"]["revisionName"], "unadopted");
    assert_eq!(frames[1]["result"]["policy"].get("revisionId"), None);

    // The adoption is the owner's answer, and it is now the policy in force.
    assert_eq!(frames[2]["ok"], true);
    assert_eq!(frames[2]["result"]["policy"]["revisionId"], "policy:rpc");
    assert_eq!(frames[2]["result"]["policy"]["revisionName"], "policy:rpc");
    assert_eq!(
        frames[2]["result"]["policy"]["preferences"]["preferredModel"],
        "model-a"
    );

    // The named refusal is the owner's own code, not a generic command failure.
    assert_eq!(
        frames[3]["error"]["code"],
        "selection_policy_already_adopted"
    );

    // The next read in the same session sees the policy the transition wrote:
    // one durable record, read and written by its owner.
    assert_eq!(frames[4]["ok"], true);
    assert_eq!(frames[4]["result"]["policy"]["revisionId"], "policy:rpc");

    // Revoking the revision in force restores the state before it.
    assert_eq!(frames[5]["ok"], true);
    assert_eq!(frames[5]["result"]["policy"]["revisionName"], "unadopted");

    let _ = std::fs::remove_dir_all(root);
}
