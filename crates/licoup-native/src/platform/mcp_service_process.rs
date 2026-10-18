//! The optional MCP service process, bound to one installed package generation.
//!
//! The service is not a bundled neighbour of this executable. It is the payload
//! of the optional `org.licoland.feature.mcp` package, and this module owns the
//! binding between the package lifecycle and the process that actually runs:
//!
//! - **Resolution.** Which generation starts comes from the package store
//!   ([`crate::platform::extension_packages::select_generation`]): the highest
//!   installed version the user has switched on, with the executable entry its
//!   own manifest declares. Nothing here falls back to a file next to this
//!   binary, and an absent or switched-off package starts no process at all.
//! - **Consent.** The bytes that start are the bytes the operator approved. The
//!   install record's content digest is the approval; the entry file is measured
//!   when the generation is selected, and the program is handed that digest in
//!   [`APPROVED_DIGEST_ENV`] so the serving process can prove it is the same
//!   bytes. A digest that no longer measures the same is refused, not started.
//! - **Ownership.** Starting writes a lease that names the generation, the
//!   measured payload digest and the callers it serves. Stopping, crash
//!   recovery and removal are all answered from that lease, so a process this
//!   client started is never forgotten and a process it did not start is never
//!   claimed.
//! - **One process.** The previous generation is stopped and confirmed before
//!   the next one starts. A failed activation restores the previous lease and
//!   reports what it could and could not restart; a crash reconciles to exactly
//!   one live process on the next start.
//!
//! The state this module keeps lives beside the service's own discovery document
//! under `<data home>/client-state/subagent-mcp/`; the process itself stays owned
//! by the payload's `service start|stop|status` verbs, which are the only things
//! that can prove the service's writer lease drained.
//!
//! SEAM(PIPELINE-COMMANDS): the package command family owns install, enable,
//! disable and uninstall. It reaches this owner with one line per transition —
//! `mcp_service_process::activate(&consent)` after an enable/activate,
//! `mcp_service_process::retire()` *before* an uninstall removes the installed
//! bytes, `mcp_service_process::stop_for_data_home_transition()` where a
//! transition already calls it. No FFI route is added here.

use anyhow::{Result, anyhow};
use licoup_application::ApplicationFailure;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    env,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use crate::platform::extension_packages::{
    GenerationSelection, InstalledGeneration, PackageStore, ensure_private_directory, now_unix_ms,
    read_bounded_text, refusal, replace_file_atomically, select_generation,
};

/// The optional package that carries the MCP service.
pub const MCP_PACKAGE_ID: &str = "org.licoland.feature.mcp";

/// The digest the launcher approved, handed to the program it starts.
///
/// The serving process measures its own executable and refuses to serve bytes
/// that do not match, so an approval that stopped describing the file on disk
/// cannot silently become a running service.
pub const APPROVED_DIGEST_ENV: &str = "LICOUP_MCP_APPROVED_DIGEST";

/// The stage every refusal from this module reports.
const PROCESS_STAGE: &str = "mcp/package-lifecycle";

/// The lease this module writes for the process it started.
const LEASE_FILE: &str = "service-generation.json";
const LEASE_SCHEMA: &str = "licoup.subagent-mcp.generation.v1";
const MAX_LEASE_BYTES: usize = 16 * 1024;
/// The record one failed activation leaves behind, so the rollback is
/// attributable after the fact rather than only in a log line.
const FAILURE_FILE: &str = "activation-failure.json";
const FAILURE_SCHEMA: &str = "licoup.subagent-mcp.activation-failure.v1";

/// What one probe of the generation's own program said.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessState {
    /// The program answered that the service is running.
    Running,
    /// The program answered that the service is not running. A crashed service
    /// leaves exactly this answer.
    Stopped,
    /// The program could not be asked: there is no claim either way.
    Unknown,
}

impl ProcessState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Stopped => "stopped",
            Self::Unknown => "unknown",
        }
    }
}

/// The digest an operator approved, as the decision that admits activation.
///
/// It is not constructible from nothing: an empty or oversized value is refused
/// here, and the value is compared with the install record's own digest before
/// anything runs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PackageConsent {
    digest: String,
}

