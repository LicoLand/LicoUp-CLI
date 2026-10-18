//! C12: packages, the install closure, and which capability belongs where.
//!
//! Three separate things are decided here, and confusing them is how a "small
//! core" turns back into one large bundle:
//!
//! 1. **Ownership.** Every capability has an owner: the kernel, or a specific
//!    package a user may leave out ([`CAPABILITY_OWNERSHIP`]). The kernel is the
//!    installed client itself — canonical conversation, the Assistant, the
//!    workflow and flywheel, the extension host with package management, the base
//!    usage journal, the declarative interface primitives, the generic PTY/CLI
//!    adapter with Agent Hub configuration registration, key custody, and update
//!    and migration admission. It is a complete product without any optional
//!    package, and it depends on none of them.
//! 2. **The install closure.** `requires` decides what is installed with a
//!    package; `optionalRequires` is a statement that extra capability becomes
//!    available *when the user already has it*, and never an instruction to
//!    install anything ([`install_closure`]). This relation is a deployment
//!    relation and nothing else: it does not enter a development task graph.
//! 3. **Availability.** Available, installed, enabled and active are four
//!    independent facts, and a capability that is missing is reported as an
//!    unavailable capability with a real next step — not as a malformed request,
//!    and not as a failure of the client ([`CapabilityAvailability`]).
//!
//! Nothing here reaches the network. A package the user built, a directory on
//! this machine, an official directory and a third-party mirror are all just
//! sources ([`PackageSource`]), and no resolution needs an account. A hosted
//! catalog is a convenience, never a prerequisite.

use crate::manifest::Dependency;
use crate::refusal;
use licoup_application::ApplicationFailure;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

const STAGE: &str = "extension/deployment";

/// The minimal trusted host. Its capabilities are the ones a distribution cannot
/// remove, because removing them removes the client.
pub const CORE_PACKAGE: &str = "org.licoland.core";

/// A capability the host itself publishes: the primitives contributions bind to.
/// It has no optional package because the primitives are compiled into the
/// client.
pub const DECLARATIVE_UI_CAPABILITY: &str = "declarative-ui.v1";

/// The continuous Assistant. It is the product's collaboration loop rather than
/// a feature a user installs, so it belongs to the kernel.
pub const ASSISTANT_CAPABILITY: &str = "assistant.v1";

/// The workflow and flywheel: durable multi-step work over the conversation.
/// Like the Assistant, it is the kernel's, not an optional package's.
pub const WORKFLOW_CAPABILITY: &str = "workflow.v1";

/// Which set of packages carries one capability.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PackOwnership {
    /// The minimal core carries it, so every distribution has it.
    Core(&'static str),
    /// A package the user may leave out. Its absence is a fact about this
    /// installation, not a defect.
    Optional(&'static str),
}

impl PackOwnership {
    /// The package that carries the capability.
    pub const fn package(self) -> &'static str {
        match self {
            Self::Core(package) | Self::Optional(package) => package,
        }
    }

    /// Whether the capability survives every profile choice.
    pub const fn is_core(self) -> bool {
        matches!(self, Self::Core(_))
    }
}

/// Per-capability ownership: the kernel, or one optional package.
///
/// The same capability may be provided by more than one package — three adapters
/// all provide `agent-execution.v1` — and the default distribution names the one a
/// user gets without choosing. Ownership answers "which package must be present
/// for this to work at all", which is a different question from "which packages
/// offer it".
pub const CAPABILITY_OWNERSHIP: [(&str, PackOwnership); 13] = [
    ("conversation.v1", PackOwnership::Core(CORE_PACKAGE)),
    (ASSISTANT_CAPABILITY, PackOwnership::Core(CORE_PACKAGE)),
    (WORKFLOW_CAPABILITY, PackOwnership::Core(CORE_PACKAGE)),
    ("extension-host.v1", PackOwnership::Core(CORE_PACKAGE)),
    ("usage-journal.v1", PackOwnership::Core(CORE_PACKAGE)),
    (DECLARATIVE_UI_CAPABILITY, PackOwnership::Core(CORE_PACKAGE)),
    (
        "agent-execution.v1",
        PackOwnership::Optional("org.licoland.adapter.generic"),
    ),
    (
        "model-provider.v1",
        PackOwnership::Optional("org.licoland.provider.compat"),
    ),
    (
        "model-gateway.v1",
        PackOwnership::Optional("org.licoland.feature.gateway"),
    ),
    (
        "analytics.v1",
        PackOwnership::Optional("org.licoland.feature.analytics"),
    ),
    (
        "endpoint-collaboration.v1",
        PackOwnership::Optional("org.licoland.feature.collaboration"),
    ),
    (
        "mcp-server.v1",
        PackOwnership::Optional("org.licoland.feature.mcp"),
    ),
    (
        "channel-connector.v1",
        PackOwnership::Optional("org.licoland.channel.telegram"),
    ),
];

