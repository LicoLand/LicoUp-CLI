//! Compatibility and availability: which package serves which Agent version,
//! and what the client remembers about it.
//!
//! Four facts are kept apart on purpose, because collapsing any two of them is
//! how "not installed" starts being drawn as a green checkmark:
//!
//! 1. **A package declares its own client compatibility list.** The list is read
//!    from the manifest the host holds ([`DeclaredPackageSource`]) and never
//!    restated here: the entries in a [`CompatibilityRow`] are the manifest's
//!    own, and the coverage decision delegates to
//!    [`licoup_extension_contracts::manifest::Compatibility::covers`], so this
//!    module cannot drift from the contract that decides it. The package's own
//!    version takes no part in that decision, which is what admits a package
//!    released on its own schedule.
//! 2. **The Agent-version interval is declared data.** One
//!    [`StrategyDeclaration`] says which first-party capability one package
//!    serves, and over which Agent versions — a half-open interval, inclusive
//!    start and exclusive end, so two adjacent rows cannot both claim the
//!    boundary version. The same capability may be declared for several packages
//!    and several intervals; the table is the whole declaration, exposed to
//!    callers as [`CompatibilityRow`] data.
//! 3. **Availability is cached by Agent version.** One record per Agent identity
//!    holds the Agent version that was observed, the capability that was asked
//!    about, the client version it was decided against, the package version that
//!    covers all three, and when that was seen
//!    ([`AvailabilityRecord`]). A request reads that record and nothing else: not
//!    a manifest, not a package directory, and nothing outside the cache
//!    ([`AvailabilityIndex::cached`]). A record decided for another Agent
//!    version is stale and is never reused; it is replaced by a fresh decision,
//!    and the Agent is named when that happens ([`AgentChange`],
//!    [`PackageOffer`]).
//! 4. **Nothing here starts an Agent.** The Agent's version is handed in by a
//!    caller that already observed it — a session handshake, a catalogue entry,
//!    a file the Agent wrote. This module has no process API at all, so
//!    "availability" can never mean "we ran it to find out": an unknown Agent
//!    stays unknown, and an unavailable capability stays unavailable.
//!
//! The offer is a recommendation, exactly like a discovery match: accepting it
//! records an install intent ([`PendingInstall`]) and installing nothing,
//! declining it records the refusal, and neither changes what the cache or the
//! installed packages say. A capability with no package behind it is reported as
//! an unavailable capability in the extension contracts' own vocabulary
//! ([`CapabilityAvailability`]), with the Agent, its version and the reason.
//!
//! This module owns the library, not a route: no FFI command, no stdio frame and
//! no bridge DTO is added here. The node that owns the host surface adds the one
//! line that uses it — `AvailabilityIndex::observe(&observation, capability,
//! client_version)` after an install, an update or a detection, and
//! `AvailabilityIndex::cached(...)` on the read path — against the owners
//! re-exported from [`super`]. Everything those calls need is already public
//! here, so wiring them changes nothing in this file.

use crate::platform::extension_packages::discovery::{PendingInstall, RecommendationLog};
use crate::platform::extension_packages::install::{PackageStore, checked_identity};
use crate::platform::extension_packages::{
    ensure_private_directory, now_unix_ms, read_bounded_text, refusal, replace_file_atomically,
};
use licoup_application::ApplicationFailure;
use licoup_extension_contracts::deployment::{
    CapabilityAvailability, PackageSource, capability_owner,
};
use licoup_extension_contracts::manifest::{Compatibility, PackageManifest};
use semver::Version;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

const COMPATIBILITY_STAGE: &str = "extension/package-compatibility";

/// The most strategy intervals one table accepts. A bound is published rather
/// than implied: a table larger than this is a catalogue, not one client's
/// declared adapter strategies.
pub const MAX_STRATEGY_DECLARATIONS: usize = 64;

/// The longest Agent identity accepted, in bytes.
pub const MAX_AGENT_IDENTITY_BYTES: usize = 64;

/// The longest version, interval bound or capability name accepted, in bytes.
pub const MAX_VERSION_BYTES: usize = 64;

/// The longest Agent display name accepted, in bytes.
pub const MAX_AGENT_NAME_BYTES: usize = 96;

/// The most installed versions one package may contribute to one table.
pub const MAX_HELD_VERSIONS: usize = 16;

/// The longest availability record the cache reads back or writes, in bytes.
pub const MAX_AVAILABILITY_RECORD_BYTES: usize = 4 * 1024;

/// The directory the availability cache occupies below the package store root.
pub const AVAILABILITY_CACHE_DIRECTORY: &str = "availability";

/// One Agent identity, as the client records it.
///
/// The identity becomes a file name below the cache root, so the accepted
/// character set is a path-safety rule and not decoration: no value admitted
/// here can be a separator, a parent directory or a platform prefix.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct AgentIdentity(String);

impl AgentIdentity {
    pub fn new(identity: impl Into<String>) -> Result<Self, ApplicationFailure> {
        let identity = identity.into();
        let safe = !identity.is_empty()
            && identity.len() <= MAX_AGENT_IDENTITY_BYTES
            && !identity.starts_with('.')
            && identity.chars().all(|character| {
                character.is_ascii_lowercase()
                    || character.is_ascii_digit()
                    || matches!(character, '.' | '-' | '_')
            });
        if !safe {
            return Err(
                refusal("agent_identity_invalid", COMPATIBILITY_STAGE).with_field("agentIdentity")
            );
        }
        Ok(Self(identity))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AgentIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// The user-facing naming of one Agent: who it is and which version was seen.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentNaming {
    pub identity: AgentIdentity,
    /// What a surface calls this Agent. The caller's own name for it, falling
    /// back to the identity when the caller has none — never a fabricated one.
    pub display_name: String,
    pub version: String,
}

/// What one caller observed about one Agent.
///
/// The version is an observation the caller already holds. This type exists so
/// the availability path can be given a version without ever obtaining one
/// itself.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentObservation {
    identity: AgentIdentity,
    version: String,
    display_name: String,
}

impl AgentObservation {
    /// An observation of one Agent at one version.
    pub fn new(
        identity: AgentIdentity,
        version: impl Into<String>,
    ) -> Result<Self, ApplicationFailure> {
        let version = version.into();
        if version.is_empty()
            || version.len() > MAX_VERSION_BYTES
            || Version::parse(&version).is_err()
        {
            return Err(
                refusal("agent_version_invalid", COMPATIBILITY_STAGE).with_field("agentVersion")
            );
        }
        let display_name = identity.as_str().to_owned();
        Ok(Self {
            identity,
            version,
            display_name,
        })
    }

    /// Name the Agent for a surface. Absent this, the identity is the name.
    pub fn with_display_name(
        mut self,
        name: impl Into<String>,
    ) -> Result<Self, ApplicationFailure> {
        let name = name.into();
        if name.is_empty()
            || name.len() > MAX_AGENT_NAME_BYTES
            || name.chars().any(char::is_control)
        {
            return Err(refusal("agent_name_invalid", COMPATIBILITY_STAGE).with_field("agentName"));
        }
        self.display_name = name;
        Ok(self)
    }

    pub fn identity(&self) -> &AgentIdentity {
        &self.identity
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    pub fn naming(&self) -> AgentNaming {
        AgentNaming {
            identity: self.identity.clone(),
            display_name: self.display_name.clone(),
            version: self.version.clone(),
        }
    }
}

/// A half-open Agent-version interval: inclusive start, exclusive end.
///
/// The exclusive end is what makes an interval table a table: the version that
/// ends one row begins the next, and no version is claimed twice.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentVersionInterval {
    /// The first Agent version this row applies to.
    pub from: String,
    /// The first Agent version it does not apply to.
    pub before: String,
}

