//! Every Agent caller reaches execution through the port.
//!
//! The agent-execution port, [`crate::agent_port`], is the one entry a caller
//! above the platform layer names. This suite is the static search that keeps
//! it that way: the lane's dispatch entries are called from the port module and
//! from the lane's own body, and from nowhere else. A new caller that reaches
//! the lane or the adapter dispatch directly fails here rather than silently
//! becoming a second path to execution.

use std::path::{Path, PathBuf};

/// The port module, which owns the composition's reach into the lane.
const PORT_MODULE: &str = "agent_port.rs";

/// The lane's own body, which implements the entries the port names.
const LANE_MODULE: &str = "platform/conversation_lane.rs";

/// The lane dispatch vocabulary no other module may call.
///
/// A new dispatch entry the lane exposes is added here in the same change that
/// exposes it, so the rule stays complete rather than drifting behind the lane.
const LANE_DISPATCH_ENTRIES: &[&str] = &[
    "conversation_lane::dispatch_lane_operation(",
    "conversation_lane::open_or_resume(",
    "conversation_lane::steer_turn(",
    "conversation_lane::cancel_turn(",
    "conversation_lane::cleanup_conversation(",
    "conversation_lane::process_local_history(",
    "conversation_lane::lane_capabilities(",
    "conversation_lane::send_and_settle(",
    "runtime_adapters::send_message(",
];

fn rust_sources(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

/// A suite is not a caller: these suites drive the lane directly to assert what
/// it answers, which is the lane's own contract rather than a second production
/// path to an Agent. The lane's body and the port module are the two modules
/// the rule exempts by name.
fn is_exempt(relative: &str) -> bool {
    relative.ends_with("tests.rs")
        || relative.contains("/tests/")
        || relative == LANE_MODULE
        || relative == PORT_MODULE
}

#[test]
fn no_agent_dispatch_happens_outside_the_agent_execution_port() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut offenders = Vec::new();
    for file in rust_sources(&root) {
        let relative = file
            .strip_prefix(&root)
            .expect("every source is under the host's source root")
            .to_string_lossy()
            .replace('\\', "/");
        if is_exempt(&relative) {
            continue;
        }
        let Ok(source) = std::fs::read_to_string(&file) else {
            offenders.push(format!("{relative}: source is unreadable"));
            continue;
        };
        for entry in LANE_DISPATCH_ENTRIES {
            if source.contains(entry) {
                offenders.push(format!("{relative}: calls {entry}"));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "Agent dispatch happens outside the agent-execution port:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn the_port_fails_closed_on_an_operation_it_does_not_own() {
    super::compose();
    assert!(
        crate::agent_port::dispatch("not-a-lane-operation", &serde_json::json!({})).is_err(),
        "an operation outside the port's vocabulary is refused, never invented"
    );
}

/// The history entry is the lane's own process-local history page, and an Agent
/// this host composes no lane for has none: the answer is a refusal, never an
/// empty page that a caller would read as "this Agent has no history".
#[test]
fn the_history_entry_refuses_an_agent_this_host_does_not_compose() {
    super::compose();
    assert!(
        crate::agent_port::history(&serde_json::json!({ "agent": "no-such-agent" })).is_err(),
        "a history read for an uncomposed Agent is refused"
    );
}
