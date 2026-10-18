//! The two state machines, kept apart.
//!
//! [`PackageMachine`] is about *bytes on disk and trust in them*: which version
//! of a package this client holds, how far the install got, and whether the user
//! has it switched on. [`InstanceMachine`] is about *one running thing*: a
//! package version may produce several instances with different permissions, and
//! an instance outlives neither its admission nor its in-flight work silently.
//!
//! The four facts that must never be collapsed into one "version" live in
//! [`InstanceIdentity`]: the package version, the instance id, the generation and
//! the registry epoch. A package update moves the generation and is committed as
//! a new registry epoch; it does not move an already admitted instance's release
//! identity, because that instance still has to be observed, cancelled and
//! settled exactly where it was admitted.

use crate::platform::extension_packages::install::ActivationAdmission;
use crate::platform::extension_packages::{actionable, now_unix_ms, refusal};
use crate::state_machines::{extension_instance, extension_package};
use licoup_application::ApplicationFailure;
use licoup_extension_contracts::deployment::{InstanceLifecycle, PackageFacts, PackageLifecycle};
use licoup_extension_contracts::manifest::PermissionRequest;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

const PACKAGE_STAGE: &str = "extension/package-state";
const INSTANCE_STAGE: &str = "extension/instance-state";

/// Whether one package-lifecycle step is legal.
///
/// A self-transition is legal: the journal may replay a step the host already
/// performed after a crash, and re-observing a state is not a state change.
pub fn package_transition_allowed(from: PackageLifecycle, to: PackageLifecycle) -> bool {
    package_next(from, to).is_some()
}

/// Whether one instance-lifecycle step is legal.
pub fn instance_transition_allowed(from: InstanceLifecycle, to: InstanceLifecycle) -> bool {
    instance_next(from, to).is_some()
}

fn package_next(from: PackageLifecycle, requested: PackageLifecycle) -> Option<PackageLifecycle> {
    extension_package::transition(package_state(from), package_event(requested))
        .map(package_lifecycle)
}

const fn package_state(state: PackageLifecycle) -> extension_package::State {
    match state {
        PackageLifecycle::Available => extension_package::State::Available,
        PackageLifecycle::Downloaded => extension_package::State::Downloaded,
        PackageLifecycle::Verified => extension_package::State::Verified,
        PackageLifecycle::LocalApproved => extension_package::State::LocalApproved,
        PackageLifecycle::Staged => extension_package::State::Staged,
        PackageLifecycle::Installed => extension_package::State::Installed,
    }
}

const fn package_event(state: PackageLifecycle) -> extension_package::Event {
    match state {
        PackageLifecycle::Available => extension_package::Event::ObserveAvailable,
        PackageLifecycle::Downloaded => extension_package::Event::ObserveDownloaded,
        PackageLifecycle::Verified => extension_package::Event::ObserveVerified,
        PackageLifecycle::LocalApproved => extension_package::Event::ObserveLocalApproved,
        PackageLifecycle::Staged => extension_package::Event::ObserveStaged,
        PackageLifecycle::Installed => extension_package::Event::ObserveInstalled,
    }
}

const fn package_lifecycle(state: extension_package::State) -> PackageLifecycle {
    match state {
        extension_package::State::Available => PackageLifecycle::Available,
        extension_package::State::Downloaded => PackageLifecycle::Downloaded,
        extension_package::State::Verified => PackageLifecycle::Verified,
        extension_package::State::LocalApproved => PackageLifecycle::LocalApproved,
        extension_package::State::Staged => PackageLifecycle::Staged,
        extension_package::State::Installed => PackageLifecycle::Installed,
    }
}

fn instance_next(
    from: InstanceLifecycle,
    requested: InstanceLifecycle,
) -> Option<InstanceLifecycle> {
    extension_instance::transition(instance_state(from), instance_event(requested))
        .map(instance_lifecycle)
}

const fn instance_state(state: InstanceLifecycle) -> extension_instance::State {
    match state {
        InstanceLifecycle::Discovered => extension_instance::State::Discovered,
        InstanceLifecycle::Preparing => extension_instance::State::Preparing,
        InstanceLifecycle::Active => extension_instance::State::Active,
        InstanceLifecycle::Draining => extension_instance::State::Draining,
        InstanceLifecycle::Stopped => extension_instance::State::Stopped,
        InstanceLifecycle::Failed => extension_instance::State::Failed,
        InstanceLifecycle::Quarantined => extension_instance::State::Quarantined,
    }
}

