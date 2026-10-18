//! Two operations that look alike and must never be one.
//!
//! **Selection** is the user's preference: which installed theme, layout,
//! style, font, language or composition this client serves. It moves a
//! presentation binding and nothing else, so it stays available while unrelated
//! work runs — including a project turn with admitted work in flight. Waiting
//! for a host-wide idle to change a font would be exactly the "new all-project
//! idle rule for a cosmetic choice" the appearance work is required not to
//! introduce.
//!
//! **A package generation entering served state** is an update: bytes that were
//! prepared at one generation become what this host serves, which is state
//! admitted work is already reading. It happens through one guarded path,
//! [`ResourceHost::begin_replacement`], which closes new-work admission *before*
//! anything may be published and refuses while this host still owns unfinished
//! work. There is no unguarded variant of the same mutation for an
//! update-switch interleaving to reach.
//!
//! Three rules are enforced by construction rather than described:
//!
//! 1. **Nothing is published before admission closes.** The only value that can
//!    publish a generation is a [`GenerationReplacement`], and the only way to
//!    obtain one is [`ResourceHost::begin_replacement`], which asks the
//!    admission question first. "Publish, then close admission" is not a
//!    representable call order.
//! 2. **One snapshot serves one set of bindings.** [`ResourceBindings`] carries
//!    every kind's binding in a single immutable value behind one `Arc`, so a
//!    refused switch leaves the value it refused to replace exactly as it was
//!    and no reader can observe half of a replacement.
//! 3. **A binding always resolves to something.** A resource that is disabled,
//!    uninstalled, or dropped by the next generation falls back to the declared
//!    system default for its kind ([`declared_default`]) — never to a
//!    half-applied binding — and the fallback is recorded with the resource,
//!    the package and the reason it stopped being served
//!    ([`ResourceFallback`]).
//!
//! What a fallback is *not*: the user's preference. [`ResourceHost::withdraw`]
//! keeps the selection and stops serving it, so re-enabling or reinstalling the
//! package restores the user's choice instead of silently resetting it to the
//! system default.
//!
//! # The one-line seam this owner declares
//!
//! The idle/update admission decision belongs to
//! `crate::domain::work_admission`, and the platform layer must not reach into
//! [`crate::domain`]. So this module declares the two facts it needs as the
//! port [`MaintenanceAdmission`], and the crate-root composition answers them
//! once per process, above both layers. The whole adapter is:
//!
//! ```ignore
//! // crates/licoup-native/src/lib.rs, above both layers.
//! struct PackageGenerationAdmission;
//!
//! impl platform::extension_packages::MaintenanceAdmission for PackageGenerationAdmission {
//!     fn hold(&self, data_root: &std::path::Path) -> Result<(), &'static str> {
//!         domain::work_admission::hold_package_activation_admission(data_root)
//!     }
//!
//!     fn release(&self, data_root: &std::path::Path) -> Result<(), &'static str> {
//!         domain::work_admission::release_maintenance_admission(data_root)
//!     }
//! }
//!
//! // and, in `install_environment_ports`:
//! platform::extension_packages::install_maintenance_admission(std::sync::Arc::new(
//!     PackageGenerationAdmission,
//! ))?;
//! ```
//!
//! Until that composition is installed, the port answers
//! [`ADMISSION_UNAVAILABLE`] and every generation switch is refused. That is the
//! fail-closed direction on purpose: a host that cannot say whether it still
//! owns unfinished work must not replace a generation underneath it.
//!
//! Persistence is deliberately out of scope here: this value owns *what is
//! served right now*, and a durable preference record is a separate owner's.

use crate::platform::extension_packages::mount_plan::{
    HOST_ACTIONS, PublishedGeneration, ResourceMountPlan, plan_published_generations,
    publish_mount_plan as write_mount_plan,
};
use crate::platform::extension_packages::{actionable, refusal};
use licoup_application::{ApplicationFailure, RecoveryAction};
use licoup_extension_contracts::manifest::{ResourceDeclaration, ResourceKind};
use licoup_extension_contracts::{is_namespaced, is_semver};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

const RESOURCE_STAGE: &str = "extension/resource-selection";
const GENERATION_STAGE: &str = "extension/resource-generation";

/// The most resources one prepared generation may carry, so a declaration
/// cannot make the switch unbounded work.
pub const MAX_GENERATION_RESOURCES: usize = 64;

// ---------------------------------------------------------------------------
// The admission seam
// ---------------------------------------------------------------------------

/// The seam's refusals, spelled exactly as the decision owner spells them, so
/// the composition adapter is a straight pass-through and this owner never has
/// to read the other layer's text.
pub const ADMISSION_BLOCKED: &str = "maintenance_admission_blocked";
/// Another maintenance switch already holds the close-admission barrier.
pub const ADMISSION_CLOSED: &str = "maintenance_admission_closed";
/// Nothing answered the question. Never treated as "idle".
pub const ADMISSION_UNAVAILABLE: &str = "maintenance_admission_unavailable";
/// The release did not happen; admission stays closed.
pub const ADMISSION_RELEASE_FAILED: &str = "maintenance_admission_release_failed";

/// One host's answer for the package-generation admission question.
///
/// This is the whole seam. The decision owner reads what unfinished work this
/// host still holds and takes the close-admission barrier in the same call, so
/// a task arriving between the answer and the switch either observes the
/// barrier or loses to it.
pub trait MaintenanceAdmission: Send + Sync {
    /// Close new-work admission for one package-generation switch.
    ///
    /// `Ok(())` means this caller owns the switch and must call
    /// [`MaintenanceAdmission::release`] when it succeeds or aborts. The
    /// refusal constants above are the seam's vocabulary.
    fn hold(&self, data_root: &Path) -> Result<(), &'static str>;

    /// Release the closed admission after the switch succeeded or aborted.
    ///
    /// Releasing when nothing is held is not an error: a release must stay
    /// idempotent so a failure path cannot leave admission closed by accident.
    fn release(&self, data_root: &Path) -> Result<(), &'static str>;
}

/// One process's installed admission answer.
///
/// The production program has exactly one; tests build their own so the
/// fail-closed default and an installed answer are both observable without
/// depending on test order.
pub struct MaintenanceAdmissionPort {
    answer: OnceLock<Arc<dyn MaintenanceAdmission>>,
}

impl MaintenanceAdmissionPort {
    pub const fn new() -> Self {
        Self {
            answer: OnceLock::new(),
        }
    }

    /// Install the composition's answer. One answer per port: a second
    /// installation is refused rather than silently replacing the first.
    pub fn install(&self, answer: Arc<dyn MaintenanceAdmission>) -> Result<(), &'static str> {
        self.answer
            .set(answer)
            .map_err(|_| "maintenance admission is already installed")
    }

    pub fn is_installed(&self) -> bool {
        self.answer.get().is_some()
    }

    /// The installed answer's, or the fail-closed one.
    pub fn hold(&self, data_root: &Path) -> Result<(), &'static str> {
        match self.answer.get() {
            Some(answer) => answer.hold(data_root),
            None => Err(ADMISSION_UNAVAILABLE),
        }
    }

    pub fn release(&self, data_root: &Path) -> Result<(), &'static str> {
        match self.answer.get() {
            Some(answer) => answer.release(data_root),
            None => Err(ADMISSION_UNAVAILABLE),
        }
    }
}

