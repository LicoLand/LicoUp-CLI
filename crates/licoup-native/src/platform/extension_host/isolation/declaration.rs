//! The declared envelope and its record.
//!
//! The manifest *requests* permissions; the composition *grants* an envelope
//! (this module never turns a request into a grant); the record then keeps the
//! granted envelope of one instance as soon as that process exists — before its
//! handshake, so a record that cannot be written stops the instance instead of
//! leaving an unrecorded process running — and every later change to it. The
//! record lives beside the install journal in the same managed root: it is the
//! boundary's own record, not a second copy of any effect account. Nothing here
//! decides whether an effect happened; that stays with the host's invocation
//! bindings and the workflow owners.
//!
//! Three record kinds, and what each one is evidence for:
//!
//! - **Grant**: the mode, roots, network grant and limits one instance runs
//!   under, plus the limits the operating system actually accepted and the
//!   confinement mechanism in effect. Written once the process exists.
//! - **Revocation**: a decision to stop granting *new* capability to a package
//!   or instance. It is not evidence that in-flight work stopped, and it never
//!   claims an already-performed effect was retracted.
//! - **Release**: written only after the carrier observed the process exit
//!   ([`ObservedExit`]), including whether a signal ended it, what
//!   [`ReleaseScope`] the release really covers, and whether some writer of its
//!   stdout was still alive afterwards. A restricted instance is one process, so
//!   its release covers the instance; a trusted local program may have
//!   descendants that left the group, and then the release covers only the
//!   supervised process and its group — the owner stays unverified rather than
//!   being cleared on a root wait alone.
//!
//! Complete lines must parse: a damaged middle line fails closed with
//! `extension_isolation_record_corrupt` rather than being skipped, because
//! skipping would silently forget a grant that is still in effect. A genuinely
//! unterminated tail — the one part a crash can leave unfinished — is
//! terminated with a newline before the next append (and when the record is
//! reopened), so the next record can never be concatenated onto it; the damaged
//! line itself stays as evidence and fails closed on read.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use licoup_application::ApplicationFailure;

use crate::platform::extension_packages::{
    append_journal_line, ensure_private_directory, now_unix_ms,
};

use super::super::{refusal, uncertain};
use super::capability::{IsolationMode, Support};
use super::limits::{EnforcedLimits, ResourceLimits};

/// The record directory inside a managed root.
const RECORD_DIRECTORY: &str = "isolation";
const RECORD_FILE: &str = "grants.jsonl";
/// One line of the record. The shared append helper refuses larger lines, and a
/// declaration that does not fit is refused rather than split across records.
const MAX_RECORD_LINE_BYTES: usize = 4096;
/// The whole record a release will read back.
const MAX_RECORD_BYTES: usize = 4 * 1024 * 1024;

/// What network access one instance is granted.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum NetworkGrant {
    /// No outbound or inbound network access.
    Denied,
    /// Outbound TCP to one loopback port only (for a local gateway or service).
    Loopback { port: u16 },
}

impl NetworkGrant {
    pub const fn id(self) -> &'static str {
        match self {
            Self::Denied => "denied",
            Self::Loopback { .. } => "loopback",
        }
    }
}

/// The boundary the resource limits really cover.
///
/// The POSIX limits are per process. Only where the instance is forced to be a
/// single process do they bound the whole instance.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum LimitScope {
    /// One enforced process == the instance, so the limits bound the instance.
    Instance,
    /// The limits are per process; descendants are not covered by them.
    Process,
}

impl LimitScope {
    pub const fn id(self) -> &'static str {
        match self {
            Self::Instance => "instance",
            Self::Process => "process",
        }
    }
}

/// The boundary a release really covers.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReleaseScope {
    /// The instance was one process that was observed to exit; nothing else
    /// could have existed.
    Instance,
    /// The supervised process and its group are gone, but a descendant that left
    /// the group cannot be verified. This is the weaker, default scope.
    #[default]
    ProcessGroup,
}

impl ReleaseScope {
    pub const fn id(self) -> &'static str {
        match self {
            Self::Instance => "instance",
            Self::ProcessGroup => "process-group",
        }
    }
}

/// Why new capability was withdrawn.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RevocationReason {
    /// The user disabled the package.
    UserWithdrawn,
    /// The package was uninstalled.
    PackageUninstalled,
    /// The authority that granted the envelope was revoked.
    AuthorityRevoked,
    /// The host faulted the instance's transport.
    FaultedTransport,
    /// The grant's own bounds were exceeded.
    BoundsExceeded,
}

/// The observed end of one process. `code` and `signal` are mutually exclusive
/// reports from the operating system, and neither is invented.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservedExit {
    pub success: bool,
    pub code: Option<i32>,
    pub signal: Option<i32>,
}

impl ObservedExit {
    pub const fn success() -> Self {
        Self {
            success: true,
            code: Some(0),
            signal: None,
        }
    }

    /// Build from the platform's own report.
    #[cfg(unix)]
    pub fn from_status(status: std::process::ExitStatus) -> Self {
        use std::os::unix::process::ExitStatusExt;
        Self {
            success: status.success(),
            code: status.code(),
            signal: status.signal(),
        }
    }