/// The owner of one capability, or `None` when it is not part of the default
/// distribution at all.
pub fn capability_owner(capability: &str) -> Option<PackOwnership> {
    CAPABILITY_OWNERSHIP
        .iter()
        .find(|(name, _)| *name == capability)
        .map(|(_, ownership)| *ownership)
}

/// Capabilities that must be present in every distribution.
pub fn core_capabilities() -> impl Iterator<Item = &'static str> {
    CAPABILITY_OWNERSHIP
        .iter()
        .filter(|(_, ownership)| ownership.is_core())
        .map(|(capability, _)| *capability)
}

/// Capabilities that arrive with an optional package.
pub fn optional_capabilities() -> impl Iterator<Item = &'static str> {
    CAPABILITY_OWNERSHIP
        .iter()
        .filter(|(_, ownership)| !ownership.is_core())
        .map(|(capability, _)| *capability)
}

/// The packages the default distribution is assembled from: the kernel first,
/// then the first-party packages that carry the optional capabilities, in table
/// order and without repetition.
///
/// Every package named here carries a self-described compatibility list
/// ([`crate::manifest::Compatibility`]), and the kernel's place in this set is
/// not a dependency: no optional package may force the kernel to load it, and the
/// kernel may not require any of them.
pub fn default_packages() -> impl Iterator<Item = &'static str> {
    let mut packages = vec![CORE_PACKAGE];
    let mut seen: BTreeSet<&'static str> = packages.iter().copied().collect();
    for (_, ownership) in CAPABILITY_OWNERSHIP {
        let package = ownership.package();
        if seen.insert(package) {
            packages.push(package);
        }
    }
    packages.into_iter()
}

/// Where a package came from.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PackageSource {
    /// A package the user built or received and imported directly.
    LocalImport,
    /// A directory on this machine.
    LocalDirectory,
    /// The official directory.
    OfficialDirectory,
    /// A third-party or enterprise mirror.
    ThirdPartyDirectory,
}

impl PackageSource {
    pub const fn id(self) -> &'static str {
        match self {
            Self::LocalImport => "local-import",
            Self::LocalDirectory => "local-directory",
            Self::OfficialDirectory => "official-directory",
            Self::ThirdPartyDirectory => "third-party-directory",
        }
    }

    /// Whether resolving through this source needs the network.
    pub const fn requires_network(self) -> bool {
        matches!(self, Self::OfficialDirectory | Self::ThirdPartyDirectory)
    }

    /// Whether resolving through this source needs an account.
    ///
    /// It never does. Signing in may unlock a directory, and the client works
    /// without one: importing a package the user already has is a first-class
    /// path, and being unpublished is not being illegal.
    pub const fn requires_account(self) -> bool {
        false
    }

    pub const fn is_local(self) -> bool {
        !self.requires_network()
    }
}

/// One package as a local catalogue knows it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PackageEntry {
    pub id: String,
    pub version: String,
    pub source: PackageSource,
    /// Installed together with this package.
    pub requires: Vec<Dependency>,
    /// Enabled when already present, never installed on this package's account.
    pub optional_requires: Vec<Dependency>,
}

impl PackageEntry {
    pub fn new(id: impl Into<String>, version: impl Into<String>, source: PackageSource) -> Self {
        Self {
            id: id.into(),
            version: version.into(),
            source,
            requires: Vec::new(),
            optional_requires: Vec::new(),
        }
    }

    pub fn requiring(mut self, requires: impl IntoIterator<Item = Dependency>) -> Self {
        self.requires = requires.into_iter().collect();
        self
    }

    pub fn optionally_requiring(
        mut self,
        optional_requires: impl IntoIterator<Item = Dependency>,
    ) -> Self {
        self.optional_requires = optional_requires.into_iter().collect();
        self
    }
}

