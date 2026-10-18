//! One native local-host maintenance admission decision.
//!
//! Installed client replacement, package activation and data conversion all
//! change state that running local work depends on. This module answers the one
//! question they must ask first — *does this host still own unfinished work?* —
//! and owns the close-admission barrier that keeps maintenance admission closed
//! from the decision through the switch.
//!
//! # Scope
//!
//! The decision is **local-host scoped**. It reads the canonical records this
//! host itself owns:
//!
//! * `licoup_conversation::store::read_unfinished_local_work` — unfinalized
//!   Events, active Direct Turns, unsettled dispatches and Subagent claims
//!   across every Conversation. That single read carries the same predicate the
//!   per-conversation destructive-write refusal uses, so both readers agree on
//!   what "in flight" means.
//! * `crate::domain::workflow_store::read_unfinished_local_work` — unfinished
//!   Adaptive Flywheel runs, queued or claimed delivery items, unsettled
//!   invocations, graphs negotiating a pause and deliberately held graphs.
//!
//! Both entries read a private database directly with a read-only connection.
//! A decision never opens, initializes, migrates or cold-recovers a store, so
//! asking "may this host change installed state?" cannot itself settle the work
//! that question is about.
//!
//! The named unfinished states map onto those owners as: *queued* — a
//! Conversation dispatch in `accepted` or a `pending` queue item; *claimed* —
//! a Direct Turn in `pending`/`claimed` or a Subagent claim in `claimed`;
//! *paused* — an active pause request; *approval-waiting* — a Direct Turn in
//! `waiting-for-human`; *stopping* — a dispatch or claim in `cancel-requested`;
//! *disconnected-but-unsettled* — a claim in `reconciliation-required`, an
//! unfinalized Event, a `claimed` queue item whose claimant is gone, or an
//! unsettled invocation.
//!
//! # Remote-only work and pending protocol replies
//!
//! There is no `local|remote|origin` column in the canonical store, and the
//! only candidate, `conversation_dispatches.native_provenance`, is **not** an
//! origin marker: it records the provider-side identity (`agentId`,
//! `nativeSessionId`, turn and message ids) bound to a dispatch this host
//! executed, and a locally running Codex or ACP dispatch carries it. Reading it
//! as "remote" would exempt exactly the dispatches executing here right now, so
//! no `native_provenance`-based exemption is implemented.
//!
//! The implemented rule is *no record, no blocker*: remote-only work executed
//! by another peer has no row in this host's canonical store, and pending
//! protocol records (`subagent_dispatch_deliveries`,
//! `subagent_mcp_inbound`, `workflow_subscriptions`) are delivery and
//! idempotency state rather than locally owned tasks. An unreachable peer
//! therefore cannot block this host merely by being silent. The limitation runs
//! the other way: if a future path writes a non-terminal local-work row purely
//! to describe an execution owned elsewhere, this decision reports it as local
//! work until a real origin column exists. That is the fail-closed direction —
//! a wrong "busy" is recoverable, a wrong "idle" is not.
//!
//! # The close-admission barrier
//!
//! [`WorkAdmission::begin_maintenance`] reads the decision *and*, when the host
//! is idle, takes the barrier in the same call, so no caller can observe an
//! idle decision and then lose the race to a newly admitted task. While the
//! barrier is held the decision is [`AdmissionDecision::Closed`], a second
//! maintenance caller is refused, and [`WorkAdmission::admission`] never
//! reports idle. The switch's owner releases it explicitly on success or abort,
//! and release is idempotent, so a failure path cannot leave admission closed
//! by accident.
//!
//! The barrier is durable: it is a record of the existing `native.update-handoff`
//! machine (`pending` → `claimed`, see `resources/state-machines/update-handoff.json`)
//! written atomically with an fsync to
//! `<data-root>/client-state/maintenance-admission.json`. It reuses the machine
//! rather than defining a second one; it is a separate *record* from the
//! client-update handoff (`client-state/migrations/update-handoff.json`)
//! because that record binds one signed release to a target path and migration
//! frontier while this one states that new work admission is closed host-wide.
//! A restart re-reads the claimed record and keeps admission closed until the
//! switch's owner or its recovery releases it; `claimed` is terminal in the
//! machine, so release retires the record instead of inventing another state,
//! and the next switch starts from a fresh `pending`.
//!
//! # What the barrier enforces, and what is still wired around it
//!
//! Held means two things today: no second maintenance operation may start, and
//! the decision stays [`AdmissionDecision::Closed`] until an explicit release,
//! across a restart. It does **not** yet refuse work *arrival*. The paths that
//! admit new local work — Conversation dispatch and turn creation, Subagent
//! claims, Adaptive Flywheel queue admission — do not read the barrier, so work
//! admitted in the window between the idle read and the barrier write starts
//! while a switch is being prepared. That window belongs to the work-arrival
//! owners: the barrier is a record in the host data root, and the canonical
//! Conversation store cannot reach into this crate to read it, so the check is
//! an answer the composition supplies to whoever admits work. Until it lands, a
//! switch must be started while the host is quiet, and the durable `claimed`
//! record is what keeps an interrupted switch visible instead of silently
//! reopening admission.
//!
//! # Wiring the gates (follow-up step)
//!
//! The gate calls are deliberately not wired here. They are:
//!
//! * `crates/licoup-native/src/domain/client_update/apply.rs:39` — before
//!   `native_runner::apply_live` performs the first write that replaces
//!   installed state:
//!   `WorkAdmission::open(portable_data_dir()?).begin_maintenance(MaintenanceOperation::ClientReplacement)?`
//!   and `WorkAdmission::open(data_root).release_admission()` on abort and on
//!   completion.
//! * `crates/licoup-native/src/platform/extension_packages/install.rs` — at the
//!   existing activation admission (`PackageStore::admit_activation_for_client`,
//!   `:606`) and before `PackageStore::install` publishes staged bytes
//!   (`:328`): `hold_package_activation_admission(data_root)?` and
//!   `release_maintenance_admission(data_root)?` on abort and on completion.
//!   These two layer-neutral entries exist because the store is bound to an
//!   explicit root (`PackageStore::open(root)`, constructed by
//!   `platform::extension_host::runtime`) while the barrier lives in the host
//!   data root: the composition that knows the root installs the answer, exactly
//!   as `install_subagent_claim_stop` installs the stop control's domain answer.
//! * Work arrival — the Conversation dispatch, turn, claim and Adaptive Flywheel
//!   queue entries — must refuse or defer while `WorkAdmission::barrier` reports
//!   a held record. This is the remaining half of "work arriving at the boundary
//!   is rejected or deferred"; it is not part of this decision owner.