impl Default for MaintenanceAdmissionPort {
    fn default() -> Self {
        Self::new()
    }
}

static MAINTENANCE_ADMISSION: MaintenanceAdmissionPort = MaintenanceAdmissionPort::new();

/// The process-wide port every [`ResourceHost::open`] reads.
pub fn maintenance_admission_port() -> &'static MaintenanceAdmissionPort {
    &MAINTENANCE_ADMISSION
}

/// Install this process's admission answer for the crate-root composition.
pub fn install_maintenance_admission(
    answer: Arc<dyn MaintenanceAdmission>,
) -> Result<(), &'static str> {
    MAINTENANCE_ADMISSION.install(answer)
}

/// Where the answer comes from for one host.
enum AdmissionSource {
    /// The process-wide port, read at each use so an installation that happens
    /// after a host opened is still seen.
    Process,
    /// One explicit answer, for a test or a composition that owns its own.
    Explicit(Arc<dyn MaintenanceAdmission>),
}

impl AdmissionSource {
    fn hold(&self, data_root: &Path) -> Result<(), &'static str> {
        match self {
            Self::Process => maintenance_admission_port().hold(data_root),
            Self::Explicit(answer) => answer.hold(data_root),
        }
    }

    fn release(&self, data_root: &Path) -> Result<(), &'static str> {
        match self {
            Self::Process => maintenance_admission_port().release(data_root),
            Self::Explicit(answer) => answer.release(data_root),
        }
    }
}

// ---------------------------------------------------------------------------
// What one kind serves
// ---------------------------------------------------------------------------

/// The declared default a kind serves when no installed resource is selected.
///
/// These are the client's own system facts — its base appearance, its system
/// font and its system locale — not a bundled fallback package and not a
/// protected baseline: removing the optional package that supplied a resource
/// returns the user to the platform's own rendering.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum SystemDefault {
    /// The platform appearance: colours, layout regions, styles and composed
    /// components as this client ships them.
    Appearance,
    /// The platform base font.
    Font,
    /// The platform locale.
    Locale,
}

impl SystemDefault {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Appearance => "system-appearance",
            Self::Font => "system-font",
            Self::Locale => "system-locale",
        }
    }
}

/// The declared default of one kind.
///
/// A composition is part of the appearance this client renders, so it falls
/// back with the same system appearance a theme does.
pub const fn declared_default(kind: ResourceKind) -> SystemDefault {
    match kind {
        ResourceKind::Theme
        | ResourceKind::Layout
        | ResourceKind::Style
        | ResourceKind::Composition => SystemDefault::Appearance,
        ResourceKind::Font => SystemDefault::Font,
        ResourceKind::Language => SystemDefault::Locale,
    }
}

/// One resource this host holds and may serve: a typed declaration of a package
/// generation that was published through the guarded path.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AvailableResource {
    pub package_id: String,
    pub version: String,
    /// Which prepared release of that package version the resource came from.
    pub package_generation: u64,
    pub declaration: ResourceDeclaration,
}

impl AvailableResource {
    pub fn resource_id(&self) -> &str {
        self.declaration.id()
    }

    pub fn kind(&self) -> ResourceKind {
        self.declaration.kind()
    }
}

/// What one kind currently serves.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResourceBinding {
    /// An installed resource the user selected.
    Selected {
        resource_id: String,
        package_id: String,
        version: String,
        package_generation: u64,
    },
    /// The declared default: the system appearance, font or locale.
    Default { system: SystemDefault },
}

impl ResourceBinding {
    /// The resource this binding serves, when it serves one.
    pub fn resource_id(&self) -> Option<&str> {
        match self {
            Self::Selected { resource_id, .. } => Some(resource_id),
            Self::Default { .. } => None,
        }
    }

    /// The package generation this binding serves, when it serves one.
    pub fn package_generation(&self) -> Option<u64> {
        match self {
            Self::Selected {
                package_generation, ..
            } => Some(*package_generation),
            Self::Default { .. } => None,
        }
    }

    pub fn is_default(&self) -> bool {
        matches!(self, Self::Default { .. })
    }
}

/// Why a kind stopped serving the resource the user had selected.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FallbackReason {
    /// The package was switched off. Its files are still on disk, and the
    /// preference is kept for when it is switched on again.
    Disabled,
    /// The package was uninstalled, or that version was removed.
    Uninstalled,
    /// The package's next generation no longer carries the selected resource.
    Replaced,
}

impl FallbackReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Uninstalled => "uninstalled",
            Self::Replaced => "replaced",
        }
    }
}

/// One recorded fallback: what was selected, and what happened to it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResourceFallback {
    pub kind: ResourceKind,
    /// The resource the user had selected. The preference survives the
    /// fallback; what this host serves does not.
    pub resource_id: String,
    /// The package that carried it.
    pub package_id: String,
    pub reason: FallbackReason,
}

/// One immutable published set of bindings.
///
/// Every kind is present, so a surface reads a binding rather than an absence,
/// and the whole set moves at once: a reader sees the revision before a switch
/// or the revision after it, never a mixture.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResourceBindings {
    revision: u64,
    bindings: BTreeMap<ResourceKind, ResourceBinding>,
    fallbacks: BTreeMap<ResourceKind, ResourceFallback>,
}

impl ResourceBindings {
    /// This host's binding revision: it advances on every published change,
    /// whether that change was an ordinary selection or a generation switch.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// What one kind serves. Every published kind answers.
    pub fn binding(&self, kind: ResourceKind) -> &ResourceBinding {
        self.bindings
            .get(&kind)
            .expect("every kind is resolved when a snapshot is built")
    }

    pub fn bindings(&self) -> impl Iterator<Item = (ResourceKind, &ResourceBinding)> {
        self.bindings.iter().map(|(kind, binding)| (*kind, binding))
    }

    /// The recorded fallback for one kind, when that kind is not serving the
    /// resource the user selected.
    pub fn fallback(&self, kind: ResourceKind) -> Option<&ResourceFallback> {
        self.fallbacks.get(&kind)
    }

    pub fn fallbacks(&self) -> impl Iterator<Item = &ResourceFallback> {
        self.fallbacks.values()
    }

    /// Whether any kind serves a resource of one package generation.
    pub fn serves_package_generation(&self, package_id: &str, package_generation: u64) -> bool {
        self.bindings.values().any(|binding| {
            matches!(
                binding,
                ResourceBinding::Selected {
                    package_id: served_package,
                    package_generation: served_generation,
                    ..
                } if served_package == package_id && *served_generation == package_generation
            )
        })
    }
}

