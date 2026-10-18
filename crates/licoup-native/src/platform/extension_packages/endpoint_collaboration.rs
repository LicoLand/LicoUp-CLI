//! The optional endpoint collaboration package, as the kernel resolves it.
//!
//! `org.licoland.feature.endpoint-collaboration` owns paired-device pairing,
//! secure mesh relay delivery and remote work control. The kernel owns custody,
//! durable state, the Canonical Conversation and authority, so the package is
//! never a prerequisite for the local client: this module answers whether the
//! capability is installed, switched on and declaring what it claims, and it
//! refuses outbound endpoint traffic while it is not.
//!
//! Three rules this owner exists to hold:
//!
//! * **Absent and disabled are different answers from "as before".** A caller
//!   that receives one of them reports it; it does not fall back to a bundled
//!   neighbour or to the kernel's own pre-package path.
//! * **Disable cuts outbound capability.** The gate below is consulted at the
//!   outbound transport entries in [`crate::domain::mobile_relay`], so a disabled
//!   or uninstalled package refuses the send instead of leaving a path that only
//!   a user interface stops showing.
//! * **An uninstalled package is not a broken store.** A store that cannot be
//!   read resolves as [`EndpointCollaborationAvailability::Unreadable`] and fails
//!   closed, and it is never reported as an uninstalled package.
//!
//! The package's own vocabulary and payload live in
//! `components/endpoint-collaboration`, which is its own workspace and its own
//! artifact: this kernel declares no cargo edge to it, so the installed bytes a
//! build ships and the kernel that gates on them stay separable, and the kernel
//! reaches the package only through its installed manifest and this gate. A build
//! that carries no package reports
//! [`EndpointOutboundAuthority::LegacyInKernel`] for the kernel's own pre-package
//! path, which is what is actually running.

use anyhow::{Result, anyhow};
use licoup_foundation::platform::paths;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use crate::platform::extension_packages::install::{InstalledPackage, PackageStore};
use crate::platform::extension_packages::switched_on;

/// The optional package whose payload is the endpoint collaboration capability.
pub const ENDPOINT_COLLABORATION_PACKAGE_ID: &str = "org.licoland.feature.endpoint-collaboration";

/// The capability an installed package must declare to own the outbound path.
pub const ENDPOINT_COLLABORATION_CAPABILITY_ID: &str = "endpoint.collaboration.v1";

/// The profile that declares [`ENDPOINT_COLLABORATION_CAPABILITY_ID`].
pub const ENDPOINT_COLLABORATION_PROFILE_ID: &str = "endpoint-control";

/// The shipped declaration this module is the host side of.
pub const ENDPOINT_COLLABORATION_MANIFEST_PATH: &str =
    "components/endpoint-collaboration/package/manifest.json";

/// What the host resolved about the installed endpoint collaboration package.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EndpointCollaborationAvailability {
    /// Installed, switched on and declaring the outbound capability.
    Active { version: String },
    /// Installed and switched off by the user, whose switch is the answer.
    Disabled { version: String },
    /// Installed and switched on without declaring the outbound capability.
    CapabilityUndeclared { version: String },
    /// No installed version of this package is present.
    Missing,
    /// The package store could not be read, so no honest answer exists.
    Unreadable,
}

impl EndpointCollaborationAvailability {
    /// The stable name of this answer, for a status surface.
    #[must_use]
    pub const fn state(&self) -> &'static str {
        match self {
            Self::Active { .. } => "active",
            Self::Disabled { .. } => "disabled",
            Self::CapabilityUndeclared { .. } => "capability-undeclared",
            Self::Missing => "missing",
            Self::Unreadable => "unreadable",
        }
    }

    /// The installed version this answer is about, when there is one.
    #[must_use]
    pub fn version(&self) -> Option<&str> {
        match self {
            Self::Active { version }
            | Self::Disabled { version }
            | Self::CapabilityUndeclared { version } => Some(version),
            Self::Missing | Self::Unreadable => None,
        }
    }

    /// Whether outbound endpoint traffic may leave this client.
    #[must_use]
    pub const fn permits_outbound(&self) -> bool {
        matches!(self, Self::Active { .. })
    }
}