const fn instance_event(state: InstanceLifecycle) -> extension_instance::Event {
    match state {
        InstanceLifecycle::Discovered => extension_instance::Event::ObserveDiscovered,
        InstanceLifecycle::Preparing => extension_instance::Event::ObservePreparing,
        InstanceLifecycle::Active => extension_instance::Event::ObserveActive,
        InstanceLifecycle::Draining => extension_instance::Event::ObserveDraining,
        InstanceLifecycle::Stopped => extension_instance::Event::ObserveStopped,
        InstanceLifecycle::Failed => extension_instance::Event::ObserveFailed,
        InstanceLifecycle::Quarantined => extension_instance::Event::ObserveQuarantined,
    }
}

const fn instance_lifecycle(state: extension_instance::State) -> InstanceLifecycle {
    match state {
        extension_instance::State::Discovered => InstanceLifecycle::Discovered,
        extension_instance::State::Preparing => InstanceLifecycle::Preparing,
        extension_instance::State::Active => InstanceLifecycle::Active,
        extension_instance::State::Draining => InstanceLifecycle::Draining,
        extension_instance::State::Stopped => InstanceLifecycle::Stopped,
        extension_instance::State::Failed => InstanceLifecycle::Failed,
        extension_instance::State::Quarantined => InstanceLifecycle::Quarantined,
    }
}

/// One permission the user has approved, as the pair of a capability and the
/// scope it is approved for.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionKey {
    pub capability: String,
    pub scope: String,
}

impl PermissionKey {
    pub fn new(capability: impl Into<String>, scope: impl Into<String>) -> Self {
        Self {
            capability: capability.into(),
            scope: scope.into(),
        }
    }

    pub fn of(request: &PermissionRequest) -> Self {
        Self::new(request.capability.clone(), request.scope.clone())
    }
}

/// Why this client trusts a package, and the exact content that trust is bound
/// to.
///
/// The two channels are different facts, not two steps: `Verified` is a
/// publisher signature checked by a trusted update mechanism, `LocalApproved` is
/// a user approving bytes they supplied themselves. Neither is "more installed"
/// than the other, and neither survives different bytes: importing the same
/// package id with different content needs the content approved again.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TrustRecord {
    digest: String,
    channel: PackageLifecycle,
    scope: BTreeSet<PermissionKey>,
    recorded_at_unix_ms: i64,
}

impl TrustRecord {
    /// A user approved this exact content, for this exact permission set.
    ///
    /// Trust is bound to content: a record with no digest cannot be constructed,
    /// because "trusted, content unknown" is the fact this type exists to make
    /// unrepresentable.
    pub fn local_approved(
        digest: impl Into<String>,
        permissions: impl IntoIterator<Item = PermissionRequest>,
    ) -> Result<Self, ApplicationFailure> {
        Self::new(digest.into(), PackageLifecycle::LocalApproved, permissions)
    }

    /// A trusted update mechanism verified this exact content.
    pub fn publisher_verified(
        digest: impl Into<String>,
        permissions: impl IntoIterator<Item = PermissionRequest>,
    ) -> Result<Self, ApplicationFailure> {
        Self::new(digest.into(), PackageLifecycle::Verified, permissions)
    }

    fn new(
        digest: String,
        channel: PackageLifecycle,
        permissions: impl IntoIterator<Item = PermissionRequest>,
    ) -> Result<Self, ApplicationFailure> {
        if digest.is_empty() || digest.len() > 128 {
            return Err(actionable(
                "package_trust_not_bound_to_content",
                PACKAGE_STAGE,
                "digest",
            ));
        }
        Ok(Self {
            digest,
            channel,
            scope: permissions
                .into_iter()
                .map(|it| PermissionKey::of(&it))
                .collect(),
            recorded_at_unix_ms: now_unix_ms(),
        })
    }

    pub fn digest(&self) -> &str {
        &self.digest
    }

    pub fn channel(&self) -> PackageLifecycle {
        self.channel
    }

    pub fn recorded_at_unix_ms(&self) -> i64 {
        self.recorded_at_unix_ms
    }

    pub fn scope(&self) -> impl Iterator<Item = &PermissionKey> {
        self.scope.iter()
    }

    /// Refuse content this trust was not recorded for.
    pub fn check_content(&self, digest: &str) -> Result<(), ApplicationFailure> {
        if self.digest != digest {
            return Err(actionable(
                "package_trust_not_bound_to_content",
                PACKAGE_STAGE,
                "digest",
            ));
        }
        Ok(())
    }

    /// The permissions this trust does not cover.
    ///
    /// A package that grows its request after approval needs a new decision; a
    /// directory listing is not a permission grant, and neither is an older
    /// approval of a smaller scope.
    pub fn uncovered(&self, requested: &[PermissionRequest]) -> Vec<PermissionRequest> {
        requested
            .iter()
            .filter(|request| !self.scope.contains(&PermissionKey::of(request)))
            .cloned()
            .collect()
    }