/// The packages this machine can already see.
///
/// This is the *local* view: what is on disk, imported, or cached. It is not a
/// directory service, it holds no download state, and every operation on it is a
/// lookup.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LocalCatalogue {
    entries: BTreeMap<String, PackageEntry>,
}

impl LocalCatalogue {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, entry: PackageEntry) -> &mut Self {
        self.entries.insert(entry.id.clone(), entry);
        self
    }

    pub fn get(&self, package_id: &str) -> Option<&PackageEntry> {
        self.entries.get(package_id)
    }

    pub fn contains(&self, package_id: &str) -> bool {
        self.entries.contains_key(package_id)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }
}

/// What an installation would end up containing.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct InstallClosure {
    selected: BTreeSet<String>,
    declined_optional: BTreeSet<String>,
}

impl InstallClosure {
    /// Every package that would be installed, roots and their `requires`
    /// closure.
    pub fn selected(&self) -> impl Iterator<Item = &str> {
        self.selected.iter().map(String::as_str)
    }

    pub fn contains(&self, package_id: &str) -> bool {
        self.selected.contains(package_id)
    }

    pub fn len(&self) -> usize {
        self.selected.len()
    }

    pub fn is_empty(&self) -> bool {
        self.selected.is_empty()
    }

    /// Optional dependencies that were declared and deliberately not installed.
    /// They are reported so a user can decide, not resolved silently.
    pub fn declined_optional(&self) -> impl Iterator<Item = &str> {
        self.declined_optional.iter().map(String::as_str)
    }
}

/// Compute the install closure of `roots`.
///
/// Only `requires` is followed. An unknown package — a root that is not in the
/// catalogue, or a required dependency that is not — refuses the plan with the
/// dependent named, because "this package needs something you do not have" is a
/// fact the user can act on and "invalid request" is not. A cycle among
/// `requires` is refused too: there is no order in which such a set can be
/// installed.
pub fn install_closure(
    catalogue: &LocalCatalogue,
    roots: &[&str],
) -> Result<InstallClosure, ApplicationFailure> {
    check_core_dependencies(catalogue)?;
    enum Frame {
        Enter(String),
        Exit(String),
    }

    let mut selected: BTreeSet<String> = BTreeSet::new();
    let mut declined: BTreeSet<String> = BTreeSet::new();
    let mut visiting: BTreeSet<String> = BTreeSet::new();
    let mut done: BTreeSet<String> = BTreeSet::new();

    for root in roots {
        if !catalogue.contains(root) {
            return Err(missing_package(root, None));
        }
        let mut stack = vec![Frame::Enter((*root).to_owned())];
        while let Some(frame) = stack.pop() {
            match frame {
                Frame::Enter(package_id) => {
                    if done.contains(&package_id) {
                        continue;
                    }
                    visiting.insert(package_id.clone());
                    stack.push(Frame::Exit(package_id.clone()));
                    let entry = catalogue
                        .get(&package_id)
                        .ok_or_else(|| missing_package(&package_id, None))?;
                    for dependency in entry.requires.iter().rev() {
                        if visiting.contains(&dependency.package_id) {
                            return Err(refusal::new("install_dependency_cycle", STAGE)
                                .with_field("requires")
                                .with_presentation_arg("package", &dependency.package_id));
                        }
                        if done.contains(&dependency.package_id) {
                            continue;
                        }
                        if !catalogue.contains(&dependency.package_id) {
                            return Err(missing_package(&dependency.package_id, Some(&package_id)));
                        }
                        stack.push(Frame::Enter(dependency.package_id.clone()));
                    }
                    for dependency in &entry.optional_requires {
                        declined.insert(dependency.package_id.clone());
                    }
                }
                Frame::Exit(package_id) => {
                    visiting.remove(&package_id);
                    done.insert(package_id.clone());
                    selected.insert(package_id);
                }
            }
        }
    }

    declined.retain(|package_id| !selected.contains(package_id));
    Ok(InstallClosure {
        selected,
        declined_optional: declined,
    })
}

fn missing_package(package_id: &str, dependent: Option<&str>) -> ApplicationFailure {
    let failure = refusal::actionable("install_package_unavailable", STAGE, "packageId")
        .with_presentation_arg("package", package_id);
    match dependent {
        Some(dependent) if dependent.len() <= 96 => {
            failure.with_presentation_arg("requiredBy", dependent)
        }
        _ => failure,
    }
}

