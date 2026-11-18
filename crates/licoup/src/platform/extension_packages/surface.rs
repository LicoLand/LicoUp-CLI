//! The declared surface of one installed optional package — and the uninstall
//! that may only ever claim it.
//!
//! The store in [`super::install`] owns bytes, records, staging and the install
//! journal; it deliberately knows nothing about what a package *is*. This module
//! closes that gap from the package's own manifest, which is the only document
//! that says what it contributes and how it is carried:
//!
//! - [`PackageSurface::register`] reads the manifest the store actually
//!   installed and turns it into the resources that package holds: one entry per
//!   declared interface contribution, and the entry it declares — a process
//!   runtime the host starts, or a declarative descriptor the host maps onto an
//!   existing route. Both are the `declared_entry` the installer already resolves
//!   the payload by, so the two owners cannot disagree about what a package holds.
//! - [`PackageSurface::release`] releases a resource **only** when that manifest
//!   declared it. An identity the package did not declare is refused
//!   (`package_surface_resource_not_owned`), and one it declared but already
//!   released is refused separately (`package_surface_resource_released`). A
//!   package therefore cannot claim completion for another package's
//!   contribution, for a page the user closed, or for the kernel's own ledger.
//! - [`uninstall`] runs the whole removal through the store's one transaction —
//!   withdraw admission, drain, then collect — and only then reports the surface
//!   released. There is no second installer and no second removal path: the bytes
//!   are reclaimed by [`Drained::collect`], the same call the package screen uses.
//!
//! Two facts stay outside this module on purpose. The kernel's usage facts, its
//! running work and its journal are not resources any package declares, so no
//! report produced here can name them; and closing a surface
//! ([`close_surface`](super::close_surface)) is not an uninstall, so it releases
//! nothing.

use crate::platform::extension_packages::install::{InstalledPackage, PackageStore};
use crate::platform::extension_packages::refusal;
use crate::platform::extension_packages::registration::RegistrationOwners;
use crate::platform::extension_packages::state::InstanceRegistry;
use crate::platform::extension_packages::uninstall::{
    DependentsDecision, PreservedFacts, RemainingWork, UninstallOutcome, UninstallTransaction,
    preview,
};
use licoup_application::ApplicationFailure;
use licoup_extension_contracts::deployment::LocalCatalogue;
use licoup_extension_contracts::manifest::PackageManifest;
use licoup_extension_contracts::ui::ContributionKind;

const SURFACE_STAGE: &str = "extension/package-surface";

/// One resource an installed package declared and holds.
///
/// The variants are the two declarations a manifest can carry today. A resource
/// this package owns is never inferred from another package's manifest, from a
/// path on disk or from a running process: it is read from the installed
/// package's own document.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SurfaceResource {
    /// One declarative interface contribution, by namespaced identity.
    Contribution { id: String, kind: ContributionKind },
    /// The entry this package declares: the program the host starts for it, or
    /// the descriptor it maps onto an existing route.
    Runtime { entry: String },
}

impl SurfaceResource {
    /// The identity a caller releases and a report names.
    pub fn identity(&self) -> &str {
        match self {
            Self::Contribution { id, .. } => id,
            Self::Runtime { entry } => entry,
        }
    }

    /// The kind of resource, as the report spells it.
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Contribution { .. } => "contribution",
            Self::Runtime { .. } => "runtime",
        }
    }
}

/// The resources one installed package version declared, and still holds.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PackageSurface {
    package_id: String,
    version: String,
    declared: Vec<SurfaceResource>,
    held: Vec<SurfaceResource>,
}

impl PackageSurface {
    /// Register the surface of one installed version from the manifest the store
    /// published.
    ///
    /// The manifest is read back from the installed content rather than from the
    /// record the host wrote about it, so the surface is the package's own
    /// declaration. A package that declares no resource at all is refused rather
    /// than registered as an empty surface: there would be nothing to release,
    /// and "removed nothing" must not be reportable as a completed removal.
    pub fn register(
        store: &PackageStore,
        installed: &InstalledPackage,
    ) -> Result<Self, ApplicationFailure> {
        let manifest = store.installed_manifest(&installed.package_id, &installed.version)?;
        Self::from_manifest(&manifest, &installed.version)
    }

