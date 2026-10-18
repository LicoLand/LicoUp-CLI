//! The Antigravity Agent Hooks receipt, written natively.
//!
//! Antigravity's official Stop-hook contract hands the hook one JSON document on
//! stdin and expects one JSON document on stdout. LicoUp registers one global
//! Stop hook whose only job is to record *which* native conversation the turn
//! just ran, so the client can resume exactly that conversation later.
//!
//! The hook used to be a generated `/bin/sh` script that piped its payload into
//! a Python interpreter. That placed a second program between the vendor client
//! and this package: a machine without that interpreter silently lost the
//! conversation identity, and its presence was never part of LicoUp's declared
//! runtime. This module is that hook, in the package's own program, with no
//! interpreter and no third program in the path.
//!
//! # What it writes
//!
//! One receipt file, named by the `LICO_ANTIGRAVITY_SESSION_RECEIPT` environment
//! variable the launching driver exports for the turn, containing
//! `{"conversationId":"<id>"}`. The file is created owner-only (`0600`) and is
//! replaced atomically through a sibling temporary file, so a reader never sees
//! a half-written receipt and a crash never leaves an unreadable one.
//!
//! # Writer-order safety
//!
//! A vendor-direct receipt can already exist on the same path when this hook
//! runs. The identity is therefore resolved in one order — the hook payload's
//! own id, then the vendor environment's id, then whatever the existing receipt
//! already recorded — and an empty result never erases a conversation a previous
//! writer bound in the same turn. The resolution is
//! [`crate::parser::parse_hook_receipt`]'s, on the text this hook received, so
//! the writer and the reader share one rule rather than two.

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

/// The environment variable a launching driver exports for one turn.
pub const RECEIPT_ENV: &str = "LICO_ANTIGRAVITY_SESSION_RECEIPT";

/// The vendor's own environment identifier, read as a compatibility fallback.
pub const VENDOR_CONVERSATION_ENV: &str = "ANTIGRAVITY_CONVERSATION_ID";

/// What one hook run did, for the program's own report.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HookOutcome {
    /// The identity was recovered and the receipt was written.
    Recorded,
    /// No identity this run resolved; the one a previous writer bound was kept
    /// (and rewritten owner-only) rather than replaced.
    KeptExisting,
    /// No identity was available and there was nothing to keep.
    NoIdentity,
}

impl HookOutcome {
    /// The stable name the hook reports on stdout.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Recorded => "recorded",
            Self::KeptExisting => "kept-existing",
            Self::NoIdentity => "no-identity",
        }
    }
}

/// Run the receipt hook for one hook payload.
///
/// The path arrives from the environment rather than an argument, because the
/// vendor client starts the hook with the environment the launching driver
/// exported. A payload the vendor sent on stdin is the hook's input; an absent
/// or unreadable stdin is an empty payload, not a failure — the vendor's Stop
/// hook still expects its response.
pub fn run(payload: &str) -> io::Result<HookOutcome> {
    let Some(path) = receipt_path() else {
        return Ok(HookOutcome::NoIdentity);
    };
    Ok(record(&path, payload)?)
}

/// Resolve the receipt path this turn's driver exported.
pub fn receipt_path() -> Option<PathBuf> {
    std::env::var_os(RECEIPT_ENV)
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
}

/// Record the conversation identity of one hook payload at `path`.
///
/// The identity is resolved by the parser's own rule over the payload, and the
/// vendor environment and the existing receipt are the two compatibility
/// fallbacks the driver has always applied. Nothing is ever written when no
/// identity resolves at all, so the receipt never carries an empty conversation
/// over one a previous writer bound.
pub fn record(path: &Path, payload: &str) -> io::Result<HookOutcome> {
    record_with_environment(path, payload, vendor_environment_identity().as_deref())
}

/// [`record`], with the vendor environment's identifier passed in.
///
/// The production entry reads the variable; this form is the same rule without
/// the process-global read, so a test can state each writer order exactly.
pub fn record_with_environment(
    path: &Path,
    payload: &str,
    vendor_environment_id: Option<&str>,
) -> io::Result<HookOutcome> {
    // The order is the writer order this host has always applied: this run's
    // payload first, the vendor environment second, and an identity a previous
    // writer already bound third. The third answer rewrites the receipt rather
    // than leaving it untouched, which is what narrows a receipt an older client
    // left group- or world-readable to owner-only.
    let (conversation_id, kept_existing) = match crate::parser::parse_hook_receipt(payload) {
        Some(recovered) => (recovered, false),
        None => match vendor_environment_id
            .map(str::trim)
            .filter(|value| crate::parser::valid_session_id(value))
            .map(str::to_owned)
        {
            Some(recovered) => (recovered, false),
            None => match existing_receipt_identity(path) {
                Some(recorded) => (recorded, true),
                None => {
                    return Ok(if path.is_file() {
                        HookOutcome::KeptExisting
                    } else {
                        HookOutcome::NoIdentity
                    });
                }
            },
        },
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    // The temporary file is a sibling, so the rename is within one filesystem
    // and therefore atomic. It is created owner-only before anything is written
    // into it, so the identity is never briefly world-readable.
    let temporary = path.with_extension("json.hook-tmp");
    write_private(&temporary, &json!({ "conversationId": conversation_id }))?;
    fs::rename(&temporary, path)?;
    Ok(if kept_existing {
        HookOutcome::KeptExisting
    } else {
        HookOutcome::Recorded
    })
}

/// The identity the vendor's own environment exports, when the payload had none.
fn vendor_environment_identity() -> Option<String> {
    std::env::var(VENDOR_CONVERSATION_ENV)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| crate::parser::valid_session_id(value))
}

/// The identity an existing receipt already recorded, when this run recovered
/// none of its own.
fn existing_receipt_identity(path: &Path) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    crate::parser::parse_hook_receipt(&text)
}

/// Write one receipt document owner-only, replacing any file at the path.
fn write_private(path: &Path, document: &Value) -> io::Result<()> {
    let encoded = serde_json::to_vec(document).map_err(io::Error::other)?;
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(&encoded)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // `mode` applies only when the file is created; an existing temporary
        // file keeps its own mode, so it is narrowed explicitly as well.
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// The hook program: read one payload from stdin, record the receipt, answer the
/// vendor with one JSON document.
///
/// The response is `{}` in every outcome, because the vendor's Stop hook reads
/// the response as a control document and this hook never asks the vendor to
/// change what it is doing. A run that could not write reports its reason on
/// stderr and still answers `{}` — an identity this package failed to record
/// must not fail the user's turn.
pub fn main() -> io::Result<()> {
    let mut payload = String::new();
    let _ = io::stdin().read_to_string(&mut payload);
    let outcome = match run(&payload) {
        Ok(outcome) => outcome,
        Err(error) => {
            eprintln!("lico-agent-antigravity hook: receipt not written: {error}");
            HookOutcome::NoIdentity
        }
    };
    let stdout = io::stdout();
    let mut writer = stdout.lock();
    serde_json::to_writer(&mut writer, &json!({ "receipt": outcome.as_str() }))?;
    writer.write_all(b"\n")?;
    writer.flush()
}