/// Why outbound endpoint traffic is refused.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EndpointOutboundRefusal {
    PackageMissing,
    PackageDisabled,
    CapabilityUndeclared,
    StoreUnreadable,
}

impl EndpointOutboundRefusal {
    /// The stable reason string a caller publishes verbatim.
    #[must_use]
    pub const fn reason(self) -> &'static str {
        match self {
            Self::PackageMissing => "endpoint_collaboration_package_absent",
            Self::PackageDisabled => "endpoint_collaboration_package_disabled",
            Self::CapabilityUndeclared => "endpoint_collaboration_capability_undeclared",
            Self::StoreUnreadable => "endpoint_collaboration_store_unreadable",
        }
    }

    /// The refusal one resolved availability produces.
    #[must_use]
    pub fn of(availability: &EndpointCollaborationAvailability) -> Option<Self> {
        match availability {
            EndpointCollaborationAvailability::Active { .. } => None,
            EndpointCollaborationAvailability::Disabled { .. } => Some(Self::PackageDisabled),
            EndpointCollaborationAvailability::CapabilityUndeclared { .. } => {
                Some(Self::CapabilityUndeclared)
            }
            EndpointCollaborationAvailability::Missing => Some(Self::PackageMissing),
            EndpointCollaborationAvailability::Unreadable => Some(Self::StoreUnreadable),
        }
    }
}

/// Resolve one answer from the three facts the store publishes.
///
/// `installed_version` is the highest installed version, `switched_on` is the
/// user's switch for it (`None` when no version is installed), and
/// `capability_declared` says whether that version's installed manifest declares
/// [`ENDPOINT_COLLABORATION_CAPABILITY_ID`].
///
/// Precedence is deliberate: an uninstalled package is `Missing`; a switched-off
/// package is `Disabled` and its declaration is not consulted, because the user's
/// switch is the actionable answer; a package that never declared the capability
/// cannot own the outbound path even while switched on.
#[must_use]
pub fn resolve_availability(
    installed_version: Option<&str>,
    switched_on: Option<bool>,
    capability_declared: bool,
) -> EndpointCollaborationAvailability {
    let Some(version) = installed_version.map(str::to_owned) else {
        return EndpointCollaborationAvailability::Missing;
    };
    if switched_on == Some(false) {
        return EndpointCollaborationAvailability::Disabled { version };
    }
    if !capability_declared {
        return EndpointCollaborationAvailability::CapabilityUndeclared { version };
    }
    EndpointCollaborationAvailability::Active { version }
}

/// Whether this client may emit outbound endpoint traffic, and who runs it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EndpointOutboundAuthority {
    /// The installed package owns the outbound path and permits it.
    Package { version: String },
    /// This build carries the kernel's own pre-package path and no installed
    /// package has taken the path over. It is a statement about which
    /// implementation is running, not a grant.
    LegacyInKernel,
}

/// One process's installed endpoint collaboration authority.
///
/// The production program has exactly one. Tests build their own, so an installed
/// answer and the pre-package default are both observable without depending on
/// test order or on the developer's own package store.
pub struct EndpointCollaborationGate {
    installed: RwLock<Option<EndpointCollaborationAvailability>>,
}

impl EndpointCollaborationGate {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            installed: RwLock::new(None),
        }
    }

    /// Install the authority the package lifecycle resolved.
    ///
    /// The previous answer is returned: activation after a disable replaces the
    /// refusal, and deactivation installs the refusal that cuts outbound traffic.
    pub fn install(
        &self,
        availability: EndpointCollaborationAvailability,
    ) -> Option<EndpointCollaborationAvailability> {
        let mut installed = self
            .installed
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        installed.replace(availability)
    }

    /// The answer the composition installed, or `None` for a build that never
    /// provisioned the package.
    #[must_use]
    pub fn installed(&self) -> Option<EndpointCollaborationAvailability> {
        self.installed
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Whether outbound endpoint traffic may leave this client.
    ///
    /// An installed refusal is authoritative and refuses. With nothing installed
    /// the client runs its own pre-package path, and that path is named as such
    /// rather than reported as a granted package authority.
    pub fn authority(&self) -> Result<EndpointOutboundAuthority, EndpointOutboundRefusal> {
        match self.installed() {
            Some(availability) => match EndpointOutboundRefusal::of(&availability) {
                Some(refusal) => Err(refusal),
                None => Ok(EndpointOutboundAuthority::Package {
                    version: availability.version().unwrap_or_default().to_owned(),
                }),
            },
            None => Ok(EndpointOutboundAuthority::LegacyInKernel),
        }
    }
}