/// Refuse a core package that depends on something removable.
///
/// The minimal core depends on the base capabilities and on nothing that can be
/// trimmed away. The reverse direction is the allowed one: an optional package
/// depends on the core, and no optional package can force the core to load it.
pub fn check_core_dependencies(catalogue: &LocalCatalogue) -> Result<(), ApplicationFailure> {
    let Some(core) = catalogue.get(CORE_PACKAGE) else {
        return Ok(());
    };
    if let Some(dependency) = core.requires.first() {
        return Err(refusal::new("core_requires_optional_package", STAGE)
            .with_field("requires")
            .with_presentation_arg("package", &dependency.package_id));
    }
    Ok(())
}

/// How far along an installed package is.
///
/// `Verified` (a publisher signature was checked against a trusted update
/// mechanism) and `LocalApproved` (the user bound trust to specific content) are
/// two different trust facts, and neither is "more installed" than the other.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PackageLifecycle {
    /// Known from directory metadata only. Nothing has been fetched.
    Available,
    Downloaded,
    Verified,
    LocalApproved,
    Staged,
    Installed,
}

impl PackageLifecycle {
    pub const fn is_installed(self) -> bool {
        matches!(self, Self::Installed)
    }

    pub const fn is_local_trust(self) -> bool {
        matches!(self, Self::LocalApproved)
    }
}

/// How far along one running instance of a package is.
///
/// One package version may produce several instances with different permissions,
/// so `packageVersion`, `instanceId`, `generation` and the registry epoch are
/// four different facts and never one version.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum InstanceLifecycle {
    Discovered,
    Preparing,
    Active,
    Draining,
    Stopped,
    Failed,
    Quarantined,
}

/// The four facts a package has, kept apart on purpose.
///
/// There is no single `ready`, because the interesting states are the mixed ones:
/// a package that is installed and disabled, a package that is available and not
/// installed at all, a package the user imported locally that was never in any
/// directory. Collapsing them into one token is how "not installed" starts being
/// drawn as a green checkmark.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageFacts {
    /// Present in a directory. A local import is installed without ever being
    /// available, which is why this does not follow from `installed`.
    pub available: bool,
    pub installed: bool,
    pub enabled: bool,
    pub active: bool,
}

impl PackageFacts {
    /// The facts a locally imported, installed and enabled package has.
    pub const fn local_import(enabled: bool) -> Self {
        Self {
            available: false,
            installed: true,
            enabled,
            active: false,
        }
    }

    /// Refuse a combination the package manager could not have produced.
    ///
    /// Only the forward implications are checked: an active instance needs an
    /// enabled package, which needs an installed one. Availability is not part of
    /// that chain.
    pub fn validate(&self) -> Result<(), ApplicationFailure> {
        if (self.active && !self.enabled) || (self.enabled && !self.installed) {
            return Err(refusal::new("package_facts_inconsistent", STAGE)
                .with_field("enabled")
                .with_presentation_arg("active", if self.active { "true" } else { "false" })
                .with_presentation_arg(
                    "installed",
                    if self.installed { "true" } else { "false" },
                ));
        }
        Ok(())
    }
}

/// Whether a capability can be served right now.
///
/// Every state other than `Served` is a legal configuration and a fact about the
/// catalogue, not an exception: a client with a minimal distribution is
/// answering honestly, and no client fails to build because a pairing package is
/// not installed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CapabilityAvailability {
    /// The owning package is enabled, so the capability can be served. Activation
    /// stays lazy: being enabled is not being started.
    Served,
    /// Installed but disabled by the user.
    InstalledNotEnabled,
    /// Not installed at all.
    NotInstalled,
    /// Not part of the default distribution.
    NotInDistribution,
}

impl CapabilityAvailability {
    pub const fn is_served(self) -> bool {
        matches!(self, Self::Served)
    }

