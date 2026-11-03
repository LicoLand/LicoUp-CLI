//! User login-shell environment snapshot for Agent CLI launches.
//!
//! Architecture invariant 4 (docs/architecture/AGENT-ADAPTERS-ARCHITECTURE.md
//! §4, ADR 0007): a CLI subagent spawned by LicoUp must observe exactly the
//! same environment as when the user starts the same CLI from their own
//! terminal login shell — same proxy variables, same PATH, same login state.
//! A GUI-launched LicoUp process carries only the launchd/service session
//! environment, so the user's login shell is asked once per process for its
//! environment and every conversation-execution spawn starts from that
//! snapshot. Each launch site's own functional injections (hook receipts,
//! caller context, portable roots, gateway ports) are applied afterwards and
//! therefore win over the snapshot. Deliberately scrubbed spawns
//! (`process_supervisor::configure_untrusted_agent_command` and the
//! `*/probe.rs` capability probes) never use this snapshot.
//!
//! No global shell or launchd mutation is involved: the snapshot is read-only
//! and private to this process.
//!
//! The capture and application machinery is `a4aa::hostenv`'s — one login
//! shell answers for every adapter on this machine.

pub use a4aa::hostenv::{
    ShellEnvironmentStatus, apply_to_command, get, search_path_dirs, snapshot, status,
};

#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub use a4aa::hostenv::{SnapshotOverrideGuard, pin_process_env_snapshot_for_testing};