impl PackageConsent {
    pub fn new(digest: impl Into<String>) -> Result<Self, ApplicationFailure> {
        let digest = digest.into();
        if digest.is_empty() || digest.len() > 128 {
            return Err(refusal("mcp_package_consent_invalid", PROCESS_STAGE).with_field("digest"));
        }
        Ok(Self { digest })
    }

    pub fn digest(&self) -> &str {
        &self.digest
    }
}

/// One running generation: the registrations the process it names serves.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceLease {
    pub schema_version: String,
    pub package_id: String,
    pub package_version: String,
    /// The four facts that name this generation, as one token.
    pub generation_id: String,
    /// The digest the operator approved when this version was installed.
    pub approved_digest: String,
    /// The digest of the entry file measured when it was started.
    pub payload_digest: String,
    /// The manifest's own entry, relative to the installed version.
    pub entry: String,
    /// Where that entry was when it was started. An absolute path: a lease that
    /// could only be read by re-resolving the store would lose the process it
    /// describes as soon as the store moved.
    pub entry_path: PathBuf,
    /// The callers this generation registered.
    pub callers: Vec<String>,
    pub activated_at_unix_ms: i64,
    /// Set when a stop could not be confirmed. The process stays recorded rather
    /// than reported stopped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_unconfirmed_at_unix_ms: Option<i64>,
}

impl ServiceLease {
    fn callers(&self) -> Vec<String> {
        self.callers.clone()
    }

    fn status_value(&self) -> Value {
        json!({
            "generationId": self.generation_id,
            "packageVersion": self.package_version,
            "entry": self.entry,
            "callers": self.callers,
        })
    }
}

/// What one activation could put back after a failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Restored {
    /// There was no previous generation to restore.
    None,
    /// The previous generation is serving again.
    Running,
    /// The previous generation was not restored; its lease is intact and the
    /// process state is not claimed.
    Unavailable,
}

impl Restored {
    const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Running => "running",
            Self::Unavailable => "unavailable",
        }
    }
}

/// The package lifecycle bound to the MCP service process.
///
/// `data_home` holds this client's own state; `store_root` is the managed root
/// the package store published. They are separate values because a data-home
/// transition moves the first while the second is read from it, and because a
/// test can point both at a synthetic root without touching either.
#[derive(Clone, Debug)]
pub struct McpServiceBinding {
    data_home: PathBuf,
    store_root: PathBuf,
}

impl McpServiceBinding {
    pub fn open(data_home: &Path, store_root: &Path) -> Self {
        Self {
            data_home: data_home.to_path_buf(),
            store_root: store_root.to_path_buf(),
        }
    }

    /// The binding over the running client's own data home.
    pub fn from_environment() -> Result<Self> {
        let data_home = licoup_foundation::platform::paths::portable_data_dir()?;
        let store_root = package_store_root(&data_home);
        Ok(Self::open(&data_home, &store_root))
    }

    pub fn data_home(&self) -> &Path {
        &self.data_home
    }

    pub fn store_root(&self) -> &Path {
        &self.store_root
    }

    fn state_root(&self) -> PathBuf {
        self.data_home.join("client-state").join("subagent-mcp")
    }

    fn lease_path(&self) -> PathBuf {
        self.state_root().join(LEASE_FILE)
    }

    fn failure_path(&self) -> PathBuf {
        self.state_root().join(FAILURE_FILE)
    }

    fn store(&self) -> Result<PackageStore, ApplicationFailure> {
        PackageStore::open(&self.store_root)
    }

    /// Which generation of the service package this client would start.
    pub fn selection(&self) -> Result<GenerationSelection, ApplicationFailure> {
        select_generation(&self.store()?, MCP_PACKAGE_ID)
    }

    /// The lease of the process this client started, when one exists.
    pub fn lease(&self) -> Result<Option<ServiceLease>, ApplicationFailure> {
        let Some(text) = read_bounded_text(&self.lease_path(), MAX_LEASE_BYTES)? else {
            return Ok(None);
        };
        serde_json::from_str(&text)
            .map(Some)
            .map_err(|_| refusal("mcp_service_lease_invalid", PROCESS_STAGE).with_field("lease"))
    }