    #[cfg(not(unix))]
    pub fn from_status(status: std::process::ExitStatus) -> Self {
        Self {
            success: status.success(),
            code: status.code(),
            signal: None,
        }
    }
}

/// The envelope one instance was granted, as the record keeps it.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceDeclaration {
    pub package_id: String,
    pub package_version: String,
    pub instance_id: String,
    pub generation: u64,
    pub mode: IsolationMode,
    /// The confinement mechanism in effect, or why there is none.
    pub confinement: Support,
    /// Canonical paths, recorded as the local envelope they are. They are local
    /// runtime data and are never published through a catalogue document.
    pub read_roots: Vec<String>,
    pub write_root: String,
    pub executable: String,
    pub exec_paths: Vec<String>,
    pub network: NetworkGrant,
    /// What the composition asked for.
    pub limits: ResourceLimits,
    /// What the operating system actually accepted and enforces.
    pub enforced: EnforcedLimits,
    /// What those limits really cover: the instance, or only each process.
    pub limit_scope: LimitScope,
    pub declared_at_unix_ms: i64,
}

impl ResourceDeclaration {
    pub fn read_root_count(&self) -> usize {
        self.read_roots.len()
    }
}

/// One line of the isolation record.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "record", rename_all = "kebab-case")]
pub enum IsolationRecord {
    /// The envelope one instance runs under.
    Grant {
        #[serde(flatten)]
        declaration: Box<ResourceDeclaration>,
    },
    /// New capability withdrawn. In-flight effects are explicitly not implied.
    Revocation {
        package_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        instance_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        generation: Option<u64>,
        reason: RevocationReason,
        revoked_at_unix_ms: i64,
    },
    /// The process was observed to exit. Only this record is release evidence.
    Release {
        package_id: String,
        instance_id: String,
        generation: u64,
        exit: ObservedExit,
        /// Whether the released process itself was ended by a signal, meaning
        /// teardown had to terminate it; a process that exited on its own is
        /// reported with its own status even when descendants were reclaimed.
        forced: bool,
        /// Whether the instance's stdout was still held after teardown, which
        /// means a descendant left the process group and was not reclaimed.
        /// The process group was signalled; this is the honest residue. `false`
        /// is *not* proof that nothing survived: a descendant may close stdio
        /// and keep running.
        #[serde(default)]
        stdout_pipe_held: bool,
        /// What this release covers ([`ReleaseScope`]).
        #[serde(default)]
        release_scope: ReleaseScope,
        released_at_unix_ms: i64,
    },
}

impl IsolationRecord {
    pub fn instance_id(&self) -> Option<&str> {
        match self {
            Self::Grant { declaration } => Some(declaration.instance_id.as_str()),
            Self::Revocation { instance_id, .. } => instance_id.as_deref(),
            Self::Release { instance_id, .. } => Some(instance_id.as_str()),
        }
    }
}

/// Terminate a genuinely unterminated tail with one newline.
///
/// Only the last byte is inspected, so this stays cheap enough to run before
/// every append. A record is evidence; a crash-damaged line is never deleted and
/// never merged with a later one — it becomes a complete, damaged line that
/// fails closed on read.
fn ensure_terminated_tail(path: &Path) -> Result<(), ApplicationFailure> {
    use std::io::{Read, Seek, SeekFrom, Write};

    let mut file = match std::fs::OpenOptions::new().read(true).open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        // A path that cannot even be read is reported by the append itself,
        // which owns the real storage error; repair never guesses.
        Err(_) => return Ok(()),
    };
    let length = file.metadata().map(|metadata| metadata.len()).unwrap_or(0);
    if length == 0 || file.seek(SeekFrom::End(-1)).is_err() {
        return Ok(());
    }
    let mut last = [0u8; 1];
    if file.read_exact(&mut last).is_err() || last[0] == b'\n' {
        return Ok(());
    }
    let mut append = std::fs::OpenOptions::new()
        .append(true)
        .open(path)
        .map_err(|_| record_unavailable())?;
    append
        .write_all(b"\n")
        .and_then(|()| append.sync_all())
        .map_err(|_| record_unavailable())
}

fn record_unavailable() -> ApplicationFailure {
    uncertain(
        "extension_isolation_record_unavailable",
        "extension/isolation",
    )
    .with_field("record")
}

/// The per-root isolation record.
///
/// One file per managed root, appended under the same private-directory and
/// fsynced-line rules as the install journal. It is not a lease: the exclusive
/// writer for a managed root is the host's catalogue journal, and this record
/// deliberately has no second writer claim to conflict with it.
pub struct IsolationLedger {
    root: PathBuf,
    path: PathBuf,
}