    pub fn check_permissions(
        &self,
        requested: &[PermissionRequest],
    ) -> Result<(), ApplicationFailure> {
        let uncovered = self.uncovered(requested);
        if uncovered.is_empty() {
            return Ok(());
        }
        let mut failure = actionable(
            "package_permission_scope_expanded",
            PACKAGE_STAGE,
            "permissions",
        )
        .with_presentation_arg(
            "capability",
            uncovered
                .first()
                .map(|request| request.capability.as_str())
                .unwrap_or("unknown"),
        );
        if let Some(request) = uncovered.first() {
            failure = failure.with_presentation_arg("scope", request.scope.as_str());
        }
        Err(failure)
    }
}

/// Whether the package is switched on for new work.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InstallActivation {
    /// Installed and available for on-demand activation; nothing is started yet.
    EnabledOnDemand,
    /// Installed and switched off. The files stay on disk.
    Disabled,
}

/// One package version's install state on this machine.
#[derive(Clone, Debug)]
pub struct PackageMachine {
    package_id: String,
    version: String,
    listed: bool,
    state: PackageLifecycle,
    enabled: bool,
    digest: Option<String>,
    trust: Option<TrustRecord>,
    installed_at_unix_ms: Option<i64>,
}

impl PackageMachine {
    /// A package this client knows from directory metadata only.
    pub fn available(
        package_id: impl Into<String>,
        version: impl Into<String>,
    ) -> Result<Self, ApplicationFailure> {
        Self::new(
            package_id.into(),
            version.into(),
            true,
            package_lifecycle(extension_package::INITIAL),
        )
    }

    /// A package the user built or received and handed in directly.
    ///
    /// It was never in any directory, and it was not fetched: `Downloaded`
    /// records bytes that arrived over a source, so a local import enters the
    /// machine at the trust step with the digest measured on disk.
    pub fn local_import(
        package_id: impl Into<String>,
        version: impl Into<String>,
        trust: TrustRecord,
    ) -> Result<Self, ApplicationFailure> {
        let digest = trust.digest().to_owned();
        let channel = trust.channel();
        let mut machine = Self::new(package_id.into(), version.into(), false, channel)?;
        machine.digest = Some(digest);
        machine.trust = Some(trust);
        Ok(machine)
    }

    fn new(
        package_id: String,
        version: String,
        listed: bool,
        state: PackageLifecycle,
    ) -> Result<Self, ApplicationFailure> {
        if !licoup_extension_contracts::is_namespaced(&package_id) {
            return Err(refusal("package_identity_invalid", PACKAGE_STAGE).with_field("packageId"));
        }
        if !licoup_extension_contracts::is_semver(&version) {
            return Err(refusal("package_identity_invalid", PACKAGE_STAGE).with_field("version"));
        }
        Ok(Self {
            package_id,
            version,
            listed,
            state,
            enabled: false,
            digest: None,
            trust: None,
            installed_at_unix_ms: None,
        })
    }