impl Default for EndpointCollaborationGate {
    fn default() -> Self {
        Self::new()
    }
}

static ENDPOINT_COLLABORATION: EndpointCollaborationGate = EndpointCollaborationGate::new();

/// The process-wide gate the outbound transport entries consult.
#[must_use]
pub fn endpoint_collaboration_gate() -> &'static EndpointCollaborationGate {
    &ENDPOINT_COLLABORATION
}

/// The installed endpoint collaboration package, read from the managed store.
#[derive(Clone, Debug)]
pub struct EndpointCollaborationBinding {
    store_root: PathBuf,
}

impl EndpointCollaborationBinding {
    /// The binding over a managed package root.
    pub fn over(store_root: impl Into<PathBuf>) -> Self {
        Self {
            store_root: store_root.into(),
        }
    }

    /// The binding over the running user's own package store.
    pub fn for_current_user() -> Result<Self> {
        let data_home = paths::portable_data_dir()?;
        Ok(Self::over(data_home.join("extension-packages")))
    }

    #[must_use]
    pub fn store_root(&self) -> &Path {
        &self.store_root
    }

    fn store(&self) -> Result<PackageStore> {
        PackageStore::open(&self.store_root).map_err(|_| anyhow!("package_store_unavailable"))
    }

    /// The highest installed version of the endpoint collaboration package.
    ///
    /// A version that does not parse as semantic versioning sorts below every one
    /// that does and is only chosen when it is the only candidate: an installed
    /// record with an unreadable version is not evidence that a newer one is
    /// absent.
    fn installed_version(&self) -> Result<Option<String>> {
        let installed: Vec<InstalledPackage> = self
            .store()?
            .installed()
            .map_err(|_| anyhow!("package_store_unavailable"))?
            .into_iter()
            .filter(|package| package.package_id == ENDPOINT_COLLABORATION_PACKAGE_ID)
            .collect();
        Ok(installed
            .into_iter()
            .max_by_key(|package| version_key(&package.version))
            .map(|package| package.version))
    }

    /// Whether one installed version's manifest declares the outbound capability.
    fn capability_declared(&self, version: &str) -> Result<bool> {
        let manifest = self
            .store()?
            .installed_manifest(ENDPOINT_COLLABORATION_PACKAGE_ID, version)
            .map_err(|_| anyhow!("package_manifest_unavailable"))?;
        Ok(manifest.profiles.iter().any(|profile| {
            profile
                .capabilities
                .iter()
                .any(|capability| capability == ENDPOINT_COLLABORATION_CAPABILITY_ID)
        }))
    }

    /// What this host resolved about the installed package.
    ///
    /// Every read failure resolves as [`EndpointCollaborationAvailability::Unreadable`]:
    /// a store this client cannot read is not an uninstalled package, and it
    /// permits no outbound traffic either way.
    #[must_use]
    pub fn availability(&self) -> EndpointCollaborationAvailability {
        let version = match self.installed_version() {
            Ok(Some(version)) => version,
            Ok(None) => return EndpointCollaborationAvailability::Missing,
            Err(_) => return EndpointCollaborationAvailability::Unreadable,
        };
        let switched_on = match switched_on(
            &self.store_root,
            ENDPOINT_COLLABORATION_PACKAGE_ID,
            &version,
        ) {
            Ok(switched_on) => switched_on,
            Err(_) => return EndpointCollaborationAvailability::Unreadable,
        };
        if !switched_on {
            return EndpointCollaborationAvailability::Disabled { version };
        }
        match self.capability_declared(&version) {
            Ok(true) => EndpointCollaborationAvailability::Active { version },
            Ok(false) => EndpointCollaborationAvailability::CapabilityUndeclared { version },
            Err(_) => EndpointCollaborationAvailability::Unreadable,
        }
    }