mod barrier;
#[cfg(test)]
mod tests;

use anyhow::{Context, Result, anyhow, ensure};
use licoup_conversation::store::read_unfinished_local_work;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::domain::workflow_store::read_unfinished_local_work as read_unfinished_workflow_work;
use crate::state_machines::update_handoff;

pub use barrier::AdmissionBarrier;

/// Which installed-state change is asking for maintenance admission.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum MaintenanceOperation {
    /// Replacing the installed client with a downloaded release.
    ClientReplacement,
    /// Activating an installed extension package or converting its data.
    PackageActivation,
}

impl MaintenanceOperation {
    /// The stable wire name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ClientReplacement => "client-replacement",
            Self::PackageActivation => "package-activation",
        }
    }

    /// The operation named by its wire name.
    pub fn from_name(value: &str) -> Option<Self> {
        match value {
            "client-replacement" => Some(Self::ClientReplacement),
            "package-activation" => Some(Self::PackageActivation),
            _ => None,
        }
    }
}

/// Which local owner reports a blocker.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum LocalWorkOwner {
    /// The canonical Conversation authority.
    CanonicalConversation,
    /// The durable Adaptive Flywheel workflow store.
    AdaptiveFlywheel,
}

impl LocalWorkOwner {
    /// The stable wire name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CanonicalConversation => "canonical-conversation",
            Self::AdaptiveFlywheel => "adaptive-flywheel",
        }
    }
}