    pub fn package_id(&self) -> &str {
        &self.package_id
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    pub fn state(&self) -> PackageLifecycle {
        self.state
    }

    pub fn digest(&self) -> Option<&str> {
        self.digest.as_deref()
    }

    pub fn trust(&self) -> Option<&TrustRecord> {
        self.trust.as_ref()
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn installed_at_unix_ms(&self) -> Option<i64> {
        self.installed_at_unix_ms
    }

    /// The facts a client surface shows, straight from this machine.
    pub fn facts(&self) -> PackageFacts {
        PackageFacts {
            available: self.listed,
            installed: self.state.is_installed(),
            enabled: self.enabled,
            active: false,
        }
    }

    /// Record that the bytes arrived.
    pub fn observe_downloaded(&mut self) -> Result<(), ApplicationFailure> {
        self.advance(PackageLifecycle::Downloaded)
    }

    /// Record the user's or the update mechanism's decision about these bytes.
    pub fn record_trust(&mut self, trust: TrustRecord) -> Result<(), ApplicationFailure> {
        if self.state != PackageLifecycle::Downloaded {
            return Err(transition_refusal(self.state, trust.channel()));
        }
        let digest = trust.digest().to_owned();
        self.advance(trust.channel())?;
        self.digest = Some(digest);
        self.trust = Some(trust);
        Ok(())
    }

    pub fn mark_staged(&mut self) -> Result<(), ApplicationFailure> {
        self.advance(PackageLifecycle::Staged)
    }

    /// Publish the staged bytes as the installed version.
    ///
    /// Installing decides availability, not activity: a package is enabled for
    /// on-demand activation (or left disabled when the user asked for that) and
    /// no capability is started here.
    pub fn commit(&mut self, activation: InstallActivation) -> Result<(), ApplicationFailure> {
        if self.state != PackageLifecycle::Staged {
            return Err(transition_refusal(self.state, PackageLifecycle::Installed));
        }
        self.advance(PackageLifecycle::Installed)?;
        self.enabled = activation == InstallActivation::EnabledOnDemand;
        self.installed_at_unix_ms = Some(now_unix_ms());
        Ok(())
    }

    /// Switch an installed package off. Its files stay where they are.
    pub fn disable(&mut self) -> Result<(), ApplicationFailure> {
        if !self.state.is_installed() {
            return Err(
                actionable("package_not_installed", PACKAGE_STAGE, "packageId")
                    .with_presentation_arg("package", &self.package_id),
            );
        }
        self.enabled = false;
        Ok(())
    }

    pub fn enable(&mut self) -> Result<(), ApplicationFailure> {
        if !self.state.is_installed() {
            return Err(
                actionable("package_not_installed", PACKAGE_STAGE, "packageId")
                    .with_presentation_arg("package", &self.package_id),
            );
        }
        self.enabled = true;
        Ok(())
    }

    /// Take one lifecycle step, refusing a step the machine could not have made.
    pub fn advance(&mut self, next: PackageLifecycle) -> Result<(), ApplicationFailure> {
        let Some(next) = package_next(self.state, next) else {
            return Err(transition_refusal(self.state, next));
        };
        self.state = next;
        Ok(())
    }
}

fn transition_refusal(from: PackageLifecycle, to: PackageLifecycle) -> ApplicationFailure {
    refusal("package_state_transition_invalid", PACKAGE_STAGE)
        .with_presentation_arg("from", lifecycle_name(from))
        .with_presentation_arg("to", lifecycle_name(to))
}

fn lifecycle_name(state: PackageLifecycle) -> &'static str {
    package_state(state).as_str()
}

fn instance_state_name(state: InstanceLifecycle) -> &'static str {
    instance_state(state).as_str()
}

/// The four facts that identify one running instance, kept apart.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstanceIdentity {
    /// This instance. A package version may have several.
    pub instance_id: String,
    pub package_id: String,
    /// The version of the package this instance was prepared from.
    pub package_version: String,
    /// Which prepared release of that version this instance runs.
    pub generation: u64,
    /// Which committed capability catalogue this instance was admitted under.
    pub registry_epoch: u64,
    /// The permission scope this instance runs with. Two instances of the same
    /// package version may differ here, which is why the version alone is never
    /// an identity.
    pub permission_scope: Vec<String>,
}

impl InstanceIdentity {
    pub fn new(
        instance_id: impl Into<String>,
        package_id: impl Into<String>,
        package_version: impl Into<String>,
        generation: u64,
        registry_epoch: u64,
        permission_scope: impl IntoIterator<Item = String>,
    ) -> Result<Self, ApplicationFailure> {
        let identity = Self {
            instance_id: instance_id.into(),
            package_id: package_id.into(),
            package_version: package_version.into(),
            generation,
            registry_epoch,
            permission_scope: permission_scope.into_iter().collect(),
        };
        identity.validate()?;
        Ok(identity)
    }

    pub fn validate(&self) -> Result<(), ApplicationFailure> {
        if self.instance_id.is_empty()
            || !licoup_extension_contracts::is_namespaced(&self.package_id)
            || !licoup_extension_contracts::is_semver(&self.package_version)
            || self.generation == 0
            || self.registry_epoch == 0
        {
            return Err(refusal("package_instance_identity_invalid", INSTANCE_STAGE)
                .with_field("instanceId"));
        }
        Ok(())
    }

    /// The same package version, but not necessarily the same instance.
    pub fn same_package_version(&self, other: &Self) -> bool {
        self.package_id == other.package_id && self.package_version == other.package_version
    }

    /// A short human-readable key naming all four facts, so a log line can never
    /// read as if one version explained an instance.
    pub fn describe(&self) -> String {
        format!(
            "{}@{}/instance={}/generation={}/epoch={}",
            self.package_id,
            self.package_version,
            self.instance_id,
            self.generation,
            self.registry_epoch
        )
    }
}

/// Whether the instance still accepts new work.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Admission {
    Open,
    Withdrawn,
}

/// What happened to one unit of admitted in-flight work.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Settlement {
    /// The effect came back and was accounted for.
    Completed,
    /// The user cancelled or the process died. The outcome is `Unknown`: it is
    /// recorded, never re-dispatched and never reported as success.
    Unknown,
}