    /// Register the surface one manifest declares.
    fn from_manifest(
        manifest: &PackageManifest,
        version: &str,
    ) -> Result<Self, ApplicationFailure> {
        if manifest.id.is_empty() {
            return Err(refusal("package_surface_identity_missing", SURFACE_STAGE)
                .with_field("manifest.id"));
        }
        let mut declared = Vec::with_capacity(manifest.contributions.len() + 1);
        for contribution in &manifest.contributions {
            declared.push(SurfaceResource::Contribution {
                id: contribution.id.clone(),
                kind: contribution.kind,
            });
        }
        if let Some(entry) = super::artifact::declared_entry(manifest) {
            declared.push(SurfaceResource::Runtime {
                entry: entry.to_owned(),
            });
        }
        if declared.is_empty() {
            return Err(refusal("package_surface_empty", SURFACE_STAGE)
                .with_field("manifest")
                .with_presentation_arg("package", manifest.id.as_str()));
        }
        Ok(Self {
            package_id: manifest.id.clone(),
            version: version.to_owned(),
            held: declared.clone(),
            declared,
        })
    }

    pub fn package_id(&self) -> &str {
        &self.package_id
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    /// Everything the installed manifest declared.
    pub fn declared(&self) -> &[SurfaceResource] {
        &self.declared
    }

    /// What is still held: the declared set minus what has been released.
    pub fn held(&self) -> &[SurfaceResource] {
        &self.held
    }

    /// Release one resource this package declared.
    ///
    /// Refuses, without changing anything, an identity this package did not
    /// declare. That refusal is the whole point of the type: it is what keeps an
    /// uninstall report from claiming a resource that belongs to another package,
    /// to the kernel or to the user.
    pub fn release(&mut self, identity: &str) -> Result<SurfaceResource, ApplicationFailure> {
        let Some(index) = self
            .held
            .iter()
            .position(|resource| resource.identity() == identity)
        else {
            if self
                .declared
                .iter()
                .any(|resource| resource.identity() == identity)
            {
                return Err(refusal("package_surface_resource_released", SURFACE_STAGE)
                    .with_field("resource")
                    .with_presentation_arg("resource", identity));
            }
            return Err(refusal("package_surface_resource_not_owned", SURFACE_STAGE)
                .with_field("resource")
                .with_presentation_arg("resource", identity)
                .with_presentation_arg("package", self.package_id.as_str()));
        };
        Ok(self.held.remove(index))
    }

    /// Release everything still held, in declaration order.
    pub fn release_all(&mut self) -> Vec<SurfaceResource> {
        std::mem::take(&mut self.held)
    }
}

/// What one package uninstall released, besides the bytes the store reclaimed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SurfaceUninstall {
    /// The store's own report: bytes, drained instances, canceled work.
    pub outcome: UninstallOutcome,
    /// The declared resources released, exactly.
    pub released: Vec<SurfaceResource>,
}

impl SurfaceUninstall {
    /// The facts the uninstall kept. They come from the store's plan, which
    /// states them as a value rather than assuming them.
    pub fn preserved(&self) -> PreservedFacts {
        self.outcome.preserved
    }

    /// Whether the uninstall released the whole declared surface and nothing
    /// else. It cannot be false for a value this module produced; it exists so a
    /// caller can assert the property instead of trusting the prose.
    pub fn released_exactly(&self, surface: &PackageSurface) -> bool {
        self.released == surface.declared
    }
}