    fn write_lease(&self, lease: &ServiceLease) -> Result<(), ApplicationFailure> {
        ensure_private_directory(&self.data_home.join("client-state"))?;
        ensure_private_directory(&self.state_root())?;
        let text = serde_json::to_string_pretty(lease)
            .map_err(|_| refusal("mcp_service_lease_invalid", PROCESS_STAGE).with_field("lease"))?;
        replace_file_atomically(&self.lease_path(), &text)
    }

    fn remove_lease(&self) -> Result<(), ApplicationFailure> {
        match std::fs::remove_file(self.lease_path()) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => {
                Err(refusal("mcp_service_state_unavailable", PROCESS_STAGE).with_field("lease"))
            }
        }
    }

    /// A failed activation, recorded where a surface can attribute it.
    fn record_failure(
        &self,
        code: &str,
        attempted: Option<&InstalledGeneration>,
        previous: Option<&ServiceLease>,
        restored: Restored,
    ) {
        let record = json!({
            "schemaVersion": FAILURE_SCHEMA,
            "code": code,
            "attemptedGenerationId": attempted.map(InstalledGeneration::generation_id),
            "previousGenerationId": previous.map(|lease| lease.generation_id.clone()),
            "previousCallers": previous.map(ServiceLease::callers).unwrap_or_default(),
            "restoredProcess": restored.as_str(),
            "failedAtUnixMs": now_unix_ms(),
        });
        let _ = ensure_private_directory(&self.data_home.join("client-state"));
        let _ = ensure_private_directory(&self.state_root());
        let _ = replace_file_atomically(&self.failure_path(), &record.to_string());
    }

    fn clear_failure(&self) {
        let _ = std::fs::remove_file(self.failure_path());
    }

    /// The failure record of the last activation attempt, when one is present.
    pub fn failure_record(&self) -> Result<Option<Value>, ApplicationFailure> {
        let Some(text) = read_bounded_text(&self.failure_path(), MAX_LEASE_BYTES)? else {
            return Ok(None);
        };
        serde_json::from_str(&text)
            .map(Some)
            .map_err(|_| refusal("mcp_service_lease_invalid", PROCESS_STAGE).with_field("failure"))
    }

    /// The callers this client registers with the service.
    ///
    /// The set is the adapter registry's own caller set — the same set the
    /// service publishes per-caller tokens for — so a registration can never
    /// name a caller the mesh does not admit.
    pub fn admitted_callers(&self) -> Vec<String> {
        let mut callers = crate::platform::runtime_adapters::production_subagent_registry(
            crate::agent_target_port(),
        )
        .caller_providers()
        .map(|provider| provider.as_str().to_owned())
        .collect::<Vec<_>>();
        callers.sort();
        callers.dedup();
        callers
    }

    /// Whether the generation's own program says the service is running.
    pub fn probe(&self, entry: &Path) -> ProcessState {
        match run_service(entry, "status", &self.data_home, None, false) {
            Ok(value) if value.get("state").and_then(Value::as_str) == Some("running") => {
                ProcessState::Running
            }
            Ok(_) => ProcessState::Stopped,
            // The program could not be asked at all. No claim is made in either
            // direction, which is what keeps a missing payload from reading as a
            // stopped service.
            Err(_) => ProcessState::Unknown,
        }
    }

    fn control(
        &self,
        entry: &Path,
        action: &str,
        approved: Option<&str>,
        transition: bool,
    ) -> Result<Value> {
        run_service(entry, action, &self.data_home, approved, transition)
            .map_err(|_| anyhow!("mcp_service_unavailable"))
    }

    /// Start the selected generation, after settling whatever came before.
    pub fn start(&self) -> Result<Value> {
        let (reconciled, selected_running) = self.reconcile_inner()?;
        if selected_running {
            // The selected generation is serving: the last failure, if any, has
            // been superseded by this confirmed state.
            self.clear_failure();
            let mut report = reconciled;
            if let Some(object) = report.as_object_mut() {
                object.insert("action".to_owned(), json!("already-running"));
            }
            return Ok(report);
        }
        if reconciled.get("state").and_then(Value::as_str) == Some("unknown") {
            return Err(anyhow!("mcp_previous_generation_unconfirmed"));
        }
        let selection = self.selection()?;
        let Some(generation) = selection.generation() else {
            return Err(selection_error(&selection));
        };
        // The install decision *is* the approval: a version was installed under
        // a content-bound trust record, and the record's digest is that content.
        let consent = PackageConsent::new(generation.approved_digest().to_owned())
            .map_err(|_| anyhow!("mcp_package_consent_invalid"))?;
        self.activate_with(generation, &consent)
    }

    /// Start the selected generation under a digest the caller already approved.
    ///
    /// This is the route a package command uses after the user confirms a
    /// version: the digest travels with the decision, and activation refuses
    /// when the installed bytes are not the ones the decision was about.
    pub fn activate(&self, consent: &PackageConsent) -> Result<Value> {
        let selection = self.selection()?;
        let Some(generation) = selection.generation() else {
            return Err(selection_error(&selection));
        };
        self.activate_with(generation, consent)
    }

    fn activate_with(
        &self,
        generation: &InstalledGeneration,
        consent: &PackageConsent,
    ) -> Result<Value> {
        if consent.digest() != generation.approved_digest() {
            self.record_failure(
                "mcp_package_consent_mismatch",
                Some(generation),
                None,
                Restored::None,
            );
            return Err(anyhow!("mcp_package_consent_mismatch"));
        }
        let previous = self.lease()?;
        if let Some(lease) = &previous
            && lease.generation_id == generation.generation_id()
        {
            if lease.payload_digest != generation.payload_digest() {
                self.record_failure(
                    "mcp_payload_changed_since_approval",
                    Some(generation),
                    Some(lease),
                    Restored::None,
                );
                return Err(anyhow!("mcp_payload_changed_since_approval"));
            }
            if self.probe(&lease.entry_path) == ProcessState::Running {
                return Ok(self.report(
                    generation,
                    ProcessState::Running,
                    "already-running",
                    lease,
                ));
            }
            // Ours, and gone: settle it before another process takes its place.
            self.remove_lease()?;
        }
        // One service, one process: the previous generation is stopped and
        // confirmed before the selected one starts.
        if let Some(lease) = &previous {
            match self.probe(&lease.entry_path) {
                ProcessState::Running => {
                    if self
                        .control(&lease.entry_path, "stop", None, false)
                        .is_err()
                    {
                        self.record_failure(
                            "mcp_previous_generation_still_running",
                            Some(generation),
                            Some(lease),
                            Restored::Running,
                        );
                        return Err(anyhow!("mcp_previous_generation_still_running"));
                    }
                }
                ProcessState::Stopped => {}
                ProcessState::Unknown => {
                    self.record_failure(
                        "mcp_previous_generation_unconfirmed",
                        Some(generation),
                        Some(lease),
                        Restored::None,
                    );
                    return Err(anyhow!("mcp_previous_generation_unconfirmed"));
                }
            }
        }
        if self
            .control(
                generation.entry_path(),
                "start",
                Some(generation.payload_digest()),
                false,
            )
            .is_err()
        {
            let restored = self.restore_previous(previous.as_ref());
            self.record_failure(
                "mcp_activation_failed",
                Some(generation),
                previous.as_ref(),
                restored,
            );
            return Err(anyhow!("mcp_activation_failed"));
        }
        if self.probe(generation.entry_path()) != ProcessState::Running {
            let restored = self.restore_previous(previous.as_ref());
            self.record_failure(
                "mcp_activation_unconfirmed",
                Some(generation),
                previous.as_ref(),
                restored,
            );
            return Err(anyhow!("mcp_activation_unconfirmed"));
        }
        let lease = ServiceLease {
            schema_version: LEASE_SCHEMA.to_owned(),
            package_id: generation.package_id().to_owned(),
            package_version: generation.version().to_owned(),
            generation_id: generation.generation_id(),
            approved_digest: generation.approved_digest().to_owned(),
            payload_digest: generation.payload_digest().to_owned(),
            entry: generation.entry().to_owned(),
            entry_path: generation.entry_path().to_path_buf(),
            callers: self.admitted_callers(),
            activated_at_unix_ms: now_unix_ms(),
            stop_unconfirmed_at_unix_ms: None,
        };
        self.write_lease(&lease)?;
        self.clear_failure();
        Ok(self.report(generation, ProcessState::Running, "started", &lease))
    }

    /// Put the previous generation back where it was.
    ///
    /// The lease was never replaced, so the previous registrations are intact by
    /// construction; what may need restoring is the process, which was stopped to
    /// make room for the attempt.
    fn restore_previous(&self, previous: Option<&ServiceLease>) -> Restored {
        let Some(lease) = previous else {
            return Restored::None;
        };
        if !lease.entry_path.exists() {
            return Restored::Unavailable;
        }
        if self
            .control(
                &lease.entry_path,
                "start",
                Some(lease.payload_digest.as_str()),
                false,
            )
            .is_err()
        {
            return Restored::Unavailable;
        }
        if self.probe(&lease.entry_path) == ProcessState::Running {
            Restored::Running
        } else {
            Restored::Unavailable
        }
    }

    /// Stop the process this client started.
    ///
    /// A stop that cannot be delivered is an error and leaves the lease in
    /// place: reporting a service stopped that was never reached is how a
    /// process escapes its owner.
    pub fn stop(&self) -> Result<Value> {
        self.stop_with(false)
    }

    fn stop_with(&self, data_home_transition: bool) -> Result<Value> {
        if let Some(lease) = self.lease()? {
            self.control(&lease.entry_path, "stop", None, data_home_transition)
                .map_err(|_| anyhow!("mcp_service_stop_unavailable"))?;
            self.remove_lease()?;
            return Ok(json!({
                "service": "subagents",
                "state": "stopped",
                "action": "stopped",
                "generationId": lease.generation_id,
                "callers": lease.callers,
            }));
        }
        // No lease: this client did not start a process. The installed generation
        // may still be serving one — started by an earlier client or by the
        // developer directly — and an explicit stop stops it rather than
        // reporting a state nobody checked. Nothing is started here.
        let selection = self.selection()?;
        if let Some(generation) = selection.generation()
            && self.probe(generation.entry_path()) == ProcessState::Running
        {
            self.control(generation.entry_path(), "stop", None, data_home_transition)
                .map_err(|_| anyhow!("mcp_service_stop_unavailable"))?;
            return Ok(json!({
                "service": "subagents",
                "state": "stopped",
                "action": "stopped-unleased-generation",
                "generationId": generation.generation_id(),
                "callers": [],
            }));
        }
        Ok(json!({
            "service": "subagents",
            "state": "stopped",
            "action": "nothing-owned",
            "callers": [],
        }))
    }

    /// Stop the service while the data-home transition holds its barrier.
    pub fn stop_for_transition(&self) -> Result<Value> {
        self.stop_with(true)
    }

    /// Remove the package: stop the process, then withdraw every caller entry.
    ///
    /// This runs *before* the installed bytes are removed, because the program
    /// that owns the service's writer lease is the only thing that can prove the
    /// drain. When it cannot be reached the lease is kept and marked
    /// unconfirmed — the removal is reported, never assumed.
    pub fn retire(&self) -> Result<Value> {
        let Some(lease) = self.lease()? else {
            return Ok(json!({
                "service": "subagents",
                "state": "stopped",
                "action": "nothing-owned",
                "callers": [],
            }));
        };
        match self.probe(&lease.entry_path) {
            ProcessState::Running => {
                self.control(&lease.entry_path, "stop", None, false)
                    .map_err(|_| anyhow!("mcp_service_stop_unavailable"))?;
            }
            ProcessState::Stopped => {}
            ProcessState::Unknown => {
                let mut unconfirmed = lease.clone();
                unconfirmed.stop_unconfirmed_at_unix_ms = Some(now_unix_ms());
                self.write_lease(&unconfirmed)?;
                return Ok(json!({
                    "service": "subagents",
                    "state": "unconfirmed",
                    "action": "stop-unconfirmed",
                    "generationId": lease.generation_id,
                    "callers": lease.callers,
                }));
            }
        }
        self.remove_lease()?;
        Ok(json!({
            "service": "subagents",
            "state": "stopped",
            "action": "retired",
            "generationId": lease.generation_id,
            "callers": lease.callers,
        }))
    }

    /// Reconcile this client's record with what is actually running.
    pub fn reconcile(&self) -> Result<Value> {
        self.reconcile_inner().map(|(value, _)| value)
    }

    fn reconcile_inner(&self) -> Result<(Value, bool)> {
        let selection = self.selection()?;
        let selected = selection.generation().cloned();
        let package = self.package_facts(&selection, None);
        let Some(lease) = self.lease()? else {
            return Ok((
                json!({
                    "service": "subagents",
                    "state": "stopped",
                    "action": "none",
                    "package": package,
                    "callers": [],
                }),
                false,
            ));
        };
        let selected_here = selected.as_ref().is_some_and(|generation| {
            generation.generation_id() == lease.generation_id
                && generation.payload_digest() == lease.payload_digest
        });
        match self.probe(&lease.entry_path) {
            ProcessState::Running if selected_here => Ok((
                json!({
                    "service": "subagents",
                    "state": "running",
                    "action": "none",
                    "package": self.package_facts(&selection, Some(&lease)),
                    "callers": lease.callers,
                }),
                true,
            )),
            ProcessState::Running => {
                // A generation is serving that is not the selected one: exactly
                // one process, and it is the wrong one. It is stopped, and the
                // disappearance of its registrations is reported.
                match self.control(&lease.entry_path, "stop", None, false) {
                    Ok(_) => {
                        self.remove_lease()?;
                        Ok((
                            json!({
                                "service": "subagents",
                                "state": "stopped",
                                "action": "stale-generation-stopped",
                                "generationId": lease.generation_id,
                                "callers": lease.callers,
                                "package": package,
                            }),
                            false,
                        ))
                    }
                    Err(_) => Ok((
                        json!({
                            "service": "subagents",
                            "state": "running",
                            "action": "stale-generation-stop-failed",
                            "generationId": lease.generation_id,
                            "callers": lease.callers,
                            "package": package,
                        }),
                        false,
                    )),
                }
            }
            ProcessState::Stopped => {
                // A crash is exactly this: our lease, and no process behind it.
                // The registrations it held are withdrawn with the record.
                self.remove_lease()?;
                Ok((
                    json!({
                        "service": "subagents",
                        "state": "stopped",
                        "action": "crashed-process-settled",
                        "generationId": lease.generation_id,
                        "callers": lease.callers,
                        "package": package,
                    }),
                    false,
                ))
            }
            ProcessState::Unknown => Ok((
                json!({
                    "service": "subagents",
                    "state": "unknown",
                    "action": "unconfirmed",
                    "generationId": lease.generation_id,
                    "callers": lease.callers,
                    "package": package,
                }),
                false,
            )),
        }
    }

    /// Whether one caller may reach the running generation.
    ///
    /// The answer is only "yes" while a lease names the caller *and* the process
    /// that would serve it answers. A refused caller is told which of the two
    /// facts is missing.
    pub fn caller_admission(&self, caller: &str) -> Result<CallerGrant, ApplicationFailure> {
        if caller.is_empty()
            || caller.len() > 64
            || !caller
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        {
            return Err(refusal("mcp_caller_invalid", PROCESS_STAGE).with_field("caller"));
        }
        let selection = self.selection()?;
        if selection.generation().is_none() {
            let code = match selection {
                GenerationSelection::Absent => "mcp_package_absent",
                GenerationSelection::Disabled { .. } => "mcp_package_disabled",
                GenerationSelection::Selected(_) => "mcp_package_selection_invalid",
            };
            return Err(refusal(code, PROCESS_STAGE).with_presentation_arg("caller", caller));
        }
        let Some(lease) = self.lease()? else {
            return Err(refusal("mcp_caller_unregistered", PROCESS_STAGE)
                .with_presentation_arg("caller", caller));
        };
        if !lease.callers.iter().any(|registered| registered == caller) {
            return Err(refusal("mcp_caller_unregistered", PROCESS_STAGE)
                .with_presentation_arg("caller", caller)
                .with_presentation_arg("generation", lease.generation_id.as_str()));
        }
        if self.probe(&lease.entry_path) != ProcessState::Running {
            return Err(refusal("mcp_service_unavailable", PROCESS_STAGE)
                .with_presentation_arg("caller", caller)
                .with_presentation_arg("generation", lease.generation_id.as_str()));
        }
        Ok(CallerGrant {
            caller: caller.to_owned(),
            generation_id: lease.generation_id.clone(),
            package_version: lease.package_version.clone(),
        })
    }

    /// The truthful state of the optional service, without starting anything.
    ///
    /// A package that is not installed at all has no service to report on, so
    /// its answer is the unavailable-service fact rather than an empty state;
    /// an installed generation — enabled or disabled — has real state to name.
    pub fn status(&self) -> Result<Value> {
        let selection = self.selection()?;
        if matches!(selection, GenerationSelection::Absent) {
            return Err(selection_error(&selection));
        }
        let lease = self.lease()?;
        let state = match &lease {
            Some(lease) => self.probe(&lease.entry_path),
            None => ProcessState::Stopped,
        };
        Ok(json!({
            "service": "subagents",
            "state": state.as_str(),
            "package": self.package_facts(&selection, lease.as_ref()),
            "callers": lease.as_ref().map(ServiceLease::callers).unwrap_or_default(),
            "active": lease.as_ref().map(ServiceLease::status_value),
        }))
    }

    fn package_facts(
        &self,
        selection: &GenerationSelection,
        lease: Option<&ServiceLease>,
    ) -> Value {
        json!({
            "id": MCP_PACKAGE_ID,
            "selection": selection.state(),
            "installedVersions": match selection {
                GenerationSelection::Disabled { installed_versions } => installed_versions.clone(),
                _ => Vec::new(),
            },
            "version": selection.generation().map(|generation| generation.version().to_owned()),
            "generationId": selection.generation().map(InstalledGeneration::generation_id),
            "approvedDigest": selection.generation().map(|generation| generation.approved_digest().to_owned()),
            "payloadDigest": selection.generation().map(|generation| generation.payload_digest().to_owned()),
            "activeGenerationId": lease.map(|lease| lease.generation_id.clone()),
            "stopUnconfirmed": lease
                .and_then(|lease| lease.stop_unconfirmed_at_unix_ms)
                .is_some(),
        })
    }

    fn report(
        &self,
        generation: &InstalledGeneration,
        state: ProcessState,
        action: &str,
        lease: &ServiceLease,
    ) -> Value {
        json!({
            "service": "subagents",
            "state": state.as_str(),
            "action": action,
            "package": {
                "id": MCP_PACKAGE_ID,
                "selection": "selected",
                "version": generation.version(),
                "generationId": generation.generation_id(),
                "approvedDigest": generation.approved_digest(),
                "payloadDigest": generation.payload_digest(),
                "activeGenerationId": lease.generation_id,
            },
            "callers": lease.callers,
        })
    }
}