// ---------------------------------------------------------------------------
// The guarded switch
// ---------------------------------------------------------------------------

/// A package generation prepared for the switch, with the typed resources it
/// would serve.
///
/// Constructing one validates the declaration, so the switch itself cannot fail
/// halfway: by the time a generation may be published, every input it carries
/// has already been refused or accepted.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreparedGeneration {
    pub package_id: String,
    pub version: String,
    pub package_generation: u64,
    pub resources: Vec<ResourceDeclaration>,
}

impl PreparedGeneration {
    pub fn new(
        package_id: impl Into<String>,
        version: impl Into<String>,
        package_generation: u64,
        resources: Vec<ResourceDeclaration>,
    ) -> Result<Self, ApplicationFailure> {
        let package_id = package_id.into();
        let version = version.into();
        if !is_namespaced(&package_id) {
            return Err(
                refusal("package_generation_identity_invalid", GENERATION_STAGE)
                    .with_field("packageId"),
            );
        }
        if !is_semver(&version) {
            return Err(
                refusal("package_generation_identity_invalid", GENERATION_STAGE)
                    .with_field("version"),
            );
        }
        if package_generation == 0 {
            return Err(
                refusal("package_generation_identity_invalid", GENERATION_STAGE)
                    .with_field("packageGeneration"),
            );
        }
        if resources.len() > MAX_GENERATION_RESOURCES {
            return Err(
                refusal("package_generation_identity_invalid", GENERATION_STAGE)
                    .with_field("resources"),
            );
        }
        let mut seen: Vec<&str> = Vec::with_capacity(resources.len());
        for declaration in &resources {
            declaration.validate()?;
            if seen.contains(&declaration.id()) {
                return Err(
                    refusal("package_generation_identity_invalid", GENERATION_STAGE)
                        .with_field("resources.id")
                        .with_presentation_arg("resource", declaration.id()),
                );
            }
            seen.push(declaration.id());
        }
        Ok(Self {
            package_id,
            version,
            package_generation,
            resources,
        })
    }
}

/// The guarded switch, held between closing new-work admission and publishing.
///
/// Dropping this value without deciding releases admission: a caller that
/// forgot to decide has aborted, and leaving the host closed by accident is the
/// failure this value exists to prevent.
pub struct GenerationReplacement<'a> {
    host: &'a mut ResourceHost,
    prepared: PreparedGeneration,
    decided: bool,
}

impl GenerationReplacement<'_> {
    /// The generation this switch would publish.
    pub fn prepared(&self) -> &PreparedGeneration {
        &self.prepared
    }

    /// Publish the whole generation, then release admission.
    ///
    /// The publication is one exchange of the binding snapshot, so no reader
    /// observes two generations and no half-applied set can exist. A release
    /// that does not happen leaves admission closed, which the outcome reports
    /// rather than hides.
    pub fn commit(mut self) -> ReplacementOutcome {
        let outcome = self.host.publish(&self.prepared);
        self.decided = true;
        let admission_released = self.host.admission.release(&self.host.data_root).is_ok();
        ReplacementOutcome {
            admission_released,
            ..outcome
        }
    }

    /// Decide against the switch: release admission and change nothing.
    pub fn abort(mut self) -> AdmissionRelease {
        self.decided = true;
        AdmissionRelease {
            released: self.host.admission.release(&self.host.data_root).is_ok(),
        }
    }
}

impl Drop for GenerationReplacement<'_> {
    fn drop(&mut self) {
        if !self.decided {
            let _ = self.host.admission.release(&self.host.data_root);
        }
    }
}

impl std::fmt::Debug for GenerationReplacement<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GenerationReplacement")
            .field("package_id", &self.prepared.package_id)
            .field("package_generation", &self.prepared.package_generation)
            .field("decided", &self.decided)
            .finish()
    }
}

/// Whether admission was released after a switch was abandoned.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AdmissionRelease {
    /// `false` means new work stays refused until the owner or its recovery
    /// releases the barrier. It is reported, never assumed away.
    pub released: bool,
}

/// What one guarded generation switch did.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReplacementOutcome {
    pub package_id: String,
    pub package_generation: u64,
    /// The generation this replaced, when the package already served one.
    pub replaced_generation: Option<u64>,
    /// Kinds that fell back because the next generation no longer carries what
    /// the user had selected.
    pub fallbacks: Vec<ResourceFallback>,
    /// The revision this switch replaced.
    pub previous_revision: u64,
    /// The revision it published.
    pub revision: u64,
    /// Whether the document for this revision reached the client.
    ///
    /// `false` means the switch is served here and the client still holds the
    /// document before it; the failure itself is read from
    /// [`ResourceHost::document_failure`].
    pub plan_published: bool,
    /// Whether admission was released afterwards.
    pub admission_released: bool,
}

/// What one ordinary selection did.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SelectionOutcome {
    pub kind: ResourceKind,
    pub binding: ResourceBinding,
    pub revision: u64,
}

/// The change that stopped a package serving its resources.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResourceChange {
    /// The package was switched off; its files stay on disk.
    Disabled,
    /// The package was uninstalled or its version was removed.
    Uninstalled,
}

impl ResourceChange {
    const fn reason(self) -> FallbackReason {
        match self {
            Self::Disabled => FallbackReason::Disabled,
            Self::Uninstalled => FallbackReason::Uninstalled,
        }
    }
}

// ---------------------------------------------------------------------------
// The owner
// ---------------------------------------------------------------------------

/// What this host serves, what the user asked for, and what is available to
/// serve it from.
///
/// The three are separate facts on purpose. `available` is what published
/// package generations carry; `selection` is the user's preference, which
/// outlives a package being switched off or removed; `active` is the single
/// snapshot a surface reads. An ordinary [`ResourceHost::select`] moves the
/// snapshot and never touches the admission question; only
/// [`ResourceHost::begin_replacement`] does, and it closes admission before the
/// snapshot it guards may move.
pub struct ResourceHost {
    data_root: PathBuf,
    admission: AdmissionSource,
    revision: u64,
    active: Arc<ResourceBindings>,
    selection: BTreeMap<ResourceKind, String>,
    fallbacks: BTreeMap<ResourceKind, ResourceFallback>,
    available: BTreeMap<String, AvailableResource>,
    generations: BTreeMap<String, u64>,
    /// The last publication's failure, when the document could not be written.
    /// Served state and the published document are one fact, so a host that
    /// moved its revision keeps the failure until a caller reads it.
    document_failure: Option<ApplicationFailure>,
}

impl ResourceHost {
    /// Bind this host to the data root the admission owner decides about,
    /// through the process-wide port.
    ///
    /// Opening creates nothing: the root is the key one question is asked with,
    /// and the bindings live in this value.
    pub fn open(data_root: impl Into<PathBuf>) -> Self {
        Self::with_admission_source(data_root.into(), AdmissionSource::Process)
    }