/// Uninstall one installed package through the store's transaction.
///
/// The order is the store's and is not repeated here: [`UninstallTransaction::begin`]
/// closes admission on this package's instances, [`drain`](UninstallTransaction::drain)
/// moves them to stopped — waiting for in-flight work or settling it as `Unknown`
/// when the caller cancels — and only a drained transaction can
/// [collect](crate::platform::extension_packages::Drained::collect) the bytes.
///
/// A package that other installed packages require is refused with its
/// dependents named, because this call removes what it was asked for and never
/// cascades.
///
/// The surface is released only after the bytes are reclaimed: a refused or
/// interrupted transaction leaves both the package and its registered surface
/// exactly as they were, so a retry starts from the same declaration.
///
/// [owners] carries the production per-owner release: the registration the
/// package recorded is withdrawn by the module that owns its surface, and the
/// bytes are only reclaimed once that withdrawal succeeded.
pub fn uninstall_package(
    store: &PackageStore,
    registry: &mut InstanceRegistry,
    catalogue: &LocalCatalogue,
    surface: &mut PackageSurface,
    remaining: RemainingWork,
    owners: &dyn RegistrationOwners,
) -> Result<SurfaceUninstall, ApplicationFailure> {
    let Some(installed) = store.installed_version(surface.package_id(), surface.version())? else {
        return Err(refusal("package_not_installed", SURFACE_STAGE)
            .with_field("packageId")
            .with_presentation_arg("package", surface.package_id()));
    };
    let plan = preview(store, catalogue, &installed, registry)?;
    let transaction =
        UninstallTransaction::begin(registry, plan, DependentsDecision::SelectedOnly)?;
    let drained = transaction.drain(registry, remaining)?;
    let outcome = drained.collect(store, registry, owners)?;
    let released = surface.release_all();
    Ok(SurfaceUninstall { outcome, released })
}