impl IsolationLedger {
    /// Open (creating when absent) the record under a managed root.
    pub fn open(managed_root: &Path) -> Result<Self, ApplicationFailure> {
        let metadata = std::fs::metadata(managed_root).map_err(|_| {
            refusal(
                "extension_isolation_root_unavailable",
                "extension/isolation",
            )
            .with_field("managedRoot")
        })?;
        if !metadata.is_dir() {
            return Err(refusal(
                "extension_isolation_root_unavailable",
                "extension/isolation",
            )
            .with_field("managedRoot"));
        }
        let directory = managed_root.join(RECORD_DIRECTORY);
        ensure_private_directory(&directory)?;
        let path = directory.join(RECORD_FILE);
        // A writer can have died between a record's bytes and its newline. That
        // tail is not evidence; terminate it before anything else is appended.
        ensure_terminated_tail(&path)?;
        Ok(Self {
            root: managed_root.to_path_buf(),
            path,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Record that one instance was granted an envelope.
    pub fn record_grant(
        &self,
        declaration: &ResourceDeclaration,
    ) -> Result<(), ApplicationFailure> {
        self.append(&IsolationRecord::Grant {
            declaration: Box::new(declaration.clone()),
        })
    }

    /// Record that new capability was withdrawn.
    pub fn record_revocation(
        &self,
        package_id: &str,
        instance_id: Option<&str>,
        generation: Option<u64>,
        reason: RevocationReason,
    ) -> Result<(), ApplicationFailure> {
        self.append(&IsolationRecord::Revocation {
            package_id: package_id.to_owned(),
            instance_id: instance_id.map(str::to_owned),
            generation,
            reason,
            revoked_at_unix_ms: now_unix_ms(),
        })
    }

    /// Record an observed release.
    ///
    /// `stdout_pipe_held` must say whether a writer of the instance's stdout was
    /// still alive after teardown; it is residue evidence, never a completeness
    /// proof. `release_scope` says what the release really covers.
    pub fn record_release(
        &self,
        declaration: &ResourceDeclaration,
        exit: ObservedExit,
        forced: bool,
        stdout_pipe_held: bool,
        release_scope: ReleaseScope,
    ) -> Result<(), ApplicationFailure> {
        self.append(&IsolationRecord::Release {
            package_id: declaration.package_id.clone(),
            instance_id: declaration.instance_id.clone(),
            generation: declaration.generation,
            exit,
            forced,
            stdout_pipe_held,
            release_scope,
            released_at_unix_ms: now_unix_ms(),
        })
    }

    fn append(&self, record: &IsolationRecord) -> Result<(), ApplicationFailure> {
        // Never concatenate a new record onto an unterminated tail: the record
        // that is being written now must survive even after a previous crash.
        ensure_terminated_tail(&self.path)?;
        let line = serde_json::to_string(record).map_err(|_| {
            refusal("extension_isolation_record_invalid", "extension/isolation")
                .with_field("record")
        })?;
        if line.len() > MAX_RECORD_LINE_BYTES {
            return Err(refusal(
                "extension_isolation_record_too_large",
                "extension/isolation",
            )
            .with_field("record"));
        }
        append_journal_line(&self.path, &line).map_err(|failure| {
            // The shared appender reports package-scoped storage codes; this
            // record keeps its own boundary visible and names what failed underneath.
            uncertain(
                "extension_isolation_record_unavailable",
                "extension/isolation",
            )
            .with_field("record")
            .with_presentation_arg("storageFailure", &failure.code)
        })
    }

    /// Read the record back.
    ///
    /// Complete lines must parse; a damaged line after the last newline is the
    /// one part a crash can leave unfinished and is ignored, like the install
    /// journal's own tail rule.
    pub fn read(&self) -> Result<Vec<IsolationRecord>, ApplicationFailure> {
        let bytes = match std::fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(_) => {
                return Err(uncertain(
                    "extension_isolation_record_unavailable",
                    "extension/isolation",
                )
                .with_field("record"));
            }
        };
        if bytes.len() > MAX_RECORD_BYTES {
            return Err(refusal(
                "extension_isolation_record_too_large",
                "extension/isolation",
            )
            .with_field("record"));
        }
        let text = String::from_utf8(bytes).map_err(|_| {
            refusal("extension_isolation_record_corrupt", "extension/isolation")
                .with_field("record")
        })?;
        let mut records = Vec::new();
        let mut lines = text.split('\n').peekable();
        while let Some(line) = lines.next() {
            let is_last = lines.peek().is_none();
            if line.is_empty() {
                continue;
            }
            if is_last && !text.ends_with('\n') {
                // The unterminated tail is never evidence.
                break;
            }
            let record: IsolationRecord = serde_json::from_str(line).map_err(|_| {
                refusal("extension_isolation_record_corrupt", "extension/isolation")
                    .with_field("record")
            })?;
            records.push(record);
        }
        Ok(records)
    }

    /// The declarations that were granted, keyed by instance id.
    pub fn grants(&self) -> Result<BTreeMap<String, ResourceDeclaration>, ApplicationFailure> {
        let mut grants = BTreeMap::new();
        for record in self.read()? {
            if let IsolationRecord::Grant { declaration } = record {
                grants.insert(declaration.instance_id.clone(), *declaration);
            }
        }
        Ok(grants)
    }
}