    /// A stable short description for a client surface. It is a catalogue fact,
    /// so a client shows it where capabilities are listed rather than as an error
    /// banner.
    pub const fn describe(self) -> &'static str {
        match self {
            Self::Served => "served",
            Self::InstalledNotEnabled => "installed-not-enabled",
            Self::NotInstalled => "not-installed",
            Self::NotInDistribution => "not-in-distribution",
        }
    }

    /// The refusal for an operation that needs this capability, or `None` when it
    /// can be served.
    ///
    /// It is an unavailable capability, not a bad request: the code names the
    /// capability, and the recovery points at installing or enabling the package
    /// that carries it.
    pub fn refusal(self, capability: &str) -> Option<ApplicationFailure> {
        if self.is_served() {
            return None;
        }
        let failure = refusal::actionable("capability_unavailable", STAGE, "capability")
            .with_presentation_arg("state", self.describe());
        Some(failure.with_presentation_arg("capability", capability))
    }
}

/// Decide a capability against the facts of the package that owns it.
pub fn availability(capability: &str, facts: PackageFacts) -> CapabilityAvailability {
    let Some(_owner) = capability_owner(capability) else {
        return CapabilityAvailability::NotInDistribution;
    };
    if !facts.installed {
        return CapabilityAvailability::NotInstalled;
    }
    if !facts.enabled {
        return CapabilityAvailability::InstalledNotEnabled;
    }
    CapabilityAvailability::Served
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalogue() -> LocalCatalogue {
        let mut catalogue = LocalCatalogue::new();
        catalogue.insert(PackageEntry::new(
            CORE_PACKAGE,
            "0.3.0",
            PackageSource::OfficialDirectory,
        ));
        catalogue.insert(
            PackageEntry::new(
                "org.licoland.adapter.generic",
                "1.0.0",
                PackageSource::OfficialDirectory,
            )
            .requiring([Dependency::new(CORE_PACKAGE, "^0.3")])
            .optionally_requiring([Dependency::new("org.licoland.feature.analytics", "^1")]),
        );
        catalogue.insert(
            PackageEntry::new(
                "org.licoland.feature.analytics",
                "1.0.0",
                PackageSource::ThirdPartyDirectory,
            )
            .requiring([Dependency::new(CORE_PACKAGE, "^0.3")]),
        );
        catalogue
    }

    #[test]
    fn the_core_carries_only_untrimmable_capabilities() {
        let core: Vec<&str> = core_capabilities().collect();
        assert_eq!(core.len(), 6);
        assert!(core.contains(&"extension-host.v1"));
        assert!(core.contains(&DECLARATIVE_UI_CAPABILITY));
        assert!(!core.contains(&"model-gateway.v1"));
        assert!(optional_capabilities().any(|id| id == "endpoint-collaboration.v1"));
        assert_eq!(capability_owner("nobody.knows.v1"), None);
    }

    #[test]
    fn the_assistant_and_the_workflow_stay_in_the_kernel() {
        for capability in [ASSISTANT_CAPABILITY, WORKFLOW_CAPABILITY] {
            assert_eq!(
                capability_owner(capability),
                Some(PackOwnership::Core(CORE_PACKAGE)),
                "{capability} is part of the installed client, not a package to install"
            );
            assert!(
                core_capabilities().any(|id| id == capability),
                "{capability} must survive every profile choice"
            );
            assert!(
                !optional_capabilities().any(|id| id == capability),
                "{capability} may not be left out"
            );
        }
        assert_eq!(
            capability_owner(WORKFLOW_CAPABILITY),
            Some(PackOwnership::Core(CORE_PACKAGE)),
            "the workflow and flywheel are the kernel's"
        );
    }

    #[test]
    fn an_optional_capability_is_never_a_kernel_dependency() {
        // Every optional capability is carried by a package the user may leave
        // out, and the kernel never names one of them.
        for capability in optional_capabilities() {
            let ownership = capability_owner(capability).expect("every capability has an owner");
            assert!(!ownership.is_core(), "{capability} must stay optional");
            assert_ne!(
                ownership.package(),
                CORE_PACKAGE,
                "{capability} may not be owned by the kernel"
            );
            assert!(
                !core_capabilities().any(|id| id == capability),
                "{capability} may not be a kernel capability as well"
            );
        }

        let mut catalogue = catalogue();
        assert!(check_core_dependencies(&catalogue).is_ok());
        for capability in optional_capabilities() {
            let owner = capability_owner(capability).expect("owner").package();
            let mut with_dependency = catalogue.get(CORE_PACKAGE).cloned().expect("core");
            with_dependency.requires = vec![Dependency::new(owner, "^1")];
            catalogue.insert(with_dependency);
            let failure = check_core_dependencies(&catalogue).expect_err("kernel must not depend");
            assert_eq!(failure.code, "core_requires_optional_package");
        }
    }

    #[test]
    fn the_default_package_set_is_the_kernel_and_its_first_party_packages() {
        let packages: Vec<&str> = default_packages().collect();
        assert_eq!(packages.first(), Some(&CORE_PACKAGE));
        assert_eq!(
            packages.len(),
            1 + optional_capabilities().count(),
            "one package per optional capability, plus the kernel"
        );
        let unique: BTreeSet<&str> = packages.iter().copied().collect();
        assert_eq!(unique.len(), packages.len(), "no package is named twice");
        assert_eq!(packages[1], "org.licoland.adapter.generic");

        // The kernel is complete without any of them: a distribution that has
        // only the core still serves every core capability.
        let mut catalogue = LocalCatalogue::new();
        catalogue.insert(PackageEntry::new(
            CORE_PACKAGE,
            "0.3.0",
            PackageSource::OfficialDirectory,
        ));
        let closure = install_closure(&catalogue, &[CORE_PACKAGE]).expect("kernel alone");
        assert_eq!(closure.len(), 1);
        assert!(closure.declined_optional().next().is_none());
        for capability in core_capabilities() {
            assert_eq!(
                availability(capability, PackageFacts::local_import(true)).describe(),
                "served",
                "{capability} is served by the kernel alone"
            );
        }
    }

    #[test]
    fn optional_dependencies_are_reported_and_never_installed_on_someone_elses_account() {
        let closure =
            install_closure(&catalogue(), &["org.licoland.adapter.generic"]).expect("plan");
        let selected: BTreeSet<&str> = closure.selected().collect();
        assert_eq!(
            selected,
            BTreeSet::from([CORE_PACKAGE, "org.licoland.adapter.generic"])
        );
        assert!(!closure.contains("org.licoland.feature.analytics"));
        assert_eq!(
            closure.declined_optional().collect::<Vec<_>>(),
            vec!["org.licoland.feature.analytics"]
        );
    }

    #[test]
    fn an_optional_dependency_that_is_also_required_is_installed_once() {
        let mut catalogue = catalogue();
        let mut adapter = catalogue
            .get("org.licoland.adapter.generic")
            .cloned()
            .expect("adapter");
        adapter
            .requires
            .push(Dependency::new("org.licoland.feature.analytics", "^1"));
        catalogue.insert(adapter);

        let closure = install_closure(
            &catalogue,
            &[
                "org.licoland.adapter.generic",
                "org.licoland.feature.analytics",
            ],
        )
        .expect("plan");
        assert!(closure.contains("org.licoland.feature.analytics"));
        assert!(closure.declined_optional().next().is_none());
        assert_eq!(closure.len(), 3);
    }

    #[test]
    fn a_missing_required_package_is_actionable_and_names_the_dependent() {
        let mut catalogue = catalogue();
        catalogue.insert(
            PackageEntry::new(
                "example.specialist.echo",
                "1.0.0",
                PackageSource::LocalImport,
            )
            .requiring([Dependency::new("example.something.absent", "^1")]),
        );
        let failure =
            install_closure(&catalogue, &["example.specialist.echo"]).expect_err("missing");
        assert_eq!(failure.code, "install_package_unavailable");
        assert_eq!(failure.field.as_deref(), Some("packageId"));

        let failure = install_closure(&catalogue, &["example.absent"]).expect_err("unknown root");
        assert_eq!(failure.code, "install_package_unavailable");
    }

    #[test]
    fn a_cycle_is_refused_rather_than_half_installed() {
        let mut catalogue = LocalCatalogue::new();
        catalogue.insert(
            PackageEntry::new("example.a", "1.0.0", PackageSource::LocalImport)
                .requiring([Dependency::new("example.b", "^1")]),
        );
        catalogue.insert(
            PackageEntry::new("example.b", "1.0.0", PackageSource::LocalImport)
                .requiring([Dependency::new("example.a", "^1")]),
        );
        let failure = install_closure(&catalogue, &["example.a"]).expect_err("cycle");
        assert_eq!(failure.code, "install_dependency_cycle");
    }

    #[test]
    fn a_diamond_closure_is_visited_once_and_is_not_a_cycle() {
        let mut catalogue = LocalCatalogue::new();
        catalogue.insert(
            PackageEntry::new("example.top", "1.0.0", PackageSource::LocalImport).requiring([
                Dependency::new("example.left", "^1"),
                Dependency::new("example.right", "^1"),
            ]),
        );
        catalogue.insert(
            PackageEntry::new("example.left", "1.0.0", PackageSource::LocalImport)
                .requiring([Dependency::new("example.leaf", "^1")]),
        );
        catalogue.insert(
            PackageEntry::new("example.right", "1.0.0", PackageSource::LocalImport)
                .requiring([Dependency::new("example.leaf", "^1")]),
        );
        catalogue.insert(PackageEntry::new(
            "example.leaf",
            "1.0.0",
            PackageSource::LocalImport,
        ));

        let closure = install_closure(&catalogue, &["example.top"]).expect("plan");
        assert_eq!(closure.len(), 4);
    }

    #[test]
    fn local_imports_need_no_directory_and_no_account() {
        assert!(!PackageSource::LocalImport.requires_network());
        assert!(!PackageSource::LocalImport.requires_account());
        assert!(PackageSource::LocalImport.is_local());
        assert!(PackageSource::OfficialDirectory.requires_network());
        assert!(!PackageSource::OfficialDirectory.requires_account());

        // A package that was never in any directory still installs and runs.
        let mut catalogue = LocalCatalogue::new();
        catalogue.insert(
            PackageEntry::new(
                "example.specialist.echo",
                "1.0.0",
                PackageSource::LocalImport,
            )
            .requiring([Dependency::new(CORE_PACKAGE, "^0.3")]),
        );
        catalogue.insert(PackageEntry::new(
            CORE_PACKAGE,
            "0.3.0",
            PackageSource::LocalImport,
        ));
        let closure = install_closure(&catalogue, &["example.specialist.echo"]).expect("plan");
        assert_eq!(closure.len(), 2);
    }

    #[test]
    fn core_may_not_require_something_removable() {
        let mut catalogue = catalogue();
        assert!(check_core_dependencies(&catalogue).is_ok());

        let mut with_dependency = catalogue.get(CORE_PACKAGE).cloned().expect("core");
        with_dependency
            .requires
            .push(Dependency::new("org.licoland.feature.gateway", "^1"));
        catalogue.insert(with_dependency);
        let failure = check_core_dependencies(&catalogue).expect_err("core must not depend");
        assert_eq!(failure.code, "core_requires_optional_package");
    }

    #[test]
    fn the_four_package_facts_are_independent() {
        let imported = PackageFacts::local_import(true);
        assert!(imported.validate().is_ok());
        assert!(
            !imported.available,
            "a local import was never in a directory"
        );

        let impossible = PackageFacts {
            active: true,
            enabled: false,
            installed: false,
            available: true,
        };
        assert_eq!(
            impossible
                .validate()
                .expect_err("active without enabled")
                .code,
            "package_facts_inconsistent"
        );

        let available_only = PackageFacts {
            available: true,
            ..PackageFacts::default()
        };
        assert!(available_only.validate().is_ok());
    }

    #[test]
    fn a_missing_capability_is_a_catalogue_fact_with_a_next_step() {
        let facts = PackageFacts {
            available: true,
            installed: true,
            enabled: false,
            active: false,
        };
        let state = availability("model-gateway.v1", facts);
        assert_eq!(state, CapabilityAvailability::InstalledNotEnabled);
        let failure = state.refusal("model-gateway.v1").expect("refusal");
        assert_eq!(failure.code, "capability_unavailable");
        assert_eq!(
            failure.recovery,
            licoup_application::RecoveryAction::InstallOrRetryRuntime
        );
        assert_eq!(
            failure.presentation_args.get("state"),
            Some(state.describe())
        );

        assert_eq!(
            availability("nobody.knows.v1", PackageFacts::local_import(true)),
            CapabilityAvailability::NotInDistribution
        );
        assert_eq!(
            availability("analytics.v1", PackageFacts::local_import(true)),
            CapabilityAvailability::Served
        );
        assert!(
            CapabilityAvailability::Served
                .refusal("analytics.v1")
                .is_none()
        );
    }
}