    /// Bind this host to one explicit admission answer.
    ///
    /// A test uses this so the fail-closed default and an installed answer are
    /// both observable in one process.
    pub fn with_admission(
        data_root: impl Into<PathBuf>,
        answer: Arc<dyn MaintenanceAdmission>,
    ) -> Self {
        Self::with_admission_source(data_root.into(), AdmissionSource::Explicit(answer))
    }

    fn with_admission_source(data_root: PathBuf, admission: AdmissionSource) -> Self {
        let active = Arc::new(resolve(
            0,
            &BTreeMap::new(),
            &BTreeMap::new(),
            &BTreeMap::new(),
        ));
        Self {
            data_root,
            admission,
            revision: 0,
            active,
            selection: BTreeMap::new(),
            fallbacks: BTreeMap::new(),
            available: BTreeMap::new(),
            generations: BTreeMap::new(),
            document_failure: None,
        }
    }

    /// The root the admission question is asked with.
    pub fn data_root(&self) -> &Path {
        &self.data_root
    }

    /// The published snapshot. Cloning the handle is how a surface reads one
    /// revision without holding the owner.
    pub fn bindings(&self) -> Arc<ResourceBindings> {
        Arc::clone(&self.active)
    }

    pub fn revision(&self) -> u64 {
        self.active.revision
    }

    /// What the user asked for, whether or not it is being served.
    pub fn selection(&self, kind: ResourceKind) -> Option<&str> {
        self.selection.get(&kind).map(String::as_str)
    }

    /// Every resource a published generation makes available.
    pub fn available(&self) -> impl Iterator<Item = &AvailableResource> {
        self.available.values()
    }

    /// The generation this host serves for one package, when it serves one.
    pub fn served_generation(&self, package_id: &str) -> Option<u64> {
        self.generations.get(package_id).copied()
    }

    /// The document the client renders from: one plan over every generation
    /// this host publishes.
    ///
    /// Every decision is made against the published snapshot, so the plan is
    /// always about what this host serves and never about what a caller believes
    /// it serves.
    pub fn mount_plan(&self, host_actions: &[&str]) -> ResourceMountPlan {
        let bindings = self.bindings();
        let mut order: Vec<(&str, u64)> = Vec::new();
        let mut grouped: BTreeMap<(&str, u64), Vec<ResourceDeclaration>> = BTreeMap::new();
        for resource in self.available.values() {
            let key = (resource.package_id.as_str(), resource.package_generation);
            if !grouped.contains_key(&key) {
                order.push(key);
            }
            grouped
                .entry(key)
                .or_default()
                .push(resource.declaration.clone());
        }
        let generations: Vec<PublishedGeneration<'_>> = order
            .into_iter()
            .map(|(package_id, package_generation)| PublishedGeneration {
                package_id,
                package_generation,
                resources: grouped
                    .get(&(package_id, package_generation))
                    .map(Vec::as_slice)
                    .unwrap_or(&[]),
            })
            .collect();
        plan_published_generations(&bindings, &generations, host_actions)
    }

    /// Publish that document where the client reads it.
    ///
    /// The one place a plan leaves this host. It writes what this host serves
    /// right now, so the client can never render a revision the host no longer
    /// serves.
    pub fn publish_mount_plan(&self, host_actions: &[&str]) -> Result<(), ApplicationFailure> {
        write_mount_plan(&self.data_root, &self.mount_plan(host_actions))
    }

    /// The last publication's failure, when this host could not write the
    /// document for the revision it serves.
    pub fn document_failure(&self) -> Option<&ApplicationFailure> {
        self.document_failure.as_ref()
    }

    /// Report the last publication, consuming the failure so the next revision
    /// reports only its own.
    fn report_publication(&mut self) -> Result<(), ApplicationFailure> {
        match self.document_failure.take() {
            Some(failure) => Err(failure),
            None => Ok(()),
        }
    }

    /// Choose which installed resource one kind serves.
    ///
    /// This is an ordinary preference operation. It never asks the admission
    /// question and never waits for unrelated work: the resources it chooses
    /// among are already installed, and moving a binding does not replace a
    /// package generation.
    pub fn select(
        &mut self,
        kind: ResourceKind,
        resource_id: &str,
    ) -> Result<SelectionOutcome, ApplicationFailure> {
        let Some(resource) = self.available.get(resource_id) else {
            return Err(actionable(
                "package_resource_not_installed",
                RESOURCE_STAGE,
                "resourceId",
            )
            .with_presentation_arg("resource", resource_id)
            .with_presentation_arg("kind", kind.as_str()));
        };
        if resource.kind() != kind {
            return Err(refusal("package_resource_kind_mismatch", RESOURCE_STAGE)
                .with_field("resourceId")
                .with_presentation_arg("resource", resource_id)
                .with_presentation_arg("kind", kind.as_str())
                .with_presentation_arg("actualKind", resource.kind().as_str()));
        }
        self.selection.insert(kind, resource_id.to_owned());
        self.fallbacks.remove(&kind);
        let revision = self.republish();
        // A selection is served the moment the snapshot moves. A client that was
        // never told about it would render the revision before this one, so a
        // publication that failed is reported rather than left in memory.
        self.report_publication()?;
        Ok(SelectionOutcome {
            kind,
            binding: self.active.binding(kind).clone(),
            revision,
        })
    }

    /// Serve the declared system default for one kind, as the user's own choice.
    ///
    /// This clears the stored preference: the user asked for the platform's
    /// rendering, which is not a fallback and is not recorded as one.
    pub fn select_default(&mut self, kind: ResourceKind) -> SelectionOutcome {
        self.selection.remove(&kind);
        self.fallbacks.remove(&kind);
        let revision = self.republish();
        SelectionOutcome {
            kind,
            binding: self.active.binding(kind).clone(),
            revision,
        }
    }

    /// A package's resources stopped being served: it was switched off, or its
    /// version was removed.
    ///
    /// Every kind whose selected resource this package carried falls back to
    /// its declared default in one snapshot, and the fallback is recorded with
    /// the change that caused it. The selection is kept, so switching the
    /// package back on restores the user's choice.
    ///
    /// A fallback is not an update and does not wait for idle: the user removed
    /// the resource, and continuing to serve it would be the half-applied state
    /// this path exists to prevent.
    pub fn withdraw(&mut self, package_id: &str, change: ResourceChange) -> Vec<ResourceFallback> {
        let removed: Vec<String> = self
            .available
            .values()
            .filter(|resource| resource.package_id == package_id)
            .map(|resource| resource.resource_id().to_owned())
            .collect();
        if removed.is_empty() {
            return Vec::new();
        }
        for resource_id in &removed {
            self.available.remove(resource_id);
        }
        self.generations.remove(package_id);

        let mut recorded = Vec::new();
        for kind in ResourceKind::ALL {
            let Some(selected) = self.selection.get(&kind).cloned() else {
                continue;
            };
            if !removed.contains(&selected) {
                continue;
            }
            let fallback = ResourceFallback {
                kind,
                resource_id: selected,
                package_id: package_id.to_owned(),
                reason: change.reason(),
            };
            self.fallbacks.insert(kind, fallback.clone());
            recorded.push(fallback);
        }
        self.republish();
        recorded
    }