/// One unfinished local task that blocks maintenance.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdmissionBlocker {
    /// The owner that reported the record.
    pub owner: LocalWorkOwner,
    /// The owner's own kind name, for example `conversation-dispatch` or
    /// `workflow-pause-request`.
    pub kind: String,
    /// The conversation or graph the record belongs to; empty when the owner's
    /// kind is not scoped to one.
    pub scope: String,
    /// Stable identity inside its kind: event, turn, dispatch, claim, run,
    /// queue item or invocation id.
    pub identity: String,
    /// The stored state that makes the record a blocker.
    pub state: String,
}

/// The host-wide maintenance decision.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AdmissionDecision {
    /// No unfinished local work and no barrier: a maintenance switch may begin.
    Idle,
    /// This host still owns unfinished local work.
    Blocked,
    /// A maintenance switch holds the close-admission barrier.
    Closed,
}

impl AdmissionDecision {
    /// The stable wire name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Blocked => "blocked",
            Self::Closed => "closed",
        }
    }

    /// Whether a maintenance switch may begin now.
    pub const fn allows_maintenance(self) -> bool {
        matches!(self, Self::Idle)
    }
}

/// The decision plus the blockers behind it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Admission {
    pub decision: AdmissionDecision,
    pub blockers: Vec<AdmissionBlocker>,
    /// True when more blockers exist than the bounded reads reported.
    pub truncated: bool,
    /// The barrier that closed admission, when one is held.
    pub barrier: Option<AdmissionBarrier>,
}

/// The result of one close-admission attempt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MaintenanceAdmission {
    /// This caller now holds the barrier and must release it when the switch
    /// succeeds or aborts.
    Held(AdmissionBarrier),
    /// Another maintenance switch already closed admission.
    AlreadyClosed(AdmissionBarrier),
    /// Admission stays open: this host still owns unfinished work.
    Blocked {
        blockers: Vec<AdmissionBlocker>,
        truncated: bool,
    },
}

impl MaintenanceAdmission {
    /// The barrier this attempt observed, when one is held.
    pub fn barrier(&self) -> Option<&AdmissionBarrier> {
        match self {
            Self::Held(barrier) | Self::AlreadyClosed(barrier) => Some(barrier),
            Self::Blocked { .. } => None,
        }
    }
}

/// The local-host maintenance admission owner for one data root.
#[derive(Clone, Debug)]
pub struct WorkAdmission {
    data_root: PathBuf,
}

impl WorkAdmission {
    /// Bind the decision owner to the data root a maintenance switch would
    /// change.
    pub fn open(data_root: impl Into<PathBuf>) -> Self {
        Self {
            data_root: data_root.into(),
        }
    }

    /// The data root this owner decides about.
    pub fn data_root(&self) -> &Path {
        &self.data_root
    }

    /// The decision plus every blocker it read.
    ///
    /// Reading creates nothing: a data root without a store has no record, so
    /// it has no local work either.
    pub fn admission(&self) -> Result<Admission> {
        let barrier = self.barrier()?;
        let (blockers, truncated) = self.local_work()?;
        let decision = if barrier.is_some() {
            AdmissionDecision::Closed
        } else if blockers.is_empty() {
            AdmissionDecision::Idle
        } else {
            AdmissionDecision::Blocked
        };
        Ok(Admission {
            decision,
            blockers,
            truncated,
            barrier,
        })
    }

    /// Read the decision and, when this host is idle, hold the close-admission
    /// barrier in the same call.
    ///
    /// The barrier is durable before this call returns, so a caller that
    /// receives [`MaintenanceAdmission::Held`] owns the only maintenance claim
    /// on this data root: no other caller can act on an idle decision, and an
    /// interrupted switch stays visible to the next reader as `Closed`. Work
    /// *arrival* does not consult the barrier yet — see the module note on what
    /// the barrier enforces and what is still wired around it.
    pub fn begin_maintenance(
        &self,
        operation: MaintenanceOperation,
    ) -> Result<MaintenanceAdmission> {
        if let Some(barrier) = self.barrier()? {
            return Ok(MaintenanceAdmission::AlreadyClosed(barrier));
        }
        let (blockers, truncated) = self.local_work()?;
        if !blockers.is_empty() {
            return Ok(MaintenanceAdmission::Blocked {
                blockers,
                truncated,
            });
        }
        let barrier = self.claim_barrier(operation)?;
        Ok(MaintenanceAdmission::Held(barrier))
    }