    /// Whether outbound endpoint traffic may leave this client.
    pub fn require_outbound(&self) -> Result<EndpointOutboundAuthority, EndpointOutboundRefusal> {
        let availability = self.availability();
        match EndpointOutboundRefusal::of(&availability) {
            Some(refusal) => Err(refusal),
            None => Ok(EndpointOutboundAuthority::Package {
                version: availability.version().unwrap_or_default().to_owned(),
            }),
        }
    }

    /// The honest status report for the client's own surface.
    ///
    /// `localClientUsable` is always true: the local client, its conversations,
    /// its running work and its history never depend on this optional package.
    #[must_use]
    pub fn recovery_report(&self) -> Value {
        let availability = self.availability();
        let refusal = EndpointOutboundRefusal::of(&availability);
        json!({
            "packageId": ENDPOINT_COLLABORATION_PACKAGE_ID,
            "capabilityId": ENDPOINT_COLLABORATION_CAPABILITY_ID,
            "state": availability.state(),
            "version": availability.version(),
            "outboundPermitted": availability.permits_outbound(),
            "outboundRefusal": refusal.map(EndpointOutboundRefusal::reason),
            "localClientUsable": true,
            "recovery": match refusal {
                None => "none",
                Some(EndpointOutboundRefusal::PackageMissing) => "install-package",
                Some(EndpointOutboundRefusal::PackageDisabled) => "enable-package",
                Some(EndpointOutboundRefusal::CapabilityUndeclared) => "install-capable-version",
                Some(EndpointOutboundRefusal::StoreUnreadable) => "repair-store",
            },
        })
    }

    /// Resolve the installed package and install its authority into `gate`.
    ///
    /// This is the entry the package lifecycle reaches after an install or an
    /// enable. It adds no second registration path: the gate it writes is the one
    /// the outbound transport entries already consult.
    pub fn activate(&self, gate: &EndpointCollaborationGate) -> EndpointCollaborationAvailability {
        let availability = self.availability();
        gate.install(availability.clone());
        availability
    }

    /// Install the refusal that cuts outbound traffic before the installed bytes
    /// go away.
    ///
    /// A disable and an uninstall both end the capability's authority. A store
    /// that still reads active is retired as disabled — the capability is being
    /// withdrawn, and re-reading a store that is about to change would leave the
    /// grant in place for exactly as long as the change takes.
    pub fn retire(&self, gate: &EndpointCollaborationGate) -> EndpointOutboundRefusal {
        let availability = self.availability();
        if let Some(refusal) = EndpointOutboundRefusal::of(&availability) {
            gate.install(availability);
            return refusal;
        }
        let version = availability.version().unwrap_or_default().to_owned();
        gate.install(EndpointCollaborationAvailability::Disabled { version });
        EndpointOutboundRefusal::PackageDisabled
    }
}

/// The lifecycle transition one package route performs on the outbound gate.
///
/// An install and an enable both make the package's own answer the one the
/// outbound transport entries read; a disable and an uninstall both withdraw it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EndpointCollaborationLifecycle {
    /// An install or an enable: the store's own answer takes the outbound path.
    Activated,
    /// A disable or an uninstall: the refusal that cuts outbound traffic.
    Retired,
}

/// Apply one package lifecycle transition over the package's own gate.
///
/// `store_root` is the managed root the route just changed, `package_id` is the
/// package that operation was about, and `gate` is the process-wide gate the
/// outbound transport entries consult. A route about another package changes
/// nothing here and returns `None`: one package's lifecycle never speaks for a
/// neighbour's gate.
///
/// The binding is resolved from the store the route changed, so an install, an
/// enable, a disable and an uninstall all flip the answer the port gives without
/// a second registration path.
pub fn apply_endpoint_collaboration_lifecycle(
    store_root: &Path,
    package_id: &str,
    transition: EndpointCollaborationLifecycle,
    gate: &EndpointCollaborationGate,
) -> Option<EndpointCollaborationAvailability> {
    if package_id != ENDPOINT_COLLABORATION_PACKAGE_ID {
        return None;
    }
    let binding = EndpointCollaborationBinding::over(store_root);
    Some(match transition {
        EndpointCollaborationLifecycle::Activated => binding.activate(gate),
        EndpointCollaborationLifecycle::Retired => {
            binding.retire(gate);
            gate.installed()
                .unwrap_or(EndpointCollaborationAvailability::Missing)
        }
    })
}