    /// Close new-work admission and hand back the value that may publish one
    /// whole prepared generation.
    ///
    /// A package generation enters what this host serves through this call
    /// whether it is the package's first generation or its next one: it is the
    /// same protected mutation of served state, and a second, unguarded path
    /// would be the one an update-switch interleaving could reach.
    ///
    /// A refusal changes nothing: no resource is added, no binding moves, and
    /// the snapshot the caller was reading stays exactly as it was.
    pub fn begin_replacement(
        &mut self,
        prepared: &PreparedGeneration,
    ) -> Result<GenerationReplacement<'_>, ApplicationFailure> {
        match self.admission.hold(&self.data_root) {
            Ok(()) => {}
            Err(code) if code == ADMISSION_BLOCKED => {
                return Err(
                    refusal("package_generation_work_unsettled", GENERATION_STAGE)
                        .with_field("packageId")
                        .with_presentation_arg("package", prepared.package_id.as_str())
                        .with_recovery(RecoveryAction::RetryAfterRecovery),
                );
            }
            Err(code) if code == ADMISSION_CLOSED => {
                return Err(
                    refusal("package_generation_admission_closed", GENERATION_STAGE)
                        .with_field("packageId")
                        .with_presentation_arg("package", prepared.package_id.as_str())
                        .with_recovery(RecoveryAction::RetryAfterRecovery),
                );
            }
            Err(_) => {
                return Err(
                    refusal("package_generation_admission_unavailable", GENERATION_STAGE)
                        .with_field("packageId")
                        .with_presentation_arg("package", prepared.package_id.as_str()),
                );
            }
        }
        Ok(GenerationReplacement {
            host: self,
            prepared: prepared.clone(),
            decided: false,
        })
    }

    /// Replace what one package serves with one whole prepared generation.
    ///
    /// A resource identity is namespaced, so one identity belongs to one
    /// publisher: a generation that publishes an identity another package was
    /// serving takes it over rather than merging the two, which keeps the
    /// resolution of one kind a single fact.
    fn publish(&mut self, prepared: &PreparedGeneration) -> ReplacementOutcome {
        let previous_revision = self.active.revision;
        let replaced_generation = self.generations.get(&prepared.package_id).copied();

        let removed: Vec<String> = self
            .available
            .values()
            .filter(|resource| resource.package_id == prepared.package_id)
            .map(|resource| resource.resource_id().to_owned())
            .collect();
        for resource_id in &removed {
            self.available.remove(resource_id);
        }
        for declaration in &prepared.resources {
            self.available.insert(
                declaration.id().to_owned(),
                AvailableResource {
                    package_id: prepared.package_id.clone(),
                    version: prepared.version.clone(),
                    package_generation: prepared.package_generation,
                    declaration: declaration.clone(),
                },
            );
        }
        self.generations
            .insert(prepared.package_id.clone(), prepared.package_generation);

        let mut fallbacks = Vec::new();
        for kind in ResourceKind::ALL {
            let Some(selected) = self.selection.get(&kind).cloned() else {
                continue;
            };
            if !removed.contains(&selected) || self.available.contains_key(&selected) {
                continue;
            }
            let fallback = ResourceFallback {
                kind,
                resource_id: selected,
                package_id: prepared.package_id.clone(),
                reason: FallbackReason::Replaced,
            };
            self.fallbacks.insert(kind, fallback.clone());
            fallbacks.push(fallback);
        }

        let revision = self.republish();
        ReplacementOutcome {
            package_id: prepared.package_id.clone(),
            package_generation: prepared.package_generation,
            replaced_generation,
            fallbacks,
            previous_revision,
            revision,
            plan_published: self.document_failure.is_none(),
            admission_released: false,
        }
    }

    /// Build one new snapshot and exchange it in a single step.
    ///
    /// Exchanging the snapshot publishes it: the document the client reads is
    /// written here, from the revision being exchanged, so a served revision and
    /// the published document are one fact rather than two that can drift. A
    /// publication that fails is recorded and reported by the caller that moved
    /// the revision, never hidden.
    fn republish(&mut self) -> u64 {
        self.revision += 1;
        self.active = Arc::new(resolve(
            self.revision,
            &self.available,
            &self.selection,
            &self.fallbacks,
        ));
        let publication = self.publish_mount_plan(&HOST_ACTIONS);
        self.document_failure = publication.err();
        self.revision
    }
}