/// Permission for one caller to reach the running generation.
///
/// Its fields are private: possessing one means [`McpServiceBinding::caller_admission`]
/// answered for that caller against the live generation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CallerGrant {
    caller: String,
    generation_id: String,
    package_version: String,
}

impl CallerGrant {
    pub fn caller(&self) -> &str {
        &self.caller
    }

    pub fn generation_id(&self) -> &str {
        &self.generation_id
    }

    pub fn package_version(&self) -> &str {
        &self.package_version
    }
}

fn selection_error(selection: &GenerationSelection) -> anyhow::Error {
    match selection {
        GenerationSelection::Absent => anyhow!("mcp_package_absent"),
        GenerationSelection::Disabled { .. } => anyhow!("mcp_package_disabled"),
        GenerationSelection::Selected(_) => anyhow!("mcp_package_selection_invalid"),
    }
}

/// The managed root of installed optional packages inside one data home.
///
/// SEAM(PIPELINE-COMMANDS): that node places the package store root inside the
/// selected data home layout and owns this path. Replacing this one function is
/// the whole change here; every read below goes through the store's own API, so
/// no second layout is invented.
pub fn package_store_root(data_home: &Path) -> PathBuf {
    data_home.join("extension-packages")
}

/// Run one service verb through the generation's own program.
///
/// The program owns its process, its writer lease and its discovery document;
/// this function only asks it to act and publishes the JSON it answers with.
/// `approved` is handed to the process so a start can prove the bytes it is
/// about to serve are the approved ones.
fn run_service(
    binary: &Path,
    action: &str,
    data_home: &Path,
    approved: Option<&str>,
    data_home_transition: bool,
) -> Result<Value> {
    let cli = env::current_exe().map_err(|_| anyhow!("mcp_cli_unavailable"))?;
    licoup_foundation::platform::file_security::ensure_private_dir(
        &data_home.join("client-state"),
    )?;
    licoup_foundation::platform::file_security::ensure_private_dir(
        &data_home.join("client-state").join("subagent-mcp"),
    )?;
    let mut command = Command::new(binary);
    command.args(["service", action]);
    if data_home_transition {
        command.arg("--data-home-transition");
    }
    command
        .env("LICOUP_CLI_BINARY", cli)
        .env("LICOUP_HOME", data_home);
    if let Some(digest) = approved {
        command.env(APPROVED_DIGEST_ENV, digest);
    }
    let output = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .map_err(|_| anyhow!("mcp_service_unavailable"))?;
    if !output.status.success() {
        return Err(anyhow!("mcp_service_unavailable"));
    }
    serde_json::from_slice(&output.stdout).map_err(|_| anyhow!("mcp_service_response_invalid"))
}