    /// Release the barrier explicitly after the switch succeeded or aborted.
    ///
    /// Releasing when no barrier is held is not an error: the holder may have
    /// already retired it, and an explicit release must stay idempotent so a
    /// failure path cannot leave admission closed by accident.
    pub fn release_admission(&self) -> Result<()> {
        if self.barrier()?.is_none() {
            return Ok(());
        }
        barrier::retire(&self.data_root).context("maintenance_admission_release_failed")
    }

    /// The barrier currently held, if any.
    pub fn barrier(&self) -> Result<Option<AdmissionBarrier>> {
        barrier::read(&self.data_root).context("maintenance_admission_record_invalid")
    }

    fn claim_barrier(&self, operation: MaintenanceOperation) -> Result<AdmissionBarrier> {
        let claimed = update_handoff::transition(
            update_handoff::State::Pending,
            update_handoff::Event::Claim,
        )
        .ok_or_else(|| anyhow!("maintenance_admission_transition_invalid"))?;
        ensure!(
            claimed == update_handoff::State::Claimed,
            "maintenance_admission_transition_invalid"
        );
        let barrier = AdmissionBarrier {
            operation,
            state: claimed.as_str().to_owned(),
            claimed_at_unix_ms: now_unix_ms(),
        };
        barrier::write(&self.data_root, &barrier).context("maintenance_admission_write_failed")?;
        Ok(barrier)
    }

    /// Read every local owner without opening, initializing or recovering its
    /// store: a maintenance decision must not change what it decides about.
    fn local_work(&self) -> Result<(Vec<AdmissionBlocker>, bool)> {
        let mut blockers = Vec::new();
        let mut truncated = false;
        let conversations = read_unfinished_local_work(&self.data_root)
            .context("maintenance_admission_store_unavailable")?;
        truncated |= conversations.truncated;
        blockers.extend(
            conversations
                .blockers()
                .iter()
                .map(|blocker| AdmissionBlocker {
                    owner: LocalWorkOwner::CanonicalConversation,
                    kind: blocker.kind.as_str().to_owned(),
                    scope: blocker.conversation_id.clone(),
                    identity: blocker.identity.clone(),
                    state: blocker.state.clone(),
                }),
        );
        let workflows = read_unfinished_workflow_work(&self.data_root)
            .context("maintenance_admission_store_unavailable")?;
        truncated |= workflows.truncated;
        blockers.extend(workflows.blockers().iter().map(|blocker| AdmissionBlocker {
            owner: LocalWorkOwner::AdaptiveFlywheel,
            kind: blocker.kind.as_str().to_owned(),
            scope: blocker.graph_id.clone(),
            identity: blocker.identity.clone(),
            state: blocker.state.clone(),
        }));
        Ok((blockers, truncated))
    }
}

/// Layer-neutral entry for the package owner: hold the close-admission barrier
/// before package activation or data conversion changes installed state.
///
/// `Ok(())` means this caller owns the switch and must call
/// [`release_maintenance_admission`] on success or abort. A refusal code means
/// no installed state may change.
pub fn hold_package_activation_admission(data_root: &Path) -> Result<(), &'static str> {
    match WorkAdmission::open(data_root).begin_maintenance(MaintenanceOperation::PackageActivation)
    {
        Ok(MaintenanceAdmission::Held(_)) => Ok(()),
        Ok(MaintenanceAdmission::AlreadyClosed(_)) => Err("maintenance_admission_closed"),
        Ok(MaintenanceAdmission::Blocked { .. }) => Err("maintenance_admission_blocked"),
        Err(_) => Err("maintenance_admission_unavailable"),
    }
}

/// Layer-neutral entry for the package owner: release the barrier after the
/// switch succeeded or aborted.
pub fn release_maintenance_admission(data_root: &Path) -> Result<(), &'static str> {
    WorkAdmission::open(data_root)
        .release_admission()
        .map_err(|_| "maintenance_admission_release_failed")
}

fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or_default()
}