impl AgentVersionInterval {
    pub fn new(
        from: impl Into<String>,
        before: impl Into<String>,
    ) -> Result<Self, ApplicationFailure> {
        let interval = Self {
            from: from.into(),
            before: before.into(),
        };
        interval.validate()?;
        Ok(interval)
    }

    /// Structural validation: both bounds are versions this contract can
    /// compare, and the interval is not empty.
    pub fn validate(&self) -> Result<(), ApplicationFailure> {
        let start = interval_bound(&self.from, "agentVersions.from")?;
        let end = interval_bound(&self.before, "agentVersions.before")?;
        if start >= end {
            return Err(
                refusal("agent_version_interval_invalid", COMPATIBILITY_STAGE)
                    .with_field("agentVersions")
                    .with_presentation_arg("from", self.from.as_str())
                    .with_presentation_arg("before", self.before.as_str()),
            );
        }
        Ok(())
    }

    /// Whether one Agent version is inside. A version this contract cannot parse
    /// is inside nothing, so an unreadable identity fails closed.
    pub fn contains(&self, agent_version: &Version) -> bool {
        match (
            interval_bound(&self.from, "agentVersions.from"),
            interval_bound(&self.before, "agentVersions.before"),
        ) {
            (Ok(start), Ok(end)) => *agent_version >= start && *agent_version < end,
            _ => false,
        }
    }

    /// Whether one Agent version, as text, is inside.
    pub fn contains_text(&self, agent_version: &str) -> bool {
        Version::parse(agent_version).is_ok_and(|version| self.contains(&version))
    }
}

/// One declared strategy interval: the first-party capability one package
/// serves, and over which Agent versions.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StrategyDeclaration {
    /// The capability the extension contracts declare as first-party.
    pub capability: String,
    /// The package that serves it over this interval.
    pub package_id: String,
    pub agent_versions: AgentVersionInterval,
}

impl StrategyDeclaration {
    pub fn new(
        capability: impl Into<String>,
        package_id: impl Into<String>,
        from: impl Into<String>,
        before: impl Into<String>,
    ) -> Result<Self, ApplicationFailure> {
        let declaration = Self {
            capability: capability.into(),
            package_id: package_id.into(),
            agent_versions: AgentVersionInterval {
                from: from.into(),
                before: before.into(),
            },
        };
        declaration.validate()?;
        Ok(declaration)
    }

    /// Structural validation: the capability is one the contracts declare, the
    /// package identity is namespaced, and the interval is real.
    ///
    /// The capability check is the "first-party" half of the table: a declaration
    /// for a capability nothing publishes is refused rather than stored, because
    /// a row nothing can ask for is not availability data.
    pub fn validate(&self) -> Result<(), ApplicationFailure> {
        if self.capability.is_empty() || self.capability.len() > MAX_VERSION_BYTES {
            return Err(
                refusal("compatibility_capability_unknown", COMPATIBILITY_STAGE)
                    .with_field("capability"),
            );
        }
        if capability_owner(&self.capability).is_none() {
            return Err(
                refusal("compatibility_capability_unknown", COMPATIBILITY_STAGE)
                    .with_field("capability")
                    .with_presentation_arg("capability", self.capability.as_str()),
            );
        }
        if !licoup_extension_contracts::is_namespaced(&self.package_id) {
            return Err(
                refusal("compatibility_declaration_invalid", COMPATIBILITY_STAGE)
                    .with_field("packageId"),
            );
        }
        self.agent_versions.validate()
    }
}

/// The declared interval table, validated once.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StrategyDeclarations {
    strategies: Vec<StrategyDeclaration>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DeclarationsDocument {
    strategies: Vec<StrategyDeclaration>,
}

impl StrategyDeclarations {
    /// Read a declaration table, refusing a malformed or duplicated one.
    pub fn new(
        strategies: impl IntoIterator<Item = StrategyDeclaration>,
    ) -> Result<Self, ApplicationFailure> {
        let strategies: Vec<StrategyDeclaration> = strategies.into_iter().collect();
        if strategies.len() > MAX_STRATEGY_DECLARATIONS {
            return Err(
                refusal("compatibility_declarations_too_many", COMPATIBILITY_STAGE)
                    .with_field("strategies"),
            );
        }
        let mut seen: Vec<(String, String)> = Vec::new();
        for strategy in &strategies {
            strategy.validate()?;
            let key = (strategy.capability.clone(), strategy.package_id.clone());
            if seen.contains(&key) {
                return Err(
                    refusal("compatibility_declaration_duplicated", COMPATIBILITY_STAGE)
                        .with_field("strategies")
                        .with_presentation_arg("capability", strategy.capability.as_str())
                        .with_presentation_arg("package", strategy.package_id.as_str()),
                );
            }
            seen.push(key);
        }
        Ok(Self { strategies })
    }

    /// Read the table from the JSON document a caller holds.
    pub fn from_json(text: &str) -> Result<Self, ApplicationFailure> {
        let document: DeclarationsDocument = serde_json::from_str(text).map_err(|_| {
            refusal("compatibility_declaration_invalid", COMPATIBILITY_STAGE)
                .with_field("strategies")
        })?;
        Self::new(document.strategies)
    }

    pub fn strategies(&self) -> &[StrategyDeclaration] {
        &self.strategies
    }

    pub fn is_empty(&self) -> bool {
        self.strategies.is_empty()
    }
}

/// One package version whose own manifest this host holds.
#[derive(Clone, Debug, PartialEq)]
pub struct DeclaredPackage {
    pub package_id: String,
    pub version: String,
    pub source: PackageSource,
    /// The package's own declaration, read verbatim. The table's client list
    /// comes from here and from nowhere else.
    pub manifest: PackageManifest,
}

impl DeclaredPackage {
    pub fn new(
        package_id: impl Into<String>,
        version: impl Into<String>,
        source: PackageSource,
        manifest: PackageManifest,
    ) -> Self {
        Self {
            package_id: package_id.into(),
            version: version.into(),
            source,
            manifest,
        }
    }
}

/// Where a table reads the packages' own declarations from.
///
/// The package's manifest is the only source of its compatibility list. A host
/// that restated the list in its own table would be answering its own question,
/// and a package released independently of the client could then be described by
/// a client that never read it.
pub trait DeclaredPackageSource {
    /// Every version of one package this host holds a manifest for, in any order.
    ///
    /// An unknown package is an empty list rather than a failure: not being
    /// installed is a fact about this installation, not a defect.
    fn declarations(&self, package_id: &str) -> Result<Vec<DeclaredPackage>, ApplicationFailure>;
}

impl DeclaredPackageSource for PackageStore {
    fn declarations(&self, package_id: &str) -> Result<Vec<DeclaredPackage>, ApplicationFailure> {
        let mut held = Vec::new();
        for installed in self.installed()? {
            if installed.package_id != package_id {
                continue;
            }
            if held.len() >= MAX_HELD_VERSIONS {
                break;
            }
            let manifest = self.installed_manifest(&installed.package_id, &installed.version)?;
            held.push(DeclaredPackage {
                package_id: installed.package_id,
                version: installed.version,
                source: installed.source,
                manifest,
            });
        }
        Ok(held)
    }
}

/// A declaration source over manifests a caller already holds.
///
/// It is how a caller answers the table without a package store — a catalogue
/// entry it read, a synthetic root, a package it is about to install — while
/// keeping the rule that the client reads a package's compatibility list from
/// the manifest rather than from a second copy.
#[derive(Clone, Debug, Default)]
pub struct ManifestSet {
    held: BTreeMap<String, Vec<DeclaredPackage>>,
}

impl ManifestSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(
        mut self,
        package_id: impl Into<String>,
        version: impl Into<String>,
        source: PackageSource,
        manifest: PackageManifest,
    ) -> Self {
        let package_id = package_id.into();
        self.held
            .entry(package_id.clone())
            .or_default()
            .push(DeclaredPackage::new(package_id, version, source, manifest));
        self
    }

    pub fn is_empty(&self) -> bool {
        self.held.is_empty()
    }

    /// How many manifests this set holds for one package. Zero is "this host
    /// holds none", which is the only thing it can honestly say.
    pub fn len_for(&self, package_id: &str) -> usize {
        self.held.get(package_id).map_or(0, Vec::len)
    }
}