/// One running instance of a package version.
#[derive(Clone, Debug)]
pub struct InstanceMachine {
    identity: InstanceIdentity,
    state: InstanceLifecycle,
    admission: Admission,
    in_flight: u32,
    settled: u32,
    unknown: u32,
    note: Option<String>,
}

impl InstanceMachine {
    pub fn discovered(identity: InstanceIdentity) -> Result<Self, ApplicationFailure> {
        identity.validate()?;
        Ok(Self {
            identity,
            state: instance_lifecycle(extension_instance::INITIAL),
            admission: Admission::Open,
            in_flight: 0,
            settled: 0,
            unknown: 0,
            note: None,
        })
    }

    pub fn identity(&self) -> &InstanceIdentity {
        &self.identity
    }

    pub fn state(&self) -> InstanceLifecycle {
        self.state
    }

    pub fn admission(&self) -> Admission {
        self.admission
    }

    pub fn in_flight(&self) -> u32 {
        self.in_flight
    }

    pub fn settled(&self) -> u32 {
        self.settled
    }

    pub fn unknown(&self) -> u32 {
        self.unknown
    }

    pub fn note(&self) -> Option<&str> {
        self.note.as_deref()
    }

    pub fn is_running(&self) -> bool {
        !matches!(
            self.state,
            InstanceLifecycle::Stopped | InstanceLifecycle::Failed
        )
    }

    /// Take one lifecycle step, refusing a step the machine could not have made.
    pub(crate) fn advance(&mut self, next: InstanceLifecycle) -> Result<(), ApplicationFailure> {
        let Some(next) = instance_next(self.state, next) else {
            return Err(
                refusal("package_instance_transition_invalid", INSTANCE_STAGE)
                    .with_presentation_arg("from", instance_state_name(self.state))
                    .with_presentation_arg("to", instance_state_name(next)),
            );
        };
        self.state = next;
        Ok(())
    }

    /// Begin preparing an instance of an installed package version.
    ///
    /// This is the activation entry. It consumes the store's compatibility
    /// admission for exactly this package version, so a package whose
    /// self-described list does not cover the running client cannot be started —
    /// including one that was installed under an earlier client and has been
    /// outgrown by a client update. Nothing is executed here: preparing is the
    /// admission, and the caller starts the program afterwards.
    pub fn prepare(
        admission: ActivationAdmission,
        identity: InstanceIdentity,
    ) -> Result<Self, ApplicationFailure> {
        if admission.package_id() != identity.package_id
            || admission.version() != identity.package_version
        {
            return Err(
                refusal("package_activation_admission_mismatch", INSTANCE_STAGE)
                    .with_field("instanceId")
                    .with_presentation_arg("package", &identity.package_id)
                    .with_presentation_arg("version", &identity.package_version),
            );
        }
        let mut machine = Self::discovered(identity)?;
        machine.advance(InstanceLifecycle::Preparing)?;
        Ok(machine)
    }

    /// Move a prepared instance to active.
    pub fn activate(&mut self) -> Result<(), ApplicationFailure> {
        self.advance(InstanceLifecycle::Active)
    }

    /// Stop accepting new work on this instance.
    ///
    /// This is the first half of an uninstall and it is idempotent, because a
    /// retried withdrawal must not fail the transaction it is protecting.
    pub fn withdraw_admission(&mut self) -> Result<(), ApplicationFailure> {
        self.admission = Admission::Withdrawn;
        Ok(())
    }

    /// Accept new work, or refuse because admission was withdrawn.
    pub fn admit(&mut self) -> Result<(), ApplicationFailure> {
        if self.admission == Admission::Withdrawn {
            return Err(
                actionable("package_admission_withdrawn", INSTANCE_STAGE, "instanceId")
                    .with_presentation_arg("instance", &self.identity.instance_id),
            );
        }
        if self.state != InstanceLifecycle::Active {
            return Err(refusal("package_instance_not_active", INSTANCE_STAGE)
                .with_presentation_arg("state", instance_state_name(self.state)));
        }
        Ok(())
    }

    /// Admit one in-flight unit of work.
    pub fn begin_in_flight(&mut self) -> Result<(), ApplicationFailure> {
        self.admit()?;
        self.in_flight += 1;
        Ok(())
    }

    /// Settle one in-flight unit of work.
    pub fn settle(&mut self, settlement: Settlement) -> Result<(), ApplicationFailure> {
        if self.in_flight == 0 {
            return Err(
                refusal("package_instance_settlement_without_work", INSTANCE_STAGE)
                    .with_field("instanceId"),
            );
        }
        self.in_flight -= 1;
        match settlement {
            Settlement::Completed => self.settled += 1,
            Settlement::Unknown => self.unknown += 1,
        }
        Ok(())
    }