/// The code a surface refusal reports, for callers that branch on it without
/// matching text.
pub const RESOURCE_NOT_OWNED: &str = "package_surface_resource_not_owned";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::extension_packages::artifact::content_digest;
    use crate::platform::extension_packages::install::InstallRequest;
    use crate::platform::extension_packages::state::TrustRecord;
    use crate::platform::extension_packages::unique_suffix;
    use licoup_extension_contracts::deployment::PackageSource;
    use licoup_extension_contracts::wire;
    use std::io::Write;
    use std::path::PathBuf;

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

    /// One package archive carrying its manifest and the entry the manifest
    /// declares. The host refuses a package whose declared entry it does not
    /// ship, so the fixture ships it.
    fn package_bytes(manifest: serde_json::Value) -> Vec<u8> {
        let entry = manifest["runtime"]
            .get("entry")
            .or_else(|| manifest["runtime"].get("descriptor"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        writer.start_file("manifest.json", options).expect("entry");
        writer
            .write_all(manifest.to_string().as_bytes())
            .expect("manifest");
        if let Some(entry) = entry {
            writer.start_file(entry, options).expect("entry");
            writer.write_all(b"staged entry\n").expect("entry");
        }
        writer.finish().expect("finish").into_inner()
    }

    fn install_manifest(
        store: &PackageStore,
        id: &str,
        manifest: serde_json::Value,
    ) -> InstalledPackage {
        let bytes = package_bytes(manifest.clone());
        // The user's decision covers exactly what the package declares.
        let permissions: Vec<licoup_extension_contracts::manifest::PermissionRequest> =
            serde_json::from_value(manifest["permissions"].clone()).expect("permissions");
        let trust =
            TrustRecord::local_approved(content_digest(&bytes), permissions).expect("trust");
        store
            .install(
                &InstallRequest::new(id, "1.0.0", PackageSource::LocalImport, trust),
                &bytes,
            )
            .expect("install")
            .installed
    }

    fn manifest_with(
        runtime: serde_json::Value,
        contributions: serde_json::Value,
    ) -> serde_json::Value {
        serde_json::json!({
            "schema": wire::MANIFEST,
            "id": "example.optional.surface",
            "version": "1.0.0",
            "displayName": "Surface fixture",
            "hostProtocol": { "major": 1, "minimumMinor": 0 },
            "compatibility": { "clientVersions": covering_client_versions() },
            "profiles": [],
            "runtime": runtime,
            "activation": "on-demand",
            "requires": [],
            "optionalRequires": [],
            "permissions": [],
            "contributions": contributions,
        })
    }

    fn store(tag: &str) -> (PathBuf, PackageStore) {
        let root =
            std::env::temp_dir().join(format!("licoup-pkg-surface-{tag}-{}", unique_suffix()));
        let store = PackageStore::open(&root).expect("store");
        (root, store)
    }

    #[test]
    fn the_surface_is_the_manifest_the_store_installed() {
        let (root, store) = store("register");
        let installed = install_manifest(
            &store,
            "example.optional.surface",
            manifest_with(
                serde_json::json!({ "mode": "process", "entry": "bin/surface" }),
                serde_json::json!([
                    { "kind": "metric-panel", "id": "example.optional.surface/panel", "definition": "p.json" }
                ]),
            ),
        );
        let surface = PackageSurface::register(&store, &installed).expect("registered");
        assert_eq!(surface.package_id(), "example.optional.surface");
        assert_eq!(surface.version(), "1.0.0");
        assert_eq!(
            surface
                .declared()
                .iter()
                .map(SurfaceResource::identity)
                .collect::<Vec<_>>(),
            ["example.optional.surface/panel", "bin/surface"]
        );
        assert_eq!(surface.held(), surface.declared());
        crate::platform::extension_packages::remove_managed_tree(&root).expect("cleanup");
    }

    #[test]
    fn a_resource_this_package_did_not_declare_is_refused() {
        let (root, store) = store("not-owned");
        let installed = install_manifest(
            &store,
            "example.optional.surface",
            manifest_with(
                serde_json::json!({ "mode": "process", "entry": "bin/surface" }),
                serde_json::json!([
                    { "kind": "metric-panel", "id": "example.optional.surface/panel", "definition": "p.json" }
                ]),
            ),
        );
        let mut surface = PackageSurface::register(&store, &installed).expect("registered");

        for foreign in ["example.other/panel", "bin/other", "licoup.tokens.input"] {
            let failure = surface.release(foreign).expect_err("not this package's");
            assert_eq!(failure.code, RESOURCE_NOT_OWNED);
            assert_eq!(failure.presentation_args.get("resource"), Some(foreign));
        }
        assert_eq!(surface.held().len(), 2, "a refused release changes nothing");

        surface
            .release("example.optional.surface/panel")
            .expect("declared");
        let failure = surface
            .release("example.optional.surface/panel")
            .expect_err("released once");
        assert_eq!(failure.code, "package_surface_resource_released");
        assert_eq!(surface.held().len(), 1);
        crate::platform::extension_packages::remove_managed_tree(&root).expect("cleanup");
    }

    #[test]
    fn a_declarative_package_declares_the_descriptor_it_holds() {
        // A declarative package holds its descriptor: it is the entry the
        // installer resolves the payload by, so the surface releases it rather
        // than reporting that this package declared nothing.
        let (root, store) = store("declarative");
        let installed = install_manifest(
            &store,
            "example.optional.surface",
            manifest_with(
                serde_json::json!({
                    "mode": "declarative",
                    "descriptor": "contributions/control-surface.json"
                }),
                serde_json::json!([]),
            ),
        );
        let mut surface = PackageSurface::register(&store, &installed).expect("registered");
        assert_eq!(
            surface
                .declared()
                .iter()
                .map(SurfaceResource::identity)
                .collect::<Vec<_>>(),
            ["contributions/control-surface.json"]
        );
        assert_eq!(surface.declared()[0].kind(), "runtime");
        assert_eq!(surface.held(), surface.declared());

        surface
            .release("contributions/control-surface.json")
            .expect("the descriptor is this package's own");
        assert!(surface.held().is_empty());
        crate::platform::extension_packages::remove_managed_tree(&root).expect("cleanup");
    }

    #[test]
    fn a_package_that_declares_nothing_has_no_surface_to_remove() {
        // A service package is bridged rather than carried: it names an endpoint
        // the user configured and resolves no entry the host holds, so a manifest
        // with no contribution beside it declares nothing at all.
        let (root, store) = store("empty");
        let installed = install_manifest(
            &store,
            "example.optional.surface",
            manifest_with(
                serde_json::json!({ "mode": "service", "endpointRef": "unix:///tmp/synthetic-surface/surface.sock" }),
                serde_json::json!([]),
            ),
        );
        let failure = PackageSurface::register(&store, &installed).expect_err("nothing declared");
        assert_eq!(failure.code, "package_surface_empty");
        assert_eq!(
            failure.presentation_args.get("package"),
            Some("example.optional.surface")
        );
        crate::platform::extension_packages::remove_managed_tree(&root).expect("cleanup");
    }

    #[test]
    fn a_closed_page_is_not_an_uninstall_and_releases_nothing() {
        let closure =
            crate::platform::extension_packages::close_surface("example.optional.surface");
        assert!(closure.package_still_installed);
        assert_eq!(closure.instances_changed, 0);
    }
}