impl DeclaredPackageSource for ManifestSet {
    fn declarations(&self, package_id: &str) -> Result<Vec<DeclaredPackage>, ApplicationFailure> {
        Ok(self.held.get(package_id).cloned().unwrap_or_default())
    }
}

/// One row of the compatibility table: a declared first-party capability, the
/// package that serves it, the client versions that package declares, and the
/// Agent versions it applies to.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompatibilityRow {
    pub capability: String,
    pub package_id: String,
    pub package_version: String,
    /// The package's own name for itself, from its manifest.
    pub display_name: String,
    /// The client versions the package itself declares, verbatim. Read from the
    /// manifest, never restated by the client.
    ///
    /// It is flattened into the row, so the row carries the manifest's own
    /// `clientVersions` list as a list rather than as a nested object.
    #[serde(flatten)]
    declared_client_versions: Compatibility,
    pub agent_versions: AgentVersionInterval,
    /// Whether this package is the one the default distribution names for the
    /// capability. A second adapter a user installed is a row too, and is not
    /// this.
    pub default_owner: bool,
    pub source: PackageSource,
}

impl CompatibilityRow {
    /// The client-versions list as the package declared it.
    pub fn declared_client_versions(&self) -> &[String] {
        &self.declared_client_versions.client_versions
    }

    /// Whether this package's own list covers one client version.
    ///
    /// The decision delegates to the manifest contract's `covers`, so the table
    /// cannot hold a second, quietly different answer to the same question.
    pub fn covers_client(&self, client_version: &str) -> bool {
        self.declared_client_versions.covers(client_version)
    }
}

/// The declared strategy intervals joined with the packages' own manifests.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CompatibilityTable {
    declarations: Vec<StrategyDeclaration>,
    rows: Vec<CompatibilityRow>,
}

impl CompatibilityTable {
    /// Build the table from the declared intervals and the manifests this host
    /// holds.
    ///
    /// A declaration whose package this host holds no manifest for contributes
    /// no row: the client cannot read a compatibility list it does not have, and
    /// inventing one is the fabrication this module exists to prevent. That the
    /// package is missing is reported by [`CompatibilityTable::explain`] instead.
    pub fn build<S: DeclaredPackageSource + ?Sized>(
        declarations: &StrategyDeclarations,
        packages: &S,
    ) -> Result<Self, ApplicationFailure> {
        let mut rows = Vec::new();
        for declaration in declarations.strategies() {
            declaration.validate()?;
            let mut held = packages.declarations(&declaration.package_id)?;
            held.truncate(MAX_HELD_VERSIONS);
            for package in held {
                checked_identity(&package.package_id, &package.version)?;
                package.manifest.validate()?;
                if package.manifest.id != package.package_id
                    || package.manifest.version != package.version
                {
                    return Err(
                        refusal("compatibility_manifest_mismatch", COMPATIBILITY_STAGE)
                            .with_field("manifest")
                            .with_presentation_arg("package", package.package_id.as_str())
                            .with_presentation_arg("version", package.version.as_str()),
                    );
                }
                rows.push(CompatibilityRow {
                    capability: declaration.capability.clone(),
                    package_id: package.package_id.clone(),
                    package_version: package.version.clone(),
                    display_name: package.manifest.display_name.clone(),
                    declared_client_versions: package.manifest.compatibility.clone(),
                    agent_versions: declaration.agent_versions.clone(),
                    default_owner: capability_owner(&declaration.capability)
                        .is_some_and(|ownership| ownership.package() == package.package_id),
                    source: package.source,
                });
            }
        }
        rows.sort_by(|left, right| {
            left.capability
                .cmp(&right.capability)
                .then_with(|| left.package_id.cmp(&right.package_id))
                .then_with(|| {
                    row_version(left.package_version.as_str())
                        .cmp(&row_version(right.package_version.as_str()))
                })
        });
        Ok(Self {
            declarations: declarations.strategies().to_vec(),
            rows,
        })
    }

    /// The declared intervals this table was built from.
    pub fn declarations(&self) -> &[StrategyDeclaration] {
        &self.declarations
    }

    /// Every row, ordered by capability, package and package version.
    pub fn rows(&self) -> &[CompatibilityRow] {
        &self.rows
    }

    pub fn rows_for(&self, capability: &str) -> impl Iterator<Item = &CompatibilityRow> {
        self.rows
            .iter()
            .filter(move |row| row.capability == capability)
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// The declaration whose interval contains one Agent version, if the table
    /// declares one for that capability.
    pub fn declared_for(
        &self,
        capability: &str,
        agent_version: &str,
    ) -> Option<&StrategyDeclaration> {
        let version = Version::parse(agent_version).ok()?;
        self.declarations.iter().find(|declaration| {
            declaration.capability == capability && declaration.agent_versions.contains(&version)
        })
    }

    /// The package version that serves one capability for one Agent version on
    /// one client.
    ///
    /// The highest covering package version wins, deterministically: rows are
    /// ordered by package and version, so two installed versions of the same
    /// package resolve to the later one rather than to whichever the filesystem
    /// listed first.
    pub fn covering(
        &self,
        capability: &str,
        agent_version: &str,
        client_version: &str,
    ) -> Option<&CompatibilityRow> {
        let version = Version::parse(agent_version).ok()?;
        self.rows
            .iter()
            .filter(|row| {
                row.capability == capability
                    && row.agent_versions.contains(&version)
                    && row.covers_client(client_version)
            })
            .max_by_key(|row| row_version(row.package_version.as_str()))
    }

    /// Why nothing covers one Agent version, in the order the client decided it.
    pub fn explain(&self, capability: &str, agent_version: &str) -> UnavailableReason {
        if capability_owner(capability).is_none() {
            return UnavailableReason::CapabilityUnknown;
        }
        let Ok(version) = Version::parse(agent_version) else {
            return UnavailableReason::AgentVersionUnreadable;
        };
        let declared: Vec<&StrategyDeclaration> = self
            .declarations
            .iter()
            .filter(|declaration| {
                declaration.capability == capability
                    && declaration.agent_versions.contains(&version)
            })
            .collect();
        if declared.is_empty() {
            return UnavailableReason::NoDeclaredInterval;
        }
        let held: Vec<String> = declared
            .iter()
            .filter(|declaration| {
                self.rows.iter().any(|row| {
                    row.capability == capability && row.package_id == declaration.package_id
                })
            })
            .map(|declaration| declaration.package_id.clone())
            .collect();
        if held.is_empty() {
            return UnavailableReason::PackageNotInstalled {
                packages: declared
                    .iter()
                    .map(|declaration| declaration.package_id.clone())
                    .collect(),
            };
        }
        UnavailableReason::ClientNotCovered { packages: held }
    }
}

/// Why no package serves one Agent version.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UnavailableReason {
    /// The capability is not one the extension contracts declare as first-party.
    CapabilityUnknown,
    /// The observed Agent version is not a version this client can compare.
    AgentVersionUnreadable,
    /// No declared interval contains this Agent version.
    NoDeclaredInterval,
    /// An interval contains it, and no package version for it is on this host.
    PackageNotInstalled { packages: Vec<String> },
    /// A package is installed, and its own list does not cover this client.
    ClientNotCovered { packages: Vec<String> },
}