/// Resolve every kind against what is available, what the user selected, and
/// which fallbacks were recorded.
///
/// A selection that is not available, or that names another kind, is not a
/// binding: the kind serves its declared default. The selection itself is left
/// alone, which is what lets a reinstall restore the user's choice.
fn resolve(
    revision: u64,
    available: &BTreeMap<String, AvailableResource>,
    selection: &BTreeMap<ResourceKind, String>,
    recorded: &BTreeMap<ResourceKind, ResourceFallback>,
) -> ResourceBindings {
    let mut bindings = BTreeMap::new();
    let mut fallbacks = BTreeMap::new();
    for kind in ResourceKind::ALL {
        match selection.get(&kind).and_then(|id| available.get(id)) {
            Some(resource) if resource.kind() == kind => {
                bindings.insert(
                    kind,
                    ResourceBinding::Selected {
                        resource_id: resource.resource_id().to_owned(),
                        package_id: resource.package_id.clone(),
                        version: resource.version.clone(),
                        package_generation: resource.package_generation,
                    },
                );
            }
            _ => {
                bindings.insert(
                    kind,
                    ResourceBinding::Default {
                        system: declared_default(kind),
                    },
                );
                if let Some(fallback) = recorded.get(&kind) {
                    fallbacks.insert(kind, fallback.clone());
                }
            }
        }
    }
    ResourceBindings {
        revision,
        bindings,
        fallbacks,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use licoup_extension_contracts::manifest::{
        FONT_RESOURCE_FORMAT, LANGUAGE_RESOURCE_FORMAT, THEME_RESOURCE_FORMAT,
    };
    use std::sync::Mutex;

    const PKG: &str = "org.licoland.appearance.synthetic";
    const THEME_A: &str = "org.licoland.theme.aurora";
    const THEME_B: &str = "org.licoland.theme.dusk";
    const FONT_A: &str = "org.licoland.font.inter";
    const LANGUAGE_A: &str = "org.licoland.language.zh";

    /// An admission answer a test decides, recording every call it receives.
    ///
    /// It is the seam's whole surface, so a test can hold one answer while an
    /// ordinary selection runs and change it before a switch without any other
    /// part of the host moving.
    struct SyntheticAdmission {
        answer: Mutex<&'static str>,
        calls: Mutex<Vec<&'static str>>,
    }

    impl SyntheticAdmission {
        /// An empty answer means the switch may begin.
        fn answering(answer: &'static str) -> Arc<Self> {
            Arc::new(Self {
                answer: Mutex::new(answer),
                calls: Mutex::new(Vec::new()),
            })
        }

        fn set(&self, answer: &'static str) {
            *self.answer.lock().expect("answer") = answer;
        }

        fn calls(&self) -> Vec<&'static str> {
            self.calls.lock().expect("calls").clone()
        }
    }

    impl MaintenanceAdmission for SyntheticAdmission {
        fn hold(&self, _data_root: &Path) -> Result<(), &'static str> {
            self.calls.lock().expect("calls").push("hold");
            let answer = *self.answer.lock().expect("answer");
            if answer.is_empty() {
                Ok(())
            } else {
                Err(answer)
            }
        }

        fn release(&self, _data_root: &Path) -> Result<(), &'static str> {
            self.calls.lock().expect("calls").push("release");
            Ok(())
        }
    }

    /// A synthetic root. Nothing is written to it: it is the key one question is
    /// asked with, and the bindings live in the host.
    fn root(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "licoup-resource-lifecycle-{tag}-{}-{}",
            std::process::id(),
            crate::platform::extension_packages::unique_suffix()
        ))
    }

    fn fixture(tag: &str, admission: Arc<SyntheticAdmission>) -> (PathBuf, ResourceHost) {
        let root = root(tag);
        let host = ResourceHost::with_admission(root.clone(), admission);
        (root, host)
    }

    fn theme(id: &str) -> ResourceDeclaration {
        ResourceDeclaration::Theme {
            id: id.to_owned(),
            definition: "theme.json".to_owned(),
            format: THEME_RESOURCE_FORMAT.to_owned(),
            tokens: vec!["org.licoland.token.surface".to_owned()],
        }
    }

    fn font(id: &str) -> ResourceDeclaration {
        ResourceDeclaration::Font {
            id: id.to_owned(),
            definition: "font.json".to_owned(),
            format: FONT_RESOURCE_FORMAT.to_owned(),
            families: vec!["Inter".to_owned()],
        }
    }

    fn language(id: &str) -> ResourceDeclaration {
        ResourceDeclaration::Language {
            id: id.to_owned(),
            definition: "language.json".to_owned(),
            format: LANGUAGE_RESOURCE_FORMAT.to_owned(),
            locales: vec!["zh-CN".to_owned()],
        }
    }

    fn generation(
        package_id: &str,
        version: &str,
        package_generation: u64,
        resources: Vec<ResourceDeclaration>,
    ) -> PreparedGeneration {
        PreparedGeneration::new(package_id, version, package_generation, resources)
            .expect("a valid prepared generation")
    }

    /// Publish one generation through the guarded path with admission held.
    fn adopt(host: &mut ResourceHost, prepared: &PreparedGeneration) -> ReplacementOutcome {
        host.begin_replacement(prepared)
            .expect("admission held")
            .commit()
    }

    #[test]
    fn every_kind_declares_the_system_rendering_as_its_default() {
        assert_eq!(
            declared_default(ResourceKind::Theme),
            SystemDefault::Appearance
        );
        assert_eq!(
            declared_default(ResourceKind::Layout),
            SystemDefault::Appearance
        );
        assert_eq!(
            declared_default(ResourceKind::Style),
            SystemDefault::Appearance
        );
        assert_eq!(
            declared_default(ResourceKind::Composition),
            SystemDefault::Appearance
        );
        assert_eq!(declared_default(ResourceKind::Font), SystemDefault::Font);
        assert_eq!(
            declared_default(ResourceKind::Language),
            SystemDefault::Locale
        );
        assert_eq!(SystemDefault::Font.as_str(), "system-font");
        assert_eq!(SystemDefault::Locale.as_str(), "system-locale");

        let (_root, host) = fixture("defaults", SyntheticAdmission::answering(""));
        for kind in ResourceKind::ALL {
            let bindings = host.bindings();
            assert_eq!(
                bindings.binding(kind),
                &ResourceBinding::Default {
                    system: declared_default(kind)
                },
                "{kind:?} serves the declared default before anything is published"
            );
            assert!(bindings.fallback(kind).is_none());
            assert!(bindings.binding(kind).is_default());
            assert_eq!(bindings.binding(kind).package_generation(), None);
        }
        assert_eq!(host.bindings().revision(), 0);
    }

    #[test]
    fn an_ordinary_selection_does_not_ask_the_admission_question() {
        let admission = SyntheticAdmission::answering("");
        let (_root, mut host) = fixture("selection", Arc::clone(&admission));
        adopt(
            &mut host,
            &generation(
                PKG,
                "1.0.0",
                1,
                vec![theme(THEME_A), font(FONT_A), language(LANGUAGE_A)],
            ),
        );
        assert_eq!(admission.calls(), vec!["hold", "release"]);

        // This host now owns unfinished admitted work: every generation switch
        // is refused, and moving a preference among what is already installed
        // is not a generation switch.
        admission.set(ADMISSION_BLOCKED);
        let selected = host
            .select(ResourceKind::Font, FONT_A)
            .expect("an ordinary selection is available during unrelated work");
        assert_eq!(selected.binding.resource_id(), Some(FONT_A));
        assert_eq!(selected.kind, ResourceKind::Font);
        assert_eq!(
            admission.calls(),
            vec!["hold", "release"],
            "a selection asks the admission question zero times"
        );

        let chosen_default = host.select_default(ResourceKind::Language);
        assert_eq!(
            chosen_default.binding,
            ResourceBinding::Default {
                system: SystemDefault::Locale
            }
        );
        assert_eq!(host.selection(ResourceKind::Language), None);
        assert!(host.bindings().fallback(ResourceKind::Language).is_none());
        assert_eq!(admission.calls(), vec!["hold", "release"]);
    }

    #[test]
    fn a_switch_holds_admission_before_it_publishes_and_releases_after() {
        let admission = SyntheticAdmission::answering("");
        let (_root, mut host) = fixture("switch-order", Arc::clone(&admission));
        let prepared = generation(PKG, "1.0.0", 1, vec![theme(THEME_A)]);

        let before = host.bindings();
        assert!(host.available().next().is_none());
        let replacement = host
            .begin_replacement(&prepared)
            .expect("admission held for the switch");
        assert_eq!(
            admission.calls(),
            vec!["hold"],
            "admission closes before anything may be published"
        );
        assert_eq!(before.revision(), 0, "nothing was published at hold time");

        let outcome = replacement.commit();
        assert_eq!(outcome.package_id, PKG);
        assert_eq!(outcome.package_generation, 1);
        assert_eq!(outcome.replaced_generation, None);
        assert_eq!(outcome.previous_revision, 0);
        assert_eq!(outcome.revision, 1);
        assert!(outcome.admission_released);
        assert_eq!(admission.calls(), vec!["hold", "release"]);
        assert_eq!(host.bindings().revision(), 1);
        assert_eq!(host.served_generation(PKG), Some(1));
    }

    #[test]
    fn an_abandoned_switch_releases_admission_and_publishes_nothing() {
        let admission = SyntheticAdmission::answering("");
        let (_root, mut host) = fixture("abort", Arc::clone(&admission));
        let prepared = generation(PKG, "1.0.0", 1, vec![theme(THEME_A)]);

        let before = host.bindings();
        let replacement = host.begin_replacement(&prepared).expect("held");
        let release = replacement.abort();
        assert!(release.released);
        assert_eq!(host.bindings(), before);
        assert_eq!(host.bindings().revision(), before.revision());
        assert!(host.available().next().is_none());
        assert_eq!(admission.calls(), vec!["hold", "release"]);

        // A caller that forgets to decide has aborted: dropping the value must
        // not leave the host's new-work admission closed by accident.
        let replacement = host.begin_replacement(&prepared).expect("held again");
        drop(replacement);
        assert_eq!(
            admission.calls(),
            vec!["hold", "release", "hold", "release"]
        );
        assert_eq!(host.bindings(), before);
    }

    #[test]
    fn unfinished_work_refuses_the_switch_and_leaves_the_previous_generation_intact() {
        let admission = SyntheticAdmission::answering("");
        let (_root, mut host) = fixture("refused-switch", Arc::clone(&admission));
        let first = generation(PKG, "1.0.0", 1, vec![theme(THEME_A), font(FONT_A)]);
        adopt(&mut host, &first);
        host.select(ResourceKind::Theme, THEME_A).expect("selected");
        let before = host.bindings();
        let available_before = host.available().count();

        admission.set(ADMISSION_BLOCKED);
        let next = generation(PKG, "2.0.0", 2, vec![theme(THEME_B)]);
        let failure = host
            .begin_replacement(&next)
            .expect_err("unfinished admitted work refuses the switch");
        assert_eq!(failure.code, "package_generation_work_unsettled");
        assert_eq!(failure.recovery, RecoveryAction::RetryAfterRecovery);
        assert_eq!(failure.presentation_args.get("package"), Some(PKG));

        let after = host.bindings();
        assert_eq!(after, before, "a refused switch republishes nothing");
        assert_eq!(after.revision(), before.revision());
        assert_eq!(
            after.binding(ResourceKind::Theme).resource_id(),
            Some(THEME_A)
        );
        assert_eq!(
            after.binding(ResourceKind::Theme).package_generation(),
            Some(1)
        );
        assert!(after.serves_package_generation(PKG, 1));
        assert!(!after.serves_package_generation(PKG, 2));
        assert_eq!(host.served_generation(PKG), Some(1));
        assert_eq!(host.available().count(), available_before);
        assert_eq!(
            admission.calls(),
            vec!["hold", "release", "hold"],
            "a refused switch holds nothing: there is nothing to release"
        );
    }

    #[test]
    fn a_closed_or_unanswered_seam_refuses_the_switch() {
        let admission = SyntheticAdmission::answering(ADMISSION_CLOSED);
        let (_root, mut host) = fixture("closed", Arc::clone(&admission));
        let prepared = generation(PKG, "1.0.0", 1, vec![theme(THEME_A)]);
        let failure = host
            .begin_replacement(&prepared)
            .expect_err("another maintenance switch holds the barrier");
        assert_eq!(failure.code, "package_generation_admission_closed");
        assert_eq!(host.bindings().revision(), 0);

        let admission = SyntheticAdmission::answering(ADMISSION_UNAVAILABLE);
        let (_root, mut host) = fixture("unavailable", Arc::clone(&admission));
        let failure = host
            .begin_replacement(&prepared)
            .expect_err("no answer is not an idle answer");
        assert_eq!(failure.code, "package_generation_admission_unavailable");
        assert_eq!(host.bindings().revision(), 0);
        assert!(host.available().next().is_none());
        assert_eq!(
            admission.calls(),
            vec!["hold"],
            "an unavailable answer releases nothing it never held"
        );
    }

    #[test]
    fn the_seam_fails_closed_when_nothing_answers_it() {
        // The port a production host opens with, before any composition
        // installs an answer.
        let port = MaintenanceAdmissionPort::new();
        assert!(!port.is_installed());
        assert_eq!(
            port.hold(Path::new("/nonexistent/data-root")),
            Err(ADMISSION_UNAVAILABLE)
        );
        assert_eq!(
            port.release(Path::new("/nonexistent/data-root")),
            Err(ADMISSION_UNAVAILABLE)
        );

        // One answer per port: installing a second is refused, not silently
        // substituted.
        let installed = SyntheticAdmission::answering("");
        port.install(installed).expect("first installation");
        assert!(port.is_installed());
        assert_eq!(port.hold(Path::new("/nonexistent/data-root")), Ok(()));
        assert_eq!(
            port.install(SyntheticAdmission::answering(ADMISSION_BLOCKED)),
            Err("maintenance admission is already installed")
        );

        // And a host bound to the fail-closed answer publishes nothing, however
        // well formed the generation it was offered.
        let admission = SyntheticAdmission::answering(ADMISSION_UNAVAILABLE);
        let (_root, mut host) = fixture("fail-closed", Arc::clone(&admission));
        let prepared = generation(PKG, "1.0.0", 1, vec![theme(THEME_A)]);
        let failure = host
            .begin_replacement(&prepared)
            .expect_err("no decision owner, no switch");
        assert_eq!(failure.code, "package_generation_admission_unavailable");
        assert_eq!(host.served_generation(PKG), None);
        assert_eq!(host.bindings().revision(), 0);
    }

    #[test]
    fn a_replacement_serves_one_generation_and_never_two() {
        let admission = SyntheticAdmission::answering("");
        let (_root, mut host) = fixture("one-generation", Arc::clone(&admission));
        adopt(
            &mut host,
            &generation(PKG, "1.0.0", 1, vec![theme(THEME_A), font(FONT_A)]),
        );
        host.select(ResourceKind::Theme, THEME_A).expect("selected");
        host.select(ResourceKind::Font, FONT_A).expect("selected");

        let outcome = adopt(
            &mut host,
            &generation(PKG, "2.0.0", 2, vec![theme(THEME_B), font(FONT_A)]),
        );
        assert_eq!(outcome.replaced_generation, Some(1));
        assert_eq!(outcome.previous_revision, outcome.revision - 1);

        let bindings = host.bindings();
        assert!(
            !bindings.serves_package_generation(PKG, 1),
            "the replaced generation is not served beside its successor"
        );
        assert!(bindings.serves_package_generation(PKG, 2));
        assert_eq!(host.served_generation(PKG), Some(2));

        // A resource the next generation still carries keeps serving, from the
        // new generation; one it dropped falls back and is recorded as replaced.
        assert_eq!(
            bindings.binding(ResourceKind::Font).resource_id(),
            Some(FONT_A)
        );
        assert_eq!(
            bindings.binding(ResourceKind::Font).package_generation(),
            Some(2)
        );
        assert!(bindings.binding(ResourceKind::Theme).is_default());
        assert_eq!(
            bindings.fallback(ResourceKind::Theme).map(|it| it.reason),
            Some(FallbackReason::Replaced)
        );
        assert_eq!(outcome.fallbacks.len(), 1);
        assert_eq!(
            host.selection(ResourceKind::Theme),
            Some(THEME_A),
            "the preference is kept even though it is not served"
        );
    }

    #[test]
    fn a_disabled_package_falls_back_to_the_declared_defaults_and_reports_it() {
        let admission = SyntheticAdmission::answering("");
        let (_root, mut host) = fixture("disabled", Arc::clone(&admission));
        adopt(
            &mut host,
            &generation(
                PKG,
                "1.0.0",
                1,
                vec![theme(THEME_A), font(FONT_A), language(LANGUAGE_A)],
            ),
        );
        host.select(ResourceKind::Theme, THEME_A).expect("selected");
        host.select(ResourceKind::Font, FONT_A).expect("selected");
        host.select(ResourceKind::Language, LANGUAGE_A)
            .expect("selected");
        let calls_before = admission.calls();

        let recorded = host.withdraw(PKG, ResourceChange::Disabled);
        assert_eq!(recorded.len(), 3);
        let bindings = host.bindings();
        for (kind, system) in [
            (ResourceKind::Theme, SystemDefault::Appearance),
            (ResourceKind::Font, SystemDefault::Font),
            (ResourceKind::Language, SystemDefault::Locale),
        ] {
            assert_eq!(
                bindings.binding(kind),
                &ResourceBinding::Default { system },
                "{kind:?} serves the declared default, not a half-applied binding"
            );
            let fallback = bindings.fallback(kind).expect("recorded");
            assert_eq!(fallback.reason, FallbackReason::Disabled);
            assert_eq!(fallback.package_id, PKG);
            assert_eq!(fallback.kind, kind);
        }
        assert_eq!(
            bindings
                .fallback(ResourceKind::Font)
                .map(|it| it.resource_id.as_str()),
            Some(FONT_A)
        );
        assert_eq!(host.served_generation(PKG), None);
        assert_eq!(host.available().count(), 0);
        for kind in [
            ResourceKind::Theme,
            ResourceKind::Font,
            ResourceKind::Language,
        ] {
            assert!(
                host.selection(kind).is_some(),
                "a fallback is not the user changing their mind"
            );
        }
        assert_eq!(
            admission.calls(),
            calls_before,
            "a fallback is a release, not an update: it asks no admission question"
        );
    }

    #[test]
    fn an_uninstalled_resource_falls_back_and_the_preference_survives_reinstall() {
        let admission = SyntheticAdmission::answering("");
        let (_root, mut host) = fixture("reinstall", Arc::clone(&admission));
        adopt(&mut host, &generation(PKG, "1.0.0", 1, vec![font(FONT_A)]));
        host.select(ResourceKind::Font, FONT_A).expect("selected");

        let recorded = host.withdraw(PKG, ResourceChange::Uninstalled);
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].reason, FallbackReason::Uninstalled);
        assert!(host.bindings().binding(ResourceKind::Font).is_default());
        assert_eq!(host.selection(ResourceKind::Font), Some(FONT_A));

        // Reinstalling restores the user's choice instead of silently resetting
        // it to the system font, and the stale fallback is cleared with it.
        adopt(&mut host, &generation(PKG, "1.0.0", 3, vec![font(FONT_A)]));
        let bindings = host.bindings();
        assert_eq!(
            bindings.binding(ResourceKind::Font).resource_id(),
            Some(FONT_A)
        );
        assert_eq!(
            bindings.binding(ResourceKind::Font).package_generation(),
            Some(3)
        );
        assert!(bindings.fallback(ResourceKind::Font).is_none());

        // Withdrawing a package this host does not hold changes nothing.
        let revision = bindings.revision();
        assert!(
            host.withdraw("org.licoland.other", ResourceChange::Uninstalled)
                .is_empty()
        );
        assert_eq!(host.bindings().revision(), revision);
    }

    #[test]
    fn a_selection_of_something_this_host_does_not_hold_is_refused() {
        let admission = SyntheticAdmission::answering("");
        let (_root, mut host) = fixture("selection-refusal", Arc::clone(&admission));
        adopt(
            &mut host,
            &generation(PKG, "1.0.0", 1, vec![theme(THEME_A), font(FONT_A)]),
        );
        let revision = host.bindings().revision();

        let failure = host
            .select(ResourceKind::Theme, "org.licoland.theme.absent")
            .expect_err("not installed");
        assert_eq!(failure.code, "package_resource_not_installed");
        assert_eq!(failure.recovery, RecoveryAction::InstallOrRetryRuntime);

        let failure = host
            .select(ResourceKind::Font, THEME_A)
            .expect_err("a theme is not a font");
        assert_eq!(failure.code, "package_resource_kind_mismatch");
        assert_eq!(failure.presentation_args.get("actualKind"), Some("theme"));

        assert_eq!(host.bindings().revision(), revision);
        assert!(host.bindings().binding(ResourceKind::Font).is_default());
    }

    #[test]
    fn a_generation_that_cannot_be_served_is_refused_before_any_switch() {
        for (prepared, field) in [
            (
                PreparedGeneration::new("not namespaced", "1.0.0", 1, Vec::new()),
                "packageId",
            ),
            (PreparedGeneration::new(PKG, "1", 1, Vec::new()), "version"),
            (
                PreparedGeneration::new(PKG, "1.0.0", 0, Vec::new()),
                "packageGeneration",
            ),
            (
                PreparedGeneration::new(PKG, "1.0.0", 1, vec![theme(THEME_A), theme(THEME_A)]),
                "resources.id",
            ),
        ] {
            let failure = prepared.expect_err("refused before it could switch anything");
            assert_eq!(
                failure.code, "package_generation_identity_invalid",
                "{field}"
            );
            assert_eq!(failure.field.as_deref(), Some(field));
        }

        // A declaration the contract cannot type is refused by the contract's
        // own check rather than by a second, weaker one here.
        let shape = PreparedGeneration::new(
            PKG,
            "1.0.0",
            1,
            vec![ResourceDeclaration::Theme {
                id: THEME_A.to_owned(),
                definition: "theme.json".to_owned(),
                format: FONT_RESOURCE_FORMAT.to_owned(),
                tokens: vec!["org.licoland.token.surface".to_owned()],
            }],
        )
        .expect_err("a theme that claims to be a font");
        assert_eq!(shape.code, "data_package_resource_shape_unknown");

        let oversized = vec![theme(THEME_A); MAX_GENERATION_RESOURCES + 1];
        assert_eq!(
            PreparedGeneration::new(PKG, "1.0.0", 1, oversized)
                .expect_err("unbounded declaration")
                .field
                .as_deref(),
            Some("resources")
        );
    }
}