/// The service lifecycle verbs, bound to the installed and enabled package.
pub fn execute(action: &str, binary: Option<&Path>) -> Result<Value> {
    if !matches!(action, "start" | "stop" | "reload" | "status") {
        return Err(anyhow!("mcp_lifecycle_invalid"));
    }
    let binding = McpServiceBinding::from_environment()?;
    check_binary_claim(&binding, binary)?;
    match action {
        "start" => binding.start(),
        "stop" => binding.stop(),
        "reload" => {
            // One process at a time: the reload is a confirmed stop followed by a
            // fresh start, never a second start beside a running one.
            binding.stop()?;
            binding.start()
        }
        _ => binding.status(),
    }
}

/// Ask the independently owned MCP service to stop while the data-home
/// admission barrier is held. The service's stop-only control process must
/// run so the coordinator can prove that its existing writer lease drained.
pub fn stop_for_data_home_transition() -> Result<Value> {
    Ok(McpServiceBinding::from_environment()?.stop_for_transition()?)
}

/// A caller-supplied executable must be the generation the store selected.
///
/// An arbitrary path would be a process this client cannot attribute to any
/// installed generation, which is exactly the forgotten process the binding
/// exists to prevent. The option stays in the CLI contract; what changes is that
/// it can no longer name bytes the package lifecycle never published.
fn check_binary_claim(binding: &McpServiceBinding, binary: Option<&Path>) -> Result<()> {
    let Some(claimed) = binary else {
        return Ok(());
    };
    let selection = binding.selection()?;
    let Some(generation) = selection.generation() else {
        return Err(selection_error(&selection));
    };
    let claimed = std::fs::canonicalize(claimed)
        .map_err(|_| anyhow!("mcp_binary_not_selected_generation"))?;
    let selected = std::fs::canonicalize(generation.entry_path())
        .map_err(|_| anyhow!("mcp_binary_not_selected_generation"))?;
    if claimed != selected {
        return Err(anyhow!("mcp_binary_not_selected_generation"));
    }
    Ok(())
}