/// Order two version strings by semantic versioning, with unparsable ones below
/// every parsable version.
fn version_key(version: &str) -> (u8, semver::Version) {
    match semver::Version::parse(version) {
        Ok(parsed) => (1, parsed),
        Err(_) => (0, semver::Version::new(0, 0, 0)),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ENDPOINT_COLLABORATION_CAPABILITY_ID, ENDPOINT_COLLABORATION_MANIFEST_PATH,
        ENDPOINT_COLLABORATION_PACKAGE_ID, ENDPOINT_COLLABORATION_PROFILE_ID,
        EndpointCollaborationAvailability, EndpointCollaborationBinding, EndpointCollaborationGate,
        EndpointCollaborationLifecycle, EndpointOutboundAuthority, EndpointOutboundRefusal,
        apply_endpoint_collaboration_lifecycle, resolve_availability,
    };
    use licoup_extension_contracts::manifest::{PackageManifest, Runtime};
    use serde_json::json;
    use std::fs;
    use std::path::{Path, PathBuf};

    fn workspace_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .canonicalize()
            .expect("workspace root resolves")
    }

    fn synthetic_store_root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "licoup-endpoint-collaboration-{label}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("records")).expect("synthetic store root is writable");
        root
    }

    /// The declared version the permitted fixture installs.
    const INSTALLED_VERSION: &str = "0.3.0";

    /// The client versions a fixture package declares when its own list covers
    /// the client running this test.
    ///
    /// A development binary is a prerelease, and a semantic range admits a
    /// prerelease only when the range names one, so the range starts at this
    /// client rather than at its major line.
    fn covering_client_versions() -> Vec<String> {
        let client = crate::platform::extension_packages::running_client_version()
            .expect("the binary declares a product version");
        let next_major = client
            .split('.')
            .next()
            .and_then(|major| major.parse::<u64>().ok())
            .map(|major| major + 1)
            .expect("a semantic major version");
        vec![format!(">={client}, <{next_major}")]
    }

    /// A store root holding one installed, switched-on version of this package
    /// that declares the outbound capability.
    ///
    /// This is the permitted package a disable or an uninstall withdraws: the
    /// store itself answers `Active`, so a retirement has a grant to cut rather
    /// than a store that already reads absent.
    fn permitted_store_root(label: &str) -> PathBuf {
        installed_store_root(label, &[ENDPOINT_COLLABORATION_CAPABILITY_ID])
    }

    /// A store root holding one installed, switched-on version of this package
    /// whose own profile declares exactly `capabilities`.
    ///
    /// A version that declares a different capability is installed and switched
    /// on and still does not own the outbound path, which is the answer this
    /// fixture exists to produce.
    fn installed_store_root(label: &str, capabilities: &[&str]) -> PathBuf {
        use super::super::PackageStore;
        use super::super::artifact::content_digest;
        use super::super::install::InstallRequest;
        use super::super::state::TrustRecord;
        use licoup_extension_contracts::deployment::PackageSource;
        use licoup_extension_contracts::wire;
        use std::io::Write;

        let root = synthetic_store_root(label);
        let manifest = json!({
            "schema": wire::MANIFEST,
            "id": ENDPOINT_COLLABORATION_PACKAGE_ID,
            "version": INSTALLED_VERSION,
            "displayName": "Synthetic endpoint collaboration",
            "hostProtocol": {"major": 1, "minimumMinor": 0},
            "compatibility": {"clientVersions": covering_client_versions()},
            "profiles": [{
                "id": ENDPOINT_COLLABORATION_PROFILE_ID,
                "major": 1,
                "capabilities": capabilities,
            }],
            "runtime": {
                "mode": "declarative",
                "descriptor": "contributions/control-surface.json",
            },
            "activation": "on-demand",
            "requires": [],
            "optionalRequires": [],
            "permissions": [],
            "contributions": [],
        });
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let plain = zip::write::SimpleFileOptions::default();
        for (name, body) in [
            ("manifest.json", manifest.to_string()),
            ("contributions/control-surface.json", "{}".to_owned()),
        ] {
            writer.start_file(name, plain).expect("entry");
            writer.write_all(body.as_bytes()).expect("content");
        }
        let bytes = writer.finish().expect("finish").into_inner();
        let trust = TrustRecord::local_approved(content_digest(&bytes), Vec::new()).expect("trust");
        PackageStore::open(&root)
            .expect("store")
            .install(
                &InstallRequest::new(
                    ENDPOINT_COLLABORATION_PACKAGE_ID,
                    INSTALLED_VERSION,
                    PackageSource::LocalImport,
                    trust,
                ),
                &bytes,
            )
            .expect("the store installs the fixture package");
        root
    }

    #[test]
    fn an_empty_store_resolves_as_missing_rather_than_unreadable() {
        let binding = EndpointCollaborationBinding::over(synthetic_store_root("empty"));

        assert_eq!(
            binding.availability(),
            EndpointCollaborationAvailability::Missing
        );
        assert_eq!(
            binding.require_outbound(),
            Err(EndpointOutboundRefusal::PackageMissing)
        );
        let report = binding.recovery_report();
        assert_eq!(report["state"], "missing");
        assert_eq!(report["localClientUsable"], true);
        assert_eq!(report["outboundPermitted"], false);
        assert_eq!(
            report["outboundRefusal"],
            "endpoint_collaboration_package_absent"
        );
        assert_eq!(report["recovery"], "install-package");
    }

    #[test]
    fn a_store_that_cannot_be_read_fails_closed_and_says_so() {
        let file = std::env::temp_dir().join(format!(
            "licoup-endpoint-collaboration-not-a-directory-{}",
            std::process::id()
        ));
        fs::write(&file, b"not a store").expect("synthetic file is writable");
        let binding = EndpointCollaborationBinding::over(&file);

        assert_eq!(
            binding.availability(),
            EndpointCollaborationAvailability::Unreadable
        );
        assert_eq!(
            binding.require_outbound(),
            Err(EndpointOutboundRefusal::StoreUnreadable)
        );
        assert_eq!(binding.recovery_report()["state"], "unreadable");
        assert_eq!(binding.recovery_report()["recovery"], "repair-store");
        let _ = fs::remove_file(&file);
    }

    #[test]
    fn a_switched_off_package_is_disabled_and_its_declaration_is_not_consulted() {
        assert_eq!(
            resolve_availability(Some("0.3.0"), Some(false), false),
            EndpointCollaborationAvailability::Disabled {
                version: "0.3.0".to_owned()
            }
        );
        assert_eq!(
            resolve_availability(Some("0.3.0"), Some(false), true),
            EndpointCollaborationAvailability::Disabled {
                version: "0.3.0".to_owned()
            }
        );
        assert!(
            !EndpointCollaborationAvailability::Disabled {
                version: "0.3.0".to_owned()
            }
            .permits_outbound()
        );
    }

    #[test]
    fn a_package_that_never_declared_the_capability_does_not_own_the_outbound_path() {
        assert_eq!(
            resolve_availability(Some("0.3.0"), Some(true), false),
            EndpointCollaborationAvailability::CapabilityUndeclared {
                version: "0.3.0".to_owned()
            }
        );
        assert_eq!(
            EndpointOutboundRefusal::of(&EndpointCollaborationAvailability::CapabilityUndeclared {
                version: "0.3.0".to_owned()
            }),
            Some(EndpointOutboundRefusal::CapabilityUndeclared)
        );
    }

    #[test]
    fn only_an_installed_switched_on_capable_package_resolves_active() {
        assert_eq!(
            resolve_availability(Some("0.3.0"), Some(true), true),
            EndpointCollaborationAvailability::Active {
                version: "0.3.0".to_owned()
            }
        );
        assert_eq!(
            resolve_availability(None, None, true),
            EndpointCollaborationAvailability::Missing
        );
    }

    #[test]
    fn retiring_a_permitted_package_installs_the_refusal_before_the_bytes_go_away() {
        let binding = EndpointCollaborationBinding::over(permitted_store_root("retire"));
        assert_eq!(
            binding.availability(),
            EndpointCollaborationAvailability::Active {
                version: INSTALLED_VERSION.to_owned()
            },
            "the fixture is the permitted package this case retires"
        );
        let gate = EndpointCollaborationGate::new();
        gate.install(EndpointCollaborationAvailability::Active {
            version: INSTALLED_VERSION.to_owned(),
        });
        assert!(gate.authority().is_ok());

        assert_eq!(
            binding.retire(&gate),
            EndpointOutboundRefusal::PackageDisabled
        );
        assert_eq!(
            gate.authority(),
            Err(EndpointOutboundRefusal::PackageDisabled)
        );
    }

    #[test]
    fn a_lifecycle_transition_speaks_only_for_the_package_it_was_about() {
        let root = permitted_store_root("lifecycle-other-package");
        let gate = EndpointCollaborationGate::new();

        assert_eq!(
            apply_endpoint_collaboration_lifecycle(
                &root,
                "example.other.package",
                EndpointCollaborationLifecycle::Activated,
                &gate,
            ),
            None
        );
        assert_eq!(gate.installed(), None);
        assert_eq!(
            gate.authority(),
            Ok(EndpointOutboundAuthority::LegacyInKernel)
        );

        assert_eq!(
            apply_endpoint_collaboration_lifecycle(
                &root,
                ENDPOINT_COLLABORATION_PACKAGE_ID,
                EndpointCollaborationLifecycle::Activated,
                &gate,
            ),
            Some(EndpointCollaborationAvailability::Active {
                version: INSTALLED_VERSION.to_owned()
            })
        );
        assert_eq!(
            gate.authority(),
            Ok(EndpointOutboundAuthority::Package {
                version: INSTALLED_VERSION.to_owned()
            })
        );

        // A retirement withdraws the grant: the gate holds the refusal that cuts
        // outbound traffic and does not fall back to the pre-package path the
        // process answered with before any package owned it.
        assert_eq!(
            apply_endpoint_collaboration_lifecycle(
                &root,
                ENDPOINT_COLLABORATION_PACKAGE_ID,
                EndpointCollaborationLifecycle::Retired,
                &gate,
            ),
            Some(EndpointCollaborationAvailability::Disabled {
                version: INSTALLED_VERSION.to_owned()
            })
        );
        assert_eq!(
            gate.authority(),
            Err(EndpointOutboundRefusal::PackageDisabled)
        );
        assert_ne!(
            gate.authority(),
            Ok(EndpointOutboundAuthority::LegacyInKernel),
            "a withdrawn capability is a refusal, not the pre-package path"
        );
    }

    #[test]
    fn activating_a_version_that_declares_no_outbound_capability_keeps_its_own_refusal() {
        let binding = EndpointCollaborationBinding::over(installed_store_root(
            "activate-undeclared",
            &["example.other.capability.v1"],
        ));
        let gate = EndpointCollaborationGate::new();

        assert_eq!(
            binding.activate(&gate),
            EndpointCollaborationAvailability::CapabilityUndeclared {
                version: INSTALLED_VERSION.to_owned()
            },
            "installed and switched on is not the same as declaring the capability"
        );
        assert_eq!(
            gate.authority(),
            Err(EndpointOutboundRefusal::CapabilityUndeclared)
        );
        assert_eq!(
            gate.authority().expect_err("the package refuses").reason(),
            "endpoint_collaboration_capability_undeclared",
            "an activation publishes the owner's own reason rather than laundering it"
        );
    }

    #[test]
    fn an_installed_refusal_is_authoritative_and_cuts_outbound_authority() {
        let gate = EndpointCollaborationGate::new();
        assert_eq!(
            gate.authority(),
            Ok(EndpointOutboundAuthority::LegacyInKernel)
        );

        gate.install(EndpointCollaborationAvailability::Disabled {
            version: "0.3.0".to_owned(),
        });
        assert_eq!(
            gate.authority(),
            Err(EndpointOutboundRefusal::PackageDisabled)
        );

        gate.install(EndpointCollaborationAvailability::Active {
            version: "0.3.0".to_owned(),
        });
        assert_eq!(
            gate.authority(),
            Ok(EndpointOutboundAuthority::Package {
                version: "0.3.0".to_owned()
            })
        );

        // Retiring the package the user switched off leaves the refusal, not the
        // previous grant: activation is replaced, never layered.
        let replaced = gate.install(EndpointCollaborationAvailability::Missing);
        assert_eq!(
            replaced,
            Some(EndpointCollaborationAvailability::Active {
                version: "0.3.0".to_owned()
            })
        );
        assert_eq!(
            gate.authority(),
            Err(EndpointOutboundRefusal::PackageMissing)
        );
    }

    #[test]
    fn every_resolved_state_names_exactly_one_refusal() {
        let states = [
            EndpointCollaborationAvailability::Active {
                version: "0.3.0".to_owned(),
            },
            EndpointCollaborationAvailability::Disabled {
                version: "0.3.0".to_owned(),
            },
            EndpointCollaborationAvailability::CapabilityUndeclared {
                version: "0.3.0".to_owned(),
            },
            EndpointCollaborationAvailability::Missing,
            EndpointCollaborationAvailability::Unreadable,
        ];
        let reasons = states
            .iter()
            .filter_map(EndpointOutboundRefusal::of)
            .map(EndpointOutboundRefusal::reason)
            .collect::<Vec<_>>();
        assert_eq!(reasons.len(), states.len() - 1, "only active permits");
        let mut unique = reasons.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(
            unique.len(),
            reasons.len(),
            "no two refusals share a reason"
        );
    }

    #[test]
    fn the_shipped_manifest_declares_the_identity_and_capability_the_kernel_gates_on() {
        let text = fs::read_to_string(workspace_root().join(ENDPOINT_COLLABORATION_MANIFEST_PATH))
            .expect("the shipped package manifest is readable");
        let value: serde_json::Value =
            serde_json::from_str(&text).expect("the shipped package manifest is JSON");
        let manifest =
            PackageManifest::from_value(value).expect("the manifest is a package manifest");

        assert_eq!(manifest.id, ENDPOINT_COLLABORATION_PACKAGE_ID);
        assert_eq!(manifest.version, "0.3.0");
        assert!(
            manifest.profiles.iter().any(|profile| profile
                .capabilities
                .iter()
                .any(|capability| capability == ENDPOINT_COLLABORATION_CAPABILITY_ID)),
            "the installed manifest is where the host reads the outbound capability from"
        );
        assert!(
            manifest.client_compatibility("0.3.0").is_covered(),
            "the shipped package declares the client line it was written for"
        );
        let Runtime::Declarative { descriptor } = &manifest.runtime else {
            panic!(
                "the endpoint collaboration package maps onto existing routes, it starts no program"
            );
        };
        assert!(
            workspace_root()
                .join("components/endpoint-collaboration/package")
                .join(descriptor)
                .is_file(),
            "the declarative descriptor the manifest names is shipped"
        );
        assert_eq!(
            manifest
                .extensions
                .get("org.licoland.feature.endpoint-collaboration/outboundAuthority")
                .and_then(serde_json::Value::as_str)
                .is_some(),
            true,
            "the package states its outbound authority rule where the host reads it"
        );
        assert!(
            manifest.permissions.iter().any(|permission| permission
                .capability
                .starts_with("org.licoland.feature.endpoint-collaboration/")),
            "the package requests only its own namespaced permissions"
        );
    }

    #[test]
    fn the_recovery_report_never_claims_the_local_client_is_unusable() {
        // The store-reachable states both report the local client as usable; the
        // package's own vocabulary asserts the same constant for the remaining
        // states, so no availability can claim otherwise.
        for label in ["report-empty", "report-second-empty"] {
            let binding = EndpointCollaborationBinding::over(synthetic_store_root(label));
            assert_eq!(binding.recovery_report()["state"], json!("missing"));
            assert_eq!(
                binding.recovery_report()["localClientUsable"],
                json!(true),
                "the local client never depends on this package"
            );
        }
        let file = std::env::temp_dir().join(format!(
            "licoup-endpoint-collaboration-report-unreadable-{}",
            std::process::id()
        ));
        fs::write(&file, b"not a store").expect("synthetic file is writable");
        let binding = EndpointCollaborationBinding::over(&file);
        assert_eq!(binding.recovery_report()["localClientUsable"], json!(true));
        let _ = fs::remove_file(&file);
    }
}