    /// Move an active instance into draining.
    pub fn drain(&mut self) -> Result<(), ApplicationFailure> {
        self.withdraw_admission()?;
        self.advance(InstanceLifecycle::Draining)
    }

    /// Stop the instance.
    ///
    /// Refused while work is still admitted and unsettled: a stopped instance
    /// with unpinned work is how an effect disappears without a record.
    pub fn stop(&mut self) -> Result<(), ApplicationFailure> {
        self.withdraw_admission()?;
        if self.in_flight > 0 {
            return Err(refusal("package_instance_work_unsettled", INSTANCE_STAGE)
                .with_presentation_arg("instance", &self.identity.instance_id)
                .with_presentation_arg("inFlight", &self.in_flight.to_string()));
        }
        self.advance(InstanceLifecycle::Stopped)
    }

    pub fn fail(&mut self, note: impl Into<String>) -> Result<(), ApplicationFailure> {
        self.note = Some(note.into());
        self.advance(InstanceLifecycle::Failed)
    }

    pub fn quarantine(&mut self, note: impl Into<String>) -> Result<(), ApplicationFailure> {
        self.note = Some(note.into());
        self.advance(InstanceLifecycle::Quarantined)
    }

    /// A record for a surface: state plus the four identity facts plus the
    /// unsettled count.
    pub fn report(&self) -> InstanceReport {
        InstanceReport {
            identity: self.identity.clone(),
            state: self.state,
            admission: self.admission,
            in_flight: self.in_flight,
            unknown: self.unknown,
        }
    }
}

/// One instance as a surface shows it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstanceReport {
    pub identity: InstanceIdentity,
    pub state: InstanceLifecycle,
    pub admission: Admission,
    pub in_flight: u32,
    pub unknown: u32,
}

/// Every instance this machine knows about.
#[derive(Clone, Debug, Default)]
pub struct InstanceRegistry {
    instances: BTreeMap<String, InstanceMachine>,
}