impl UnavailableReason {
    /// The stable short code a client reports this reason with.
    pub const fn code(&self) -> &'static str {
        match self {
            Self::CapabilityUnknown => "capability_not_declared",
            Self::AgentVersionUnreadable => "agent_version_unreadable",
            Self::NoDeclaredInterval => "no_declared_interval",
            Self::PackageNotInstalled { .. } => "package_not_installed",
            Self::ClientNotCovered { .. } => "client_not_covered",
        }
    }

    /// The catalogue fact a surface shows for this reason.
    ///
    /// Nothing here is a served capability, and nothing here is an exception: a
    /// client with a minimal distribution answers honestly.
    pub const fn availability(&self) -> CapabilityAvailability {
        match self {
            Self::CapabilityUnknown | Self::NoDeclaredInterval => {
                CapabilityAvailability::NotInDistribution
            }
            Self::AgentVersionUnreadable
            | Self::PackageNotInstalled { .. }
            | Self::ClientNotCovered { .. } => CapabilityAvailability::NotInstalled,
        }
    }

    /// The packages this reason names, when it names any.
    pub fn packages(&self) -> &[String] {
        match self {
            Self::PackageNotInstalled { packages } | Self::ClientNotCovered { packages } => {
                packages
            }
            _ => &[],
        }
    }
}

/// One Agent capability no package answers, with the Agent named.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Unavailable {
    pub agent: AgentNaming,
    pub capability: String,
    pub reason: UnavailableReason,
}

impl Unavailable {
    /// The refusal for an operation that needs this capability.
    ///
    /// It is the extension contracts' own unavailable-capability failure, with
    /// the Agent named: the capability is missing, the request was not malformed,
    /// and the next step is to obtain a package that covers this client.
    ///
    /// A failure publishes at most four presentation arguments, and the
    /// contracts' own failure already spends two of them on the catalogue state
    /// and the capability. The Agent therefore arrives as the two facts a surface
    /// shows — its name and the version that made it unavailable — and the reason
    /// stays on [`Unavailable::reason`], where a caller reads it as a code rather
    /// than as display text.
    pub fn refusal(&self) -> ApplicationFailure {
        let availability = self.reason.availability();
        availability
            .refusal(&self.capability)
            .unwrap_or_else(|| {
                refusal("capability_unavailable", COMPATIBILITY_STAGE).with_field("capability")
            })
            .with_presentation_arg("agent", self.agent.display_name.as_str())
            .with_presentation_arg("agentVersion", self.agent.version.as_str())
    }

    /// The catalogue fact for this capability right now.
    pub const fn availability(&self) -> CapabilityAvailability {
        self.reason.availability()
    }
}

/// The package offered because the Agent's version changed.
///
/// An offer is a recommendation, never an install: the user still sees the
/// package, its source and its permissions before anything is fetched.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PackageOffer {
    pub agent: AgentNaming,
    pub capability: String,
    pub package_id: String,
    pub package_version: String,
    /// The package's own name for itself, from its manifest.
    pub display_name: String,
    pub source: PackageSource,
    /// Always true: an offer is not an install.
    pub requires_user_confirmation: bool,
}

impl PackageOffer {
    /// The user chose this package: record the intent, install nothing.
    pub fn accept(&self, log: &mut RecommendationLog) -> PendingInstall {
        log.record_acceptance(&self.package_id);
        PendingInstall {
            package_id: self.package_id.clone(),
            version: self.package_version.clone(),
            source: self.source,
            requires_user_confirmation: true,
        }
    }

    /// The user declined: the capability stays unavailable.
    ///
    /// Nothing about the installation changes, no other package is substituted,
    /// and the refusal is recorded in the log a surface reads before offering
    /// again. That is the whole meaning of "declining leaves it unavailable":
    /// the offer is a recommendation, so refusing it decides the offer and not
    /// the capability.
    pub fn decline(&self, log: &mut RecommendationLog, reason: &str) {
        log.record_decline(&self.package_id, reason);
    }
}

/// What one observation of an Agent decided.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Availability {
    /// The cache already held a mapping for exactly this Agent version,
    /// capability and client. Nothing was recomputed and nothing was written.
    Cached(AvailabilityRecord),
    /// The mapping was decided now. `previous` is what this identity's cache
    /// held beforehand, so a caller can explain the change; `None` means this
    /// Agent was never recorded.
    Changed(Box<AgentChange>),
    /// No package serves this Agent version on this client.
    Unavailable(Unavailable),
}

/// The decision that replaced a stale or absent mapping.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentChange {
    pub agent: AgentNaming,
    pub previous: Option<AvailabilityRecord>,
    pub record: AvailabilityRecord,
    pub offer: PackageOffer,
}

impl AgentChange {
    /// Whether the Agent changed version, rather than being seen for the first
    /// time.
    pub fn is_version_change(&self) -> bool {
        self.previous
            .as_ref()
            .is_some_and(|previous| previous.agent_version != self.record.agent_version)
    }

    /// The version this Agent was last seen at, when it was seen before.
    pub fn previous_version(&self) -> Option<&str> {
        self.previous
            .as_ref()
            .map(|previous| previous.agent_version.as_str())
    }
}

/// One observation of which package serves one Agent capability on this client.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AvailabilityRecord {
    pub agent_identity: String,
    /// The Agent version this record was decided for. A different observed
    /// version makes it stale.
    pub agent_version: String,
    /// The capability that was asked about. A different question is a new
    /// decision, never a reuse.
    pub capability: String,
    /// The client version it was decided against. The same package may cover one
    /// client and not the next, so a client change is staleness too.
    pub client_version: String,
    pub package_id: String,
    pub package_version: String,
    pub observed_at_unix_ms: i64,
}

impl AvailabilityRecord {
    /// Whether this record answers exactly this question.
    pub fn is_current_for(
        &self,
        agent_version: &str,
        capability: &str,
        client_version: &str,
    ) -> bool {
        self.agent_version == agent_version
            && self.capability == capability
            && self.client_version == client_version
    }
}

/// What the cache holds for one request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CachedAvailability {
    /// Decided for exactly this Agent version, capability and client.
    Fresh(AvailabilityRecord),
    /// A record for this Agent exists, and it was decided for another version,
    /// capability or client. It is not reused: the mapping it holds describes a
    /// different world.
    Stale { cached: AvailabilityRecord },
    /// Nothing was ever recorded for this identity.
    Unknown,
}

/// The durable, version-keyed availability cache.
///
/// One record per Agent identity, below the package store's own root, so
/// relocating the store relocates its cache with it. A request reads one bounded
/// file and stops: no manifest, no package directory and nothing outside the
/// cache.
///
/// A record is derived data. The store's own GC may reclaim the cache directory
/// with it, and a record that is gone reads as [`CachedAvailability::Unknown`]
/// until the next observation says otherwise — never as a mapping this client
/// no longer holds.
#[derive(Clone, Debug)]
pub struct AvailabilityCache {
    root: PathBuf,
}

impl AvailabilityCache {
    /// Open the cache below one package store.
    pub fn open(store: &PackageStore) -> Result<Self, ApplicationFailure> {
        Self::open_at(
            &store
                .root()
                .join("cache")
                .join(AVAILABILITY_CACHE_DIRECTORY),
        )
    }

