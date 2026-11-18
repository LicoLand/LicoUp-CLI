//! The RPC session identity, the transcript layout and the active plan layout
//! this protocol resumes by.
//!
//! Lico Agent's RPC is launched with `--session-id` and optionally `--resume`,
//! and plan mode adds `--plan-path`. That makes three facts the protocol's own
//! rather than the host's: which strings are valid native session identities,
//! where one session's transcript lives, and which plan file a turn is bound
//! to. They are the facts the skill `lico-agent-target-lico-agent` publishes as
//! adapter facts, and they live here rather than beside the process supervisor
//! so a second reader cannot derive a different layout.
//!
//! What this module does **not** own: the transcript's *contents*. Reading and
//! validating a persisted conversation is `licoup-agent-targets`' (the
//! `Agent::load_persisted_history` owner), and this module only states the path
//! that reader is given. The macOS seatbelt profile a plan-mode turn is
//! sandboxed with stays with the platform sandbox that compiles it.
//!
//! Every function here answers with a stable code rather than a sentence: the
//! code is the protocol's, the message is the host's presentation.

use licoup_foundation::platform::file_security::ensure_private_dir;
use serde_json::Value;
use std::path::{Path, PathBuf};

/// Where one data root keeps its Lico Agent session transcripts, relative to
/// the root.
pub const SESSIONS_RELATIVE_PATH: &str = "client-state/lico-agent/sessions";

/// Where one data root keeps the plans a plan-mode turn writes, relative to the
/// root.
pub const PLAN_DIRECTORY_RELATIVE_PATH: &str = "client-state/plans";

/// The active plan file inside [`PLAN_DIRECTORY_RELATIVE_PATH`].
pub const ACTIVE_PLAN_FILE: &str = "active-plan.md";

/// The transcript file name extension one session's records are written to.
pub const TRANSCRIPT_EXTENSION: &str = "jsonl";

/// The requested native session identity is not one this protocol accepts.
pub const SESSION_ID_INVALID: &str = "lico_agent_session_id_invalid";

/// No unused native session identity could be allocated.
pub const SESSION_ID_UNAVAILABLE: &str = "lico_agent_session_id_unavailable";

/// The session store could not be opened.
pub const SESSION_STORE_UNAVAILABLE: &str = "lico_agent_session_store_unavailable";

/// The `params` key a caller may bind an absolute plan path with.
const PLAN_PATH_KEYS: [&str; 2] = ["planPath", "plan_path"];

/// How many identities the allocator tries before it reports exhaustion.
///
/// It is a bounded loop rather than an unbounded one: a store that keeps
/// answering with an existing path is a store this turn may not write into, and
/// a refusal is the honest answer.
const ALLOCATION_ATTEMPTS: usize = 8;

/// One session this host is about to run a turn against.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreparedSession {
    /// The canonical native session identity the program is launched with.
    pub session_id: String,
    /// Whether the program must be launched with `--resume`: a caller-named
    /// session continues a conversation that already exists.
    pub resume: bool,
    /// The transcript this identity's records belong to.
    pub transcript: PathBuf,
}

/// The canonical form of a caller-named native session identity.
///
/// Only a canonical UUID is accepted, and it is accepted only in its canonical
/// spelling: a program that resumed a differently-spelled identity would open a
/// second transcript for one conversation, so a non-canonical spelling is
/// refused rather than normalized.
pub fn canonical_session_id(raw: &str) -> Result<String, &'static str> {
    let trimmed = raw.trim();
    let canonical = uuid::Uuid::parse_str(trimmed)
        .map_err(|_| SESSION_ID_INVALID)?
        .to_string();
    if canonical != trimmed {
        return Err(SESSION_ID_INVALID);
    }
    Ok(canonical)
}

/// The directory one data root keeps its session transcripts in.
pub fn sessions_dir(root: &Path) -> PathBuf {
    root.join(SESSIONS_RELATIVE_PATH)
}

/// The transcript one native session identity's records are written to.
pub fn transcript_path(sessions_dir: &Path, session_id: &str) -> PathBuf {
    sessions_dir.join(format!("{session_id}.{TRANSCRIPT_EXTENSION}"))
}

/// The active plan file one data root carries.
pub fn active_plan_path(root: &Path) -> PathBuf {
    root.join(PLAN_DIRECTORY_RELATIVE_PATH)
        .join(ACTIVE_PLAN_FILE)
}

/// Open the session store under one data root, privately.
///
/// The directory is created and hardened through the shared private-path rule,
/// so a session store a neighbouring owner would refuse is refused here too.
pub fn ensure_sessions_dir(sessions_dir: &Path) -> Result<(), &'static str> {
    ensure_private_dir(sessions_dir).map_err(|_| SESSION_STORE_UNAVAILABLE)
}

/// Resolve the session one turn runs against, allocating an identity when the
/// caller named none.
///
/// A caller-named identity is canonicalized and its transcript path returned as
/// `resume`; whether that transcript is readable, well-formed and about that
/// identity is the transcript owner's answer, not this module's, so the caller
/// asks the owner next.
pub fn prepare(root: &Path, session_id: &str) -> Result<PreparedSession, &'static str> {
    let sessions_dir = sessions_dir(root);
    ensure_sessions_dir(&sessions_dir)?;
    if !session_id.trim().is_empty() {
        let session_id = canonical_session_id(session_id)?;
        let transcript = transcript_path(&sessions_dir, &session_id);
        return Ok(PreparedSession {
            session_id,
            resume: true,
            transcript,
        });
    }
    for _ in 0..ALLOCATION_ATTEMPTS {
        let session_id = uuid::Uuid::new_v4().to_string();
        let transcript = transcript_path(&sessions_dir, &session_id);
        if !transcript.exists() {
            return Ok(PreparedSession {
                session_id,
                resume: false,
                transcript,
            });
        }
    }
    Err(SESSION_ID_UNAVAILABLE)
}

/// The absolute plan file a caller named in `params`, if any.
///
/// A relative path is not a plan this protocol can bind: the program is
/// launched with the workspace as its working directory, which is not the
/// directory a relative plan path would be resolved against here, so a relative
/// value is refused rather than silently reinterpreted.
pub fn named_plan_path(params: &Value) -> Option<PathBuf> {
    let path = PLAN_PATH_KEYS
        .iter()
        .find_map(|key| params.get(*key).and_then(Value::as_str))?;
    let path = PathBuf::from(path);
    path.is_absolute().then_some(path)
}

/// The data root's active plan file, created empty when the root has none yet
/// so the program is never launched against a path that does not exist.
///
/// Creating it is best effort for the same reason the launch is: a root whose
/// plan cannot be written is reported by the launch, not by a path this
/// function refuses to name.
pub fn ensure_active_plan(root: &Path) -> PathBuf {
    let plan = active_plan_path(root);
    if let Some(dir) = plan.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if !plan.exists() {
        let _ = std::fs::write(&plan, b"");
    }
    plan
}