impl InstanceRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, machine: InstanceMachine) -> &mut Self {
        self.instances
            .insert(machine.identity.instance_id.clone(), machine);
        self
    }

    pub fn get(&self, instance_id: &str) -> Option<&InstanceMachine> {
        self.instances.get(instance_id)
    }

    pub fn get_mut(&mut self, instance_id: &str) -> Option<&mut InstanceMachine> {
        self.instances.get_mut(instance_id)
    }

    pub fn len(&self) -> usize {
        self.instances.len()
    }

    pub fn is_empty(&self) -> bool {
        self.instances.is_empty()
    }

    pub fn instances(&self) -> impl Iterator<Item = &InstanceMachine> {
        self.instances.values()
    }

    /// Instances of one package that are not stopped.
    pub fn running_of(&self, package_id: &str) -> impl Iterator<Item = &InstanceMachine> {
        self.instances.values().filter(move |machine| {
            machine.identity.package_id == package_id && machine.is_running()
        })
    }

    pub fn active(&self) -> impl Iterator<Item = &InstanceMachine> {
        self.instances
            .values()
            .filter(|machine| machine.state == InstanceLifecycle::Active)
    }

    pub fn total_in_flight(&self) -> u32 {
        self.instances
            .values()
            .map(|machine| machine.in_flight)
            .sum()
    }

    /// Withdraw admission on every instance of one package. Returns how many
    /// instances were affected.
    pub fn withdraw_admission_of(&mut self, package_id: &str) -> usize {
        let mut affected = 0;
        for machine in self.instances.values_mut() {
            if machine.identity.package_id == package_id {
                let _ = machine.withdraw_admission();
                affected += 1;
            }
        }
        affected
    }

    /// Reconcile against what is actually running after a restart.
    ///
    /// An instance that the host was holding but that is no longer there is
    /// reported `Stopped` with a note, never quietly forgotten. The process that
    /// held its admitted work is gone, so that work cannot be maintained: each
    /// unit is settled `Unknown` — recorded, never re-dispatched and never
    /// reported as success — before the instance stops.
    pub fn reconcile_after_restart(&mut self, observed: &[String]) -> Vec<String> {
        let seen: BTreeSet<&str> = observed.iter().map(String::as_str).collect();
        let mut missing = Vec::new();
        for machine in self.instances.values_mut() {
            if machine.is_running() && !seen.contains(machine.identity.instance_id.as_str()) {
                machine.note = Some("missing at restart".to_owned());
                let _ = machine.withdraw_admission();
                while machine.in_flight > 0 {
                    let _ = machine.settle(Settlement::Unknown);
                }
                if machine.state == InstanceLifecycle::Active {
                    let _ = machine.advance(InstanceLifecycle::Draining);
                }
                let _ = machine.advance(InstanceLifecycle::Stopped);
                missing.push(machine.identity.instance_id.clone());
            }
        }
        missing
    }

    pub fn reports(&self) -> Vec<InstanceReport> {
        self.instances
            .values()
            .map(InstanceMachine::report)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn instance(instance_id: &str, generation: u64, scope: &[&str]) -> InstanceMachine {
        InstanceMachine::discovered(
            InstanceIdentity::new(
                instance_id,
                "example.specialist.echo",
                "1.0.0",
                generation,
                7,
                scope.iter().map(|it| (*it).to_owned()),
            )
            .expect("identity"),
        )
        .expect("instance")
    }

    #[test]
    fn a_package_walks_the_state_machine_in_order() {
        let mut machine =
            PackageMachine::available("example.specialist.echo", "1.0.0").expect("available");
        assert_eq!(machine.state(), PackageLifecycle::Available);
        assert!(machine.facts().available);

        machine.observe_downloaded().expect("downloaded");
        assert_eq!(
            machine
                .mark_staged()
                .expect_err("staging before trust")
                .code,
            "package_state_transition_invalid"
        );
        machine
            .record_trust(TrustRecord::local_approved("sha256:abc", []).expect("trust"))
            .expect("approved");
        assert_eq!(machine.state(), PackageLifecycle::LocalApproved);
        machine.mark_staged().expect("staged");
        machine
            .commit(InstallActivation::EnabledOnDemand)
            .expect("installed");

        let facts = machine.facts();
        assert!(facts.available && facts.installed && facts.enabled);
        assert!(!facts.active, "installing does not activate anything");
        assert!(facts.validate().is_ok());
    }

    #[test]
    fn installing_does_not_activate_and_disabling_keeps_the_files() {
        let mut machine = PackageMachine::local_import(
            "example.specialist.echo",
            "1.0.0",
            TrustRecord::local_approved("sha256:abc", []).expect("trust"),
        )
        .expect("import");
        assert_eq!(machine.state(), PackageLifecycle::LocalApproved);
        machine.mark_staged().expect("staged");
        machine
            .commit(InstallActivation::Disabled)
            .expect("install");

        assert!(machine.state().is_installed());
        assert!(!machine.enabled(), "asked for, and kept, switched off");
        machine.enable().expect("enable");
        machine.disable().expect("disable");
        assert!(machine.state().is_installed(), "still on disk");
    }

    #[test]
    fn a_local_import_is_not_available_anywhere() {
        let machine = PackageMachine::local_import(
            "example.specialist.echo",
            "1.0.0",
            TrustRecord::local_approved("sha256:abc", []).expect("trust"),
        )
        .expect("import");
        assert!(!machine.facts().available, "never in a directory");
        assert!(machine.facts().validate().is_ok());
    }

    #[test]
    fn trust_is_bound_to_content_and_scope() {
        let trust = TrustRecord::local_approved(
            "sha256:abc",
            [PermissionRequest::new("example.specialist/net", "self")],
        )
        .expect("trust");
        assert!(trust.check_content("sha256:abc").is_ok());
        assert_eq!(
            trust
                .check_content("sha256:def")
                .expect_err("other bytes")
                .code,
            "package_trust_not_bound_to_content"
        );
        assert!(
            trust
                .check_permissions(&[PermissionRequest::new("example.specialist/net", "self")])
                .is_ok()
        );

        let expanded = [PermissionRequest::new("example.specialist/fs", "/tmp")];
        let failure = trust
            .check_permissions(&expanded)
            .expect_err("a grown scope needs a new decision");
        assert_eq!(failure.code, "package_permission_scope_expanded");
        assert_eq!(trust.uncovered(&expanded).len(), 1);

        assert_eq!(
            TrustRecord::local_approved("", [])
                .expect_err("trust without content")
                .code,
            "package_trust_not_bound_to_content"
        );
    }

    #[test]
    fn a_verified_and_a_locally_approved_package_are_two_facts() {
        let mut machine =
            PackageMachine::available("example.specialist.echo", "1.0.0").expect("available");
        machine.observe_downloaded().expect("downloaded");
        machine
            .record_trust(TrustRecord::publisher_verified("sha256:abc", []).expect("verified"))
            .expect("verified");
        assert_eq!(machine.state(), PackageLifecycle::Verified);
        assert!(
            !machine.state().is_local_trust(),
            "a publisher signature is not a local approval"
        );

        // A verified package cannot then claim to be locally approved: the two
        // channels are facts about different checks.
        assert_eq!(
            machine
                .record_trust(TrustRecord::local_approved("sha256:abc", []).expect("trust"))
                .expect_err("not a step")
                .code,
            "package_state_transition_invalid"
        );
    }

    #[test]
    fn the_four_instance_facts_are_never_one_version() {
        let first = instance("instance-a", 1, &["example.specialist/net"]);
        let second = instance(
            "instance-b",
            1,
            &["example.specialist/net", "example.specialist/fs"],
        );
        assert!(first.identity().same_package_version(second.identity()));
        assert_ne!(
            first.identity().describe(),
            second.identity().describe(),
            "same version, different instance"
        );

        let next_generation = instance("instance-c", 2, &["example.specialist/net"]);
        assert_eq!(
            next_generation.identity().package_version,
            first.identity().package_version
        );
        assert_ne!(
            next_generation.identity().generation,
            first.identity().generation
        );
        assert_eq!(
            next_generation.identity().registry_epoch,
            first.identity().registry_epoch,
            "a new generation inside one epoch is still the same catalogue"
        );

        let described = first.identity().describe();
        for fact in ["instance=", "generation=", "epoch="] {
            assert!(described.contains(fact), "{described}");
        }
    }

    #[test]
    fn an_instance_drains_before_it_stops_and_pins_its_work() {
        let mut machine = instance("instance-1", 1, &[]);
        machine
            .advance(InstanceLifecycle::Preparing)
            .expect("preparing");
        machine.advance(InstanceLifecycle::Active).expect("active");
        machine.begin_in_flight().expect("admit work");

        let failure = machine.stop().expect_err("work is unsettled");
        assert_eq!(failure.code, "package_instance_work_unsettled");

        machine.drain().expect("drain");
        assert_eq!(machine.admission(), Admission::Withdrawn);
        assert_eq!(
            machine.admit().expect_err("no new admissions").code,
            "package_admission_withdrawn"
        );
        machine.settle(Settlement::Unknown).expect("settle");
        assert_eq!(machine.unknown(), 1);
        assert_eq!(machine.settled(), 0);
        machine.stop().expect("stopped");
        assert_eq!(machine.state(), InstanceLifecycle::Stopped);
    }

    #[test]
    fn a_failed_instance_stays_visible_and_keeps_its_pins() {
        let mut machine = instance("instance-1", 1, &[]);
        machine
            .advance(InstanceLifecycle::Preparing)
            .expect("preparing");
        machine.advance(InstanceLifecycle::Active).expect("active");
        machine.begin_in_flight().expect("work");
        machine.fail("the adapter exited").expect("failed");
        assert_eq!(machine.state(), InstanceLifecycle::Failed);
        assert_eq!(
            machine.in_flight(),
            1,
            "unknown work is never re-dispatched"
        );
        machine.quarantine("hash mismatch").expect("quarantined");
        assert_eq!(machine.state(), InstanceLifecycle::Quarantined);
    }

    #[test]
    fn a_restart_that_loses_an_instance_reports_it() {
        let mut registry = InstanceRegistry::new();
        let mut lost = instance("instance-1", 1, &[]);
        lost.advance(InstanceLifecycle::Preparing)
            .expect("preparing");
        lost.advance(InstanceLifecycle::Active).expect("active");
        lost.begin_in_flight().expect("admitted work");
        registry.insert(lost);
        registry.insert(instance("instance-2", 2, &[]));

        let missing = registry.reconcile_after_restart(&["instance-2".to_owned()]);
        assert_eq!(missing, vec!["instance-1".to_owned()]);
        let stopped = registry.get("instance-1").expect("instance");
        assert_eq!(stopped.state(), InstanceLifecycle::Stopped);
        assert_eq!(stopped.note(), Some("missing at restart"));
        assert_eq!(
            stopped.unknown(),
            1,
            "work the vanished process held is Unknown, not completed"
        );
        assert_eq!(stopped.in_flight(), 0);
        assert_eq!(stopped.admission(), Admission::Withdrawn);
    }

    #[test]
    fn withdrawing_admission_touches_every_instance_of_one_package() {
        let mut registry = InstanceRegistry::new();
        registry.insert(instance("instance-1", 1, &[]));
        registry.insert(instance("instance-2", 2, &[]));
        let affected = registry.withdraw_admission_of("example.specialist.echo");
        assert_eq!(affected, 2);
        assert!(
            registry
                .instances()
                .all(|machine| machine.admission() == Admission::Withdrawn)
        );
        assert_eq!(registry.withdraw_admission_of("example.other"), 0);
    }
}