    /// Open the cache at an explicit root, for a caller that owns its own layout
    /// and for tests over synthetic roots.
    pub fn open_at(root: &Path) -> Result<Self, ApplicationFailure> {
        ensure_private_directory(root)?;
        Ok(Self {
            root: root.to_path_buf(),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Where one Agent's record lives.
    pub fn record_path(&self, identity: &AgentIdentity) -> PathBuf {
        self.root.join(format!("{identity}.json"))
    }

    /// Read the record for one Agent and decide whether it answers this request.
    pub fn read(
        &self,
        identity: &AgentIdentity,
        agent_version: &str,
        capability: &str,
        client_version: &str,
    ) -> Result<CachedAvailability, ApplicationFailure> {
        let path = self.record_path(identity);
        let Some(text) = read_bounded_text(&path, MAX_AVAILABILITY_RECORD_BYTES)? else {
            return Ok(CachedAvailability::Unknown);
        };
        let record: AvailabilityRecord = serde_json::from_str(&text).map_err(|_| {
            refusal("availability_record_invalid", COMPATIBILITY_STAGE).with_field("record")
        })?;
        if record.agent_identity != identity.as_str() {
            // A record that lost its identity is not this Agent's answer.
            return Err(refusal("availability_record_invalid", COMPATIBILITY_STAGE)
                .with_field("agentIdentity")
                .with_presentation_arg("agentIdentity", identity.as_str()));
        }
        if record.is_current_for(agent_version, capability, client_version) {
            Ok(CachedAvailability::Fresh(record))
        } else {
            Ok(CachedAvailability::Stale { cached: record })
        }
    }

    /// Write one record, replacing whatever this identity held.
    ///
    /// The write is atomic, so a reader sees the previous record or this one and
    /// never half of either.
    pub fn write(&self, record: &AvailabilityRecord) -> Result<(), ApplicationFailure> {
        let identity = AgentIdentity::new(record.agent_identity.clone())?;
        let text = serde_json::to_string_pretty(record).map_err(|_| {
            refusal("availability_record_invalid", COMPATIBILITY_STAGE).with_field("record")
        })?;
        if text.len() > MAX_AVAILABILITY_RECORD_BYTES {
            return Err(
                refusal("availability_record_too_large", COMPATIBILITY_STAGE).with_field("record"),
            );
        }
        replace_file_atomically(&self.record_path(&identity), &text)
    }

    /// Forget one Agent's record, reporting whether there was one.
    pub fn invalidate(&self, identity: &AgentIdentity) -> Result<bool, ApplicationFailure> {
        match std::fs::remove_file(self.record_path(identity)) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(_) => Err(
                refusal("availability_cache_unavailable", COMPATIBILITY_STAGE)
                    .with_field("agentIdentity"),
            ),
        }
    }
}

/// The availability surface: the declared intervals, the manifests they are
/// joined with, and the durable cache.
///
/// Two entry points, and the split is the design:
///
/// - [`AvailabilityIndex::cached`] is the request path. It reads the cache and
///   nothing else, so a surface can ask about an Agent without a manifest read
///   and without a process.
/// - [`AvailabilityIndex::observe`] is the recording path, called after an
///   install, an update or a detection. It decides from the packages' own
///   manifests, writes the record, and names the Agent when the version changed.
#[derive(Clone, Debug)]
pub struct AvailabilityIndex<S> {
    cache: AvailabilityCache,
    declarations: StrategyDeclarations,
    packages: S,
}

impl<S: DeclaredPackageSource> AvailabilityIndex<S> {
    pub fn new(cache: AvailabilityCache, declarations: StrategyDeclarations, packages: S) -> Self {
        Self {
            cache,
            declarations,
            packages,
        }
    }

    pub fn cache(&self) -> &AvailabilityCache {
        &self.cache
    }

    pub fn declarations(&self) -> &StrategyDeclarations {
        &self.declarations
    }

    pub fn packages(&self) -> &S {
        &self.packages
    }

    /// The table this index decides from, built from the manifests the source
    /// holds.
    pub fn table(&self) -> Result<CompatibilityTable, ApplicationFailure> {
        CompatibilityTable::build(&self.declarations, &self.packages)
    }

    /// The request path: answer from the cache only.
    pub fn cached(
        &self,
        observation: &AgentObservation,
        capability: &str,
        client_version: &str,
    ) -> Result<CachedAvailability, ApplicationFailure> {
        self.cache.read(
            &observation.identity,
            &observation.version,
            capability,
            client_version,
        )
    }

    /// Record one observation of which package serves this Agent's capability.
    ///
    /// A record that answers this exact question is returned as it stands. A
    /// record decided for another Agent version, capability or client is stale:
    /// the mapping is decided again from the manifests, the record is replaced,
    /// and the returned [`AgentChange`] names the Agent and offers the package.
    /// Nothing that cannot be decided is written, so an unknown Agent stays
    /// unknown rather than cached as unavailable.
    pub fn observe(
        &self,
        observation: &AgentObservation,
        capability: &str,
        client_version: &str,
    ) -> Result<Availability, ApplicationFailure> {
        let previous = match self.cached(observation, capability, client_version)? {
            CachedAvailability::Fresh(record) => return Ok(Availability::Cached(record)),
            CachedAvailability::Stale { cached } => Some(cached),
            CachedAvailability::Unknown => None,
        };
        let agent = observation.naming();
        let table = self.table()?;
        let Some(row) = table.covering(capability, &observation.version, client_version) else {
            return Ok(Availability::Unavailable(Unavailable {
                agent,
                capability: capability.to_owned(),
                reason: table.explain(capability, &observation.version),
            }));
        };
        let record = AvailabilityRecord {
            agent_identity: observation.identity.as_str().to_owned(),
            agent_version: observation.version.clone(),
            capability: capability.to_owned(),
            client_version: client_version.to_owned(),
            package_id: row.package_id.clone(),
            package_version: row.package_version.clone(),
            observed_at_unix_ms: now_unix_ms(),
        };
        self.cache.write(&record)?;
        let offer = PackageOffer {
            agent: agent.clone(),
            capability: capability.to_owned(),
            package_id: row.package_id.clone(),
            package_version: row.package_version.clone(),
            display_name: row.display_name.clone(),
            source: row.source,
            requires_user_confirmation: true,
        };
        Ok(Availability::Changed(Box::new(AgentChange {
            agent,
            previous,
            record,
            offer,
        })))
    }
}

fn interval_bound(value: &str, field: &str) -> Result<Version, ApplicationFailure> {
    if value.is_empty() || value.len() > MAX_VERSION_BYTES {
        return Err(
            refusal("agent_version_interval_invalid", COMPATIBILITY_STAGE).with_field(field),
        );
    }
    Version::parse(value).map_err(|_| {
        refusal("agent_version_interval_invalid", COMPATIBILITY_STAGE).with_field(field)
    })
}

/// A package version as a sort key. Every version reaching this point was
/// validated as semantic, so the fallback is unreachable rather than forgiving.
fn row_version(value: &str) -> Version {
    Version::parse(value).unwrap_or_else(|_| Version::new(0, 0, 0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::extension_packages::artifact::content_digest;
    use crate::platform::extension_packages::state::TrustRecord;
    use crate::platform::extension_packages::{remove_managed_tree, unique_suffix};
    use licoup_extension_contracts::wire;
    use std::io::Write;

    /// The Agent-execution capability the default distribution declares, so
    /// these tests exercise the real ownership table rather than a fixture of
    /// their own.
    const CAPABILITY: &str = "agent-execution.v1";
    const GENERIC_ADAPTER: &str = "org.licoland.adapter.generic";

    fn sandbox(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("licoup-compat-{tag}-{}", unique_suffix()))
    }

    fn client_version() -> String {
        crate::platform::extension_packages::running_client_version()
            .expect("the binary declares a product version")
    }

    /// The client versions a package declares when its own list covers the
    /// running client.
    ///
    /// The development binary is a prerelease, and a semantic range admits a
    /// prerelease only when the range names one, so the range starts at this
    /// client rather than at its major line.
    fn covering_client_versions() -> Vec<String> {
        let client = client_version();
        vec![format!(">={client}, <{}", next_major(&client))]
    }

    /// A manifest a package would carry: the client versions are its own
    /// declaration, read back from here rather than restated by the table.
    fn manifest(
        package_id: &str,
        version: &str,
        display_name: &str,
        client_versions: &[&str],
    ) -> PackageManifest {
        PackageManifest::from_value(serde_json::json!({
            "schema": wire::MANIFEST,
            "id": package_id,
            "version": version,
            "displayName": display_name,
            "hostProtocol": { "major": 1, "minimumMinor": 0 },
            "compatibility": { "clientVersions": client_versions },
            "profiles": [{ "id": "agent-execution", "major": 1 }],
            "runtime": { "mode": "process", "entry": "agent.py" },
            "activation": "on-demand",
        }))
        .expect("a synthetic manifest")
    }

    /// The same manifest carrying this client's own covering list.
    fn covering_manifest(package_id: &str, version: &str, display_name: &str) -> PackageManifest {
        let declared = covering_client_versions();
        let declared_refs: Vec<&str> = declared.iter().map(String::as_str).collect();
        manifest(package_id, version, display_name, &declared_refs)
    }

    fn declarations(intervals: &[(&str, &str, &str)]) -> StrategyDeclarations {
        StrategyDeclarations::new(
            intervals
                .iter()
                .map(|(package_id, from, before)| {
                    StrategyDeclaration::new(CAPABILITY, *package_id, *from, *before)
                        .expect("a declared interval")
                })
                .collect::<Vec<_>>(),
        )
        .expect("declarations")
    }

    fn observation(version: &str) -> AgentObservation {
        AgentObservation::new(AgentIdentity::new("codex").expect("identity"), version)
            .expect("observation")
            .with_display_name("Codex")
            .expect("display name")
    }

    fn index(
        root: &Path,
        intervals: &[(&str, &str, &str)],
        packages: ManifestSet,
    ) -> AvailabilityIndex<ManifestSet> {
        AvailabilityIndex::new(
            AvailabilityCache::open_at(root).expect("cache"),
            declarations(intervals),
            packages,
        )
    }

    #[test]
    fn a_differently_versioned_package_is_admitted_when_its_list_covers_the_client() {
        let client = client_version();
        let covering = ManifestSet::new().with(
            GENERIC_ADAPTER,
            "9.9.9",
            PackageSource::ThirdPartyDirectory,
            manifest(
                GENERIC_ADAPTER,
                "9.9.9",
                "Generic adapter",
                &[&format!(">={client}, <{}", next_major(&client))],
            ),
        );
        let table = CompatibilityTable::build(
            &declarations(&[(GENERIC_ADAPTER, "0.1.0", "1.0.0")]),
            &covering,
        )
        .expect("table");

        assert_eq!(table.len(), 1);
        let row = &table.rows()[0];
        assert_eq!(
            row.package_version, "9.9.9",
            "the package's own version is unrelated"
        );
        assert_eq!(
            row.declared_client_versions().to_vec(),
            vec![format!(">={client}, <{}", next_major(&client))]
        );
        assert!(row.covers_client(&client));
        assert!(
            table.covering(CAPABILITY, "0.5.0", &client).is_some(),
            "a package version that differs from the client is admitted when its list covers it"
        );

        let out_of_range = ManifestSet::new().with(
            GENERIC_ADAPTER,
            "1.0.0",
            PackageSource::LocalDirectory,
            manifest(GENERIC_ADAPTER, "1.0.0", "Generic adapter", &[">=99.0.0"]),
        );
        let future = CompatibilityTable::build(
            &declarations(&[(GENERIC_ADAPTER, "0.1.0", "1.0.0")]),
            &out_of_range,
        )
        .expect("table");
        assert!(future.covering(CAPABILITY, "0.5.0", &client).is_none());
        assert_eq!(
            future.explain(CAPABILITY, "0.5.0"),
            UnavailableReason::ClientNotCovered {
                packages: vec![GENERIC_ADAPTER.to_owned()]
            }
        );
    }

    #[test]
    fn the_table_reads_the_manifest_and_applies_an_inclusive_start_exclusive_end() {
        let client = client_version();
        let declared = covering_client_versions();
        let declared_refs: Vec<&str> = declared.iter().map(String::as_str).collect();
        let packages = ManifestSet::new().with(
            GENERIC_ADAPTER,
            "1.0.0",
            PackageSource::LocalDirectory,
            manifest(GENERIC_ADAPTER, "1.0.0", "Generic adapter", &declared_refs),
        );
        let table = CompatibilityTable::build(
            &declarations(&[(GENERIC_ADAPTER, "0.20.0", "0.30.0")]),
            &packages,
        )
        .expect("table");

        assert_eq!(
            table.rows()[0].declared_client_versions().to_vec(),
            declared,
            "the list is the manifest's, read rather than restated"
        );
        let wire = serde_json::to_value(&table.rows()[0]).expect("a row is data");
        assert_eq!(
            wire["clientVersions"],
            serde_json::json!(declared),
            "the row carries the manifest's own list, not a nested object"
        );
        assert_eq!(wire["agentVersions"]["from"], "0.20.0");
        assert_eq!(wire["agentVersions"]["before"], "0.30.0");
        assert_eq!(wire["defaultOwner"], true);
        assert!(
            table.covering(CAPABILITY, "0.20.0", &client).is_some(),
            "inclusive start"
        );
        assert!(table.covering(CAPABILITY, "0.29.99", &client).is_some());
        assert!(
            table.covering(CAPABILITY, "0.30.0", &client).is_none(),
            "exclusive end: the boundary version belongs to the next row"
        );
        assert!(table.covering(CAPABILITY, "0.19.9", &client).is_none());
        assert_eq!(
            table.explain(CAPABILITY, "0.30.0"),
            UnavailableReason::NoDeclaredInterval
        );

        // The declared list is the only input to coverage: a manifest that says
        // something else changes the answer without a code change.
        let re_declared = ManifestSet::new().with(
            GENERIC_ADAPTER,
            "1.0.0",
            PackageSource::LocalDirectory,
            manifest(GENERIC_ADAPTER, "1.0.0", "Generic adapter", &[">=99.0.0"]),
        );
        let rebuilt = CompatibilityTable::build(
            &declarations(&[(GENERIC_ADAPTER, "0.20.0", "0.30.0")]),
            &re_declared,
        )
        .expect("table");
        assert!(!rebuilt.rows()[0].covers_client(&client));

        // A declaration for a capability nothing publishes is refused, and so is
        // an interval that is empty or ends before it starts.
        assert_eq!(
            StrategyDeclaration::new(
                "not.a.published.capability",
                GENERIC_ADAPTER,
                "0.1.0",
                "1.0.0"
            )
            .expect_err("unknown capability")
            .code,
            "compatibility_capability_unknown"
        );
        assert_eq!(
            StrategyDeclaration::new(CAPABILITY, GENERIC_ADAPTER, "1.0.0", "0.1.0")
                .expect_err("empty interval")
                .code,
            "agent_version_interval_invalid"
        );
    }

    #[test]
    fn a_package_this_host_does_not_hold_is_reported_rather_than_invented() {
        let empty = ManifestSet::new();
        let table = CompatibilityTable::build(
            &declarations(&[(GENERIC_ADAPTER, "0.1.0", "1.0.0")]),
            &empty,
        )
        .expect("table");

        assert!(table.is_empty(), "no manifest, no row");
        assert_eq!(
            table.explain(CAPABILITY, "0.5.0"),
            UnavailableReason::PackageNotInstalled {
                packages: vec![GENERIC_ADAPTER.to_owned()]
            }
        );
        assert_eq!(
            table.explain(CAPABILITY, "0.5.0").availability(),
            CapabilityAvailability::NotInstalled
        );
    }

    #[test]
    fn the_cache_answers_a_request_without_reading_a_manifest_or_starting_a_process() {
        let client = client_version();
        let root = sandbox("cache");
        let index = index(
            &root,
            &[(GENERIC_ADAPTER, "0.20.0", "0.30.0")],
            ManifestSet::new().with(
                GENERIC_ADAPTER,
                "1.0.0",
                PackageSource::LocalDirectory,
                covering_manifest(GENERIC_ADAPTER, "1.0.0", "Generic adapter"),
            ),
        );

        // Nothing is recorded yet, so a request says exactly that.
        assert_eq!(
            index
                .cached(&observation("0.21.0"), CAPABILITY, &client)
                .expect("cache read"),
            CachedAvailability::Unknown
        );

        let recorded = index
            .observe(&observation("0.21.0"), CAPABILITY, &client)
            .expect("observe");
        let Availability::Changed(change) = recorded else {
            panic!("the first observation is a decision, not a cached answer");
        };
        assert!(change.previous.is_none());
        assert_eq!(change.record.package_version, "1.0.0");
        assert!(
            change.record.observed_at_unix_ms > 0,
            "the observation is timed"
        );

        // A request reads the record and nothing else: the source is never asked
        // again, which is what "the cache answers" means.
        let fresh = index
            .cached(&observation("0.21.0"), CAPABILITY, &client)
            .expect("cache read");
        assert_eq!(fresh, CachedAvailability::Fresh(change.record.clone()));

        // A source that cannot answer at all still leaves the request path
        // working, because the request path never asks it.
        let unreadable = index_with_unreadable_source(&root);
        assert_eq!(
            unreadable
                .cached(&observation("0.21.0"), CAPABILITY, &client)
                .expect("cache read"),
            CachedAvailability::Fresh(change.record.clone())
        );
        assert_eq!(
            unreadable.packages().asked(),
            0,
            "a request reads no manifest"
        );

        // The read path's own source contains no process API: the module cannot
        // start an Agent because it has no way to start anything. The scan reads
        // the production half only, so the needles below cannot match themselves.
        let source = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("src/platform/extension_packages/compatibility.rs"),
        )
        .expect("the module source");
        let (production, _) = source
            .split_once("#[cfg(test)]")
            .expect("the module has a production half");
        assert!(
            production.contains("pub fn cached("),
            "the scan must read the real production half"
        );
        for needle in [
            "std::process",
            "process::Command",
            "Command::new",
            "spawn(",
            "--version",
        ] {
            assert!(
                !production.contains(needle),
                "the availability path must not contain {needle}"
            );
        }
        remove_managed_tree(&root).expect("cleanup");
    }

    /// A source that refuses to answer, so a test can prove a path never asked.
    #[derive(Debug, Default)]
    struct UnreadableSource {
        asked: std::cell::Cell<usize>,
    }

    impl UnreadableSource {
        fn asked(&self) -> usize {
            self.asked.get()
        }
    }

    impl DeclaredPackageSource for UnreadableSource {
        fn declarations(
            &self,
            _package_id: &str,
        ) -> Result<Vec<DeclaredPackage>, ApplicationFailure> {
            self.asked.set(self.asked.get() + 1);
            Err(refusal("compatibility_source_unused", COMPATIBILITY_STAGE))
        }
    }

    fn index_with_unreadable_source(root: &Path) -> AvailabilityIndex<UnreadableSource> {
        AvailabilityIndex::new(
            AvailabilityCache::open_at(root).expect("cache"),
            declarations(&[(GENERIC_ADAPTER, "0.20.0", "0.30.0")]),
            UnreadableSource::default(),
        )
    }

    #[test]
    fn a_version_change_invalidates_the_mapping_and_names_the_agent() {
        let client = client_version();
        let root = sandbox("change");
        let index = index(
            &root,
            &[(GENERIC_ADAPTER, "0.20.0", "0.30.0")],
            ManifestSet::new().with(
                GENERIC_ADAPTER,
                "2.0.0",
                PackageSource::OfficialDirectory,
                covering_manifest(GENERIC_ADAPTER, "2.0.0", "Generic adapter"),
            ),
        );

        let first = index
            .observe(&observation("0.21.0"), CAPABILITY, &client)
            .expect("first observation");
        let Availability::Changed(first) = first else {
            panic!("the first observation decides");
        };
        assert_eq!(first.agent.display_name, "Codex");
        assert_eq!(first.agent.version, "0.21.0");
        assert_eq!(first.offer.package_id, GENERIC_ADAPTER);
        assert_eq!(first.offer.package_version, "2.0.0");
        assert_eq!(first.offer.display_name, "Generic adapter");
        assert_eq!(first.offer.source, PackageSource::OfficialDirectory);
        assert!(first.offer.requires_user_confirmation);

        let upgraded = index
            .observe(&observation("0.25.0"), CAPABILITY, &client)
            .expect("second observation");
        let Availability::Changed(upgraded) = upgraded else {
            panic!("a new Agent version is a new decision");
        };
        assert!(
            upgraded.is_version_change(),
            "the cached mapping was decided for another version"
        );
        assert_eq!(upgraded.previous_version(), Some("0.21.0"));
        assert_eq!(upgraded.agent.display_name, "Codex", "the Agent is named");
        assert_eq!(upgraded.agent.version, "0.25.0");
        assert_eq!(upgraded.record.agent_version, "0.25.0");
        assert_eq!(upgraded.offer.agent.identity.as_str(), "codex");

        // The stale mapping is replaced, not reused.
        assert_eq!(
            index
                .cached(&observation("0.25.0"), CAPABILITY, &client)
                .expect("cache read"),
            CachedAvailability::Fresh(upgraded.record.clone())
        );
        assert_eq!(
            index
                .cached(&observation("0.21.0"), CAPABILITY, &client)
                .expect("cache read"),
            CachedAvailability::Stale {
                cached: upgraded.record.clone()
            }
        );

        // A client change is staleness as well: the same package may cover one
        // client and not the next.
        let other_client = format!("{client}-next");
        assert!(matches!(
            index
                .cached(&observation("0.25.0"), CAPABILITY, &other_client)
                .expect("cache read"),
            CachedAvailability::Stale { .. }
        ));
        remove_managed_tree(&root).expect("cleanup");
    }

    #[test]
    fn an_unknown_agent_stays_unknown_and_a_declined_offer_changes_nothing() {
        let client = client_version();
        let root = sandbox("unknown");
        let unknown = index(&root, &[], ManifestSet::new());

        assert_eq!(
            unknown
                .cached(&observation("0.21.0"), CAPABILITY, &client)
                .expect("cache read"),
            CachedAvailability::Unknown
        );

        let decided = unknown
            .observe(&observation("0.21.0"), CAPABILITY, &client)
            .expect("observe");
        let Availability::Unavailable(unavailable) = decided else {
            panic!("no declaration, no availability");
        };
        assert_eq!(unavailable.reason, UnavailableReason::NoDeclaredInterval);
        assert_eq!(unavailable.agent.display_name, "Codex");
        assert_eq!(
            unavailable.availability(),
            CapabilityAvailability::NotInDistribution
        );
        assert!(!unavailable.availability().is_served());
        let failure = unavailable.refusal();
        assert_eq!(failure.code, "capability_unavailable");
        assert_eq!(
            failure.presentation_args.get("agent"),
            Some("Codex"),
            "the refusal names the Agent"
        );
        assert_eq!(
            failure.presentation_args.get("agentVersion"),
            Some("0.21.0"),
            "and the version that made it unavailable"
        );
        assert_eq!(
            failure.presentation_args.get("capability"),
            Some(CAPABILITY)
        );
        assert_eq!(
            failure.presentation_args.get("state"),
            Some("not-in-distribution"),
            "the contracts' own catalogue fact survives the two Agent arguments"
        );

        // Nothing was fabricated: no record was written for a mapping that was
        // never decided, and the request path still says unknown.
        assert_eq!(
            unknown
                .cached(&observation("0.21.0"), CAPABILITY, &client)
                .expect("cache read"),
            CachedAvailability::Unknown
        );

        // A declined offer leaves the capability exactly as unavailable and
        // substitutes nothing.
        let offered = index(
            &root,
            &[(GENERIC_ADAPTER, "0.20.0", "0.30.0")],
            ManifestSet::new().with(
                GENERIC_ADAPTER,
                "1.0.0",
                PackageSource::LocalDirectory,
                covering_manifest(GENERIC_ADAPTER, "1.0.0", "Generic adapter"),
            ),
        );
        let Availability::Changed(change) = offered
            .observe(&observation("0.21.0"), CAPABILITY, &client)
            .expect("observe")
        else {
            panic!("a held package is offered");
        };
        let mut log = RecommendationLog::new();
        change.offer.decline(&mut log, "not needed here");
        assert!(log.was_declined(GENERIC_ADAPTER));
        assert!(!log.was_declined("org.licoland.adapter.somewhere.else"));
        assert_eq!(log.declined().count(), 1);
        assert_eq!(log.accepted().count(), 0);

        // Declining is a decision about an offer, not a change to the packages:
        // the record stays what was observed, and nothing became available.
        assert!(matches!(
            offered
                .cached(&observation("0.21.0"), CAPABILITY, &client)
                .expect("cache read"),
            CachedAvailability::Fresh(_)
        ));
        let unavailable = offered
            .observe(&observation("0.31.0"), CAPABILITY, &client)
            .expect("observe");
        assert!(matches!(unavailable, Availability::Unavailable(_)));
        remove_managed_tree(&root).expect("cleanup");
    }

    #[test]
    fn a_record_for_another_identity_is_refused_rather_than_reused() {
        let root = sandbox("identity");
        let cache = AvailabilityCache::open_at(&root).expect("cache");
        let codex = AgentIdentity::new("codex").expect("identity");
        let claude = AgentIdentity::new("claude-code").expect("identity");
        cache
            .write(&AvailabilityRecord {
                agent_identity: "codex".to_owned(),
                agent_version: "0.21.0".to_owned(),
                capability: CAPABILITY.to_owned(),
                client_version: client_version(),
                package_id: GENERIC_ADAPTER.to_owned(),
                package_version: "1.0.0".to_owned(),
                observed_at_unix_ms: 1,
            })
            .expect("write");

        // The file is the identity's own; reading it as another identity cannot
        // happen through the cache, and a crafted record is refused by name.
        std::fs::write(
            cache.record_path(&claude),
            serde_json::json!({
                "agentIdentity": "codex",
                "agentVersion": "0.21.0",
                "capability": CAPABILITY,
                "clientVersion": client_version(),
                "packageId": GENERIC_ADAPTER,
                "packageVersion": "1.0.0",
                "observedAtUnixMs": 1,
            })
            .to_string(),
        )
        .expect("crafted record");
        let failure = cache
            .read(&claude, "0.21.0", CAPABILITY, &client_version())
            .expect_err("another identity's record is not this Agent's answer");
        assert_eq!(failure.code, "availability_record_invalid");
        assert_eq!(failure.field.as_deref(), Some("agentIdentity"));
        assert!(cache.invalidate(&claude).expect("invalidate"));
        assert!(!cache.invalidate(&claude).expect("idempotent"));
        assert!(cache.invalidate(&codex).expect("invalidated"));
        remove_managed_tree(&root).expect("cleanup");
    }

    #[test]
    fn the_package_store_answers_the_table_from_a_synthetic_root() {
        let client = client_version();
        let root = sandbox("store");
        let store = PackageStore::open(&root).expect("store");
        let bytes = package_bytes(GENERIC_ADAPTER, "3.0.0", &client);
        let trust = TrustRecord::local_approved(content_digest(&bytes), []).expect("trust");
        store
            .install_local_import(GENERIC_ADAPTER, "3.0.0", trust, &bytes)
            .expect("install");

        let table = CompatibilityTable::build(
            &declarations(&[(GENERIC_ADAPTER, "0.20.0", "0.30.0")]),
            &store,
        )
        .expect("table");
        assert_eq!(table.len(), 1);
        let row = &table.rows()[0];
        assert_eq!(row.package_version, "3.0.0");
        assert_eq!(row.display_name, "Generic adapter");
        assert_eq!(row.source, PackageSource::LocalImport);
        assert!(
            row.default_owner,
            "the generic adapter is the declared owner"
        );
        assert!(row.covers_client(&client));
        assert_eq!(
            row.declared_client_versions().to_vec(),
            vec![format!(">={client}, <{}", next_major(&client))]
        );

        // The cache sits below the store root, so relocating the store relocates
        // its availability records with it.
        let cache = AvailabilityCache::open(&store).expect("cache");
        assert_eq!(
            cache.root(),
            root.canonicalize()
                .expect("canonical root")
                .join("cache")
                .join("availability")
        );
        let index = AvailabilityIndex::new(
            cache,
            declarations(&[(GENERIC_ADAPTER, "0.20.0", "0.30.0")]),
            store,
        );
        let observed = index
            .observe(&observation("0.21.0"), CAPABILITY, &client)
            .expect("observe");
        let Availability::Changed(change) = observed else {
            panic!("the store's own package answers the table");
        };
        assert_eq!(change.record.package_version, "3.0.0");
        assert_eq!(change.offer.display_name, "Generic adapter");
        remove_managed_tree(&root).expect("cleanup");
    }

    #[test]
    fn a_declaration_table_is_read_from_the_document_a_caller_holds() {
        let client = client_version();
        let document = serde_json::json!({
            "strategies": [{
                "capability": CAPABILITY,
                "packageId": GENERIC_ADAPTER,
                "agentVersions": { "from": "0.20.0", "before": "0.30.0" },
            }],
        })
        .to_string();
        let read = StrategyDeclarations::from_json(&document).expect("declarations");
        assert_eq!(read.strategies().len(), 1);
        assert!(read.strategies()[0].agent_versions.contains_text("0.29.9"));
        assert!(!read.strategies()[0].agent_versions.contains_text("0.30.0"));

        let table = CompatibilityTable::build(
            &read,
            &ManifestSet::new().with(
                GENERIC_ADAPTER,
                "1.0.0",
                PackageSource::LocalDirectory,
                covering_manifest(GENERIC_ADAPTER, "1.0.0", "Generic adapter"),
            ),
        )
        .expect("table");
        assert!(table.covering(CAPABILITY, "0.25.0", &client).is_some());

        assert_eq!(
            StrategyDeclarations::from_json("{\"strategies\":[{\"capability\":\"x\"}]}")
                .expect_err("an incomplete declaration is refused")
                .code,
            "compatibility_declaration_invalid"
        );
        assert_eq!(
            StrategyDeclarations::new([
                StrategyDeclaration::new(CAPABILITY, GENERIC_ADAPTER, "0.1.0", "1.0.0").unwrap(),
                StrategyDeclaration::new(CAPABILITY, GENERIC_ADAPTER, "1.0.0", "2.0.0").unwrap(),
            ])
            .expect_err("one package per capability is declared once")
            .code,
            "compatibility_declaration_duplicated"
        );
    }

    /// The next major line after one client version, from that version rather
    /// than from a literal here.
    fn next_major(client: &str) -> u64 {
        client
            .split('.')
            .next()
            .and_then(|major| major.parse::<u64>().ok())
            .expect("a semantic major version")
            + 1
    }

    /// A package archive a store can install, so the store-backed source is
    /// exercised against a synthetic root rather than described.
    fn package_bytes(package_id: &str, version: &str, client: &str) -> Vec<u8> {
        let manifest = serde_json::json!({
            "schema": wire::MANIFEST,
            "id": package_id,
            "version": version,
            "displayName": "Generic adapter",
            "hostProtocol": { "major": 1, "minimumMinor": 0 },
            "compatibility": {
                "clientVersions": [format!(">={client}, <{}", next_major(client))],
            },
            "profiles": [{ "id": "agent-execution", "major": 1 }],
            "runtime": { "mode": "process", "entry": "agent.py" },
            "activation": "on-demand",
        })
        .to_string();
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        writer.start_file("manifest.json", options).expect("entry");
        writer
            .write_all(manifest.as_bytes())
            .expect("manifest bytes");
        writer.start_file("agent.py", options).expect("entry");
        writer.write_all(b"print('echo')\n").expect("entry bytes");
        writer.finish().expect("finish").into_inner()
    }
}
