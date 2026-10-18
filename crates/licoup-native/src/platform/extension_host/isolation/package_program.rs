//! The real program source: an installed package's own declared entry point.
//!
//! [`ProgramSource`] is the one seam between the host's isolation carrier and
//! *what* it starts. The reference module owns the shape and the fixture table;
//! this module is the production answer, and it reads exactly one authority: the
//! manifest the package store installed, under the managed root that store owns.
//!
//! Five rules, each a refusal rather than a fallback:
//!
//! 1. **Not installed is not startable.** The version is looked up in the
//!    [`PackageStore`] record, and an absent record is refused before any path is
//!    derived from the request. A caller cannot name a directory into existence.
//! 2. **Only a `process` runtime is a program.** A `declarative` descriptor and a
//!    `service` endpoint are host-side projections of something that already
//!    exists; starting a process for either would be inventing an execution the
//!    manifest never asked for, so both are refused by name.
//! 3. **Enabled means declared for this instance's profiles.** The instance is
//!    prepared for a published profile set; a manifest that declares none of them
//!    has not been switched on for that work, and is refused instead of being
//!    started and failing at its first call.
//! 4. **The entry is inside the installed bytes.** The declared `runtime.entry`
//!    must be a relative path of ordinary components, and what it resolves to
//!    must be a regular file the package itself carries — never a symlink, never
//!    an absolute path, never a parent traversal. The host starts what the
//!    package shipped, not what it points at.
//! 5. **No interpreter is bundled.** A `user:` runtime reference names the user's
//!    own interpreter and is resolved on this host; a host-owned runtime
//!    reference has no implementation in this build and is refused rather than
//!    emulated. A program with no runtime reference must itself be executable.
//!
//! The program's writable root is the instance's own directory inside the
//! managed root, so the confinement plan and the package layout cannot disagree:
//! both are derived from the store's own root.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use licoup_application::ApplicationFailure;
use licoup_extension_contracts::manifest::{PackageManifest, Runtime, USER_RUNTIME_PREFIX};

use crate::platform::extension_packages::{
    MANIFEST_FILE, PackageStore, ensure_private_directory, read_bounded_text, refusal,
};

use super::super::actionable;
use super::program::{ProgramSource, ResolvedProgram, program_unavailable};

/// The longest installed manifest this module will read.
const MAX_INSTALLED_MANIFEST_BYTES: usize = 256 * 1024;

/// The stage every refusal from this module names.
const STAGE: &str = "extension/isolation";

/// The directory, inside the managed root, that holds one directory per
/// instance.
const INSTANCE_DIRECTORY: &str = "instances";

/// A program source over the host's installed packages.
///
/// The source holds the store, not a second catalogue: every answer comes from
/// the record and the bytes the store already owns, so a package that was
/// removed stops resolving without this type being told.
#[derive(Clone, Debug)]
pub struct PackagePrograms {
    store: PackageStore,
    descendants: bool,
    read_roots: Vec<PathBuf>,
    env: Vec<(String, String)>,
}

impl PackagePrograms {
    pub fn new(store: PackageStore) -> Self {
        Self {
            store,
            descendants: false,
            read_roots: Vec::new(),
            env: Vec::new(),
        }
    }

    /// The store this source reads.
    pub fn store(&self) -> &PackageStore {
        &self.store
    }

    /// The managed root every derived path stays inside.
    pub fn managed_root(&self) -> &Path {
        self.store.root()
    }

    /// The instance's own writable directory inside the managed root.
    pub fn instance_root(&self, instance_id: &str) -> PathBuf {
        self.store.root().join(INSTANCE_DIRECTORY).join(instance_id)
    }

    /// Declare that this program creates descendants.
    ///
    /// A restricted instance is one process, so the carrier refuses this before
    /// anything starts; it is declared here because the decision belongs to the
    /// composition that knows the program.
    pub fn with_descendants(mut self) -> Self {
        self.descendants = true;
        self
    }

    /// Declare one readable root beyond the installed package directory, such as
    /// a user-installed interpreter's own runtime root.
    pub fn with_read_root(mut self, path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        if !self.read_roots.contains(&path) {
            self.read_roots.push(path);
        }
        self
    }

    /// Declare one environment entry for the program. The carrier keeps the
    /// variables it owns and refuses a restricted run that tries to redeclare
    /// one.
    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    /// The installed manifest behind one package version, or the refusal that
    /// says why it cannot be read.
    pub fn installed_manifest(
        &self,
        package_id: &str,
        package_version: &str,
    ) -> Result<PackageManifest, ApplicationFailure> {
        if self
            .store
            .installed_version(package_id, package_version)?
            .is_none()
        {
            return Err(program_unavailable(package_id, package_version));
        }
        let content_dir = self.store.installed_path(package_id, package_version);
        let manifest_path = content_dir.join(MANIFEST_FILE);
        let Some(text) = read_bounded_text(&manifest_path, MAX_INSTALLED_MANIFEST_BYTES)? else {
            return Err(path_refusal(
                "extension_program_manifest_unavailable",
                "manifest",
                &manifest_path,
            ));
        };
        let value: serde_json::Value = serde_json::from_str(&text).map_err(|_| {
            refusal("extension_program_manifest_invalid", STAGE).with_field("manifest")
        })?;
        PackageManifest::from_value(value)
    }

    /// The program one package version resolves to, for the profile set of one
    /// instance.
    ///
    /// This is `ProgramSource::resolve` with the two spec fields a caller outside
    /// the carrier can name; it exists so the composition can answer "would this
    /// start?" without preparing an instance.
    pub fn program_for(
        &self,
        package_id: &str,
        package_version: &str,
        instance_id: &str,
        profiles: &[String],
    ) -> Result<ResolvedProgram, ApplicationFailure> {
        resolve_program(self, package_id, package_version, instance_id, profiles)
    }
}

impl ProgramSource for PackagePrograms {
    fn resolve(
        &self,
        spec: &super::super::carrier::CarrierSpec,
    ) -> Result<ResolvedProgram, ApplicationFailure> {
        resolve_program(
            self,
            &spec.package_id,
            &spec.package_version,
            &spec.instance_id,
            &spec.profiles,
        )
    }
}

/// Rule 1 through rule 5 for one `(package, version, instance, profiles)`.
fn resolve_program(
    source: &PackagePrograms,
    package_id: &str,
    package_version: &str,
    instance_id: &str,
    profiles: &[String],
) -> Result<ResolvedProgram, ApplicationFailure> {
    let manifest = source.installed_manifest(package_id, package_version)?;
    let content_dir = source.store.installed_path(package_id, package_version);

    if !profiles.is_empty() {
        let declared: BTreeSet<&str> = manifest
            .profiles
            .iter()
            .map(|profile| profile.id.as_str())
            .collect();
        if let Some(missing) = profiles
            .iter()
            .find(|profile| !declared.contains(profile.as_str()))
        {
            return Err(
                actionable("extension_program_profile_not_declared", STAGE, "profiles")
                    .with_presentation_arg("packageId", package_id)
                    .with_presentation_arg("profile", missing),
            );
        }
    }

    let (entry, runtime_ref) = match &manifest.runtime {
        Runtime::Process { entry, runtime_ref } => (entry.as_str(), runtime_ref.as_deref()),
        other => {
            return Err(refusal("extension_program_not_process", STAGE)
                .with_field("runtime.mode")
                .with_presentation_arg("packageId", package_id)
                .with_presentation_arg("mode", other.mode()));
        }
    };

    let entry_path = declared_entry(&content_dir, entry)?;
    let mut program = match runtime_ref {
        None => {
            if !is_executable(&entry_path) {
                return Err(path_refusal(
                    "extension_program_entry_not_executable",
                    "runtime.entry",
                    &entry_path,
                ));
            }
            ResolvedProgram::new(entry_path)
        }
        Some(reference) => {
            let interpreter = user_interpreter(reference, package_id)?;
            let mut program = ResolvedProgram::new(interpreter.clone())
                .with_args([entry_path.display().to_string()])
                .with_exec_path(entry_path);
            // A user-installed runtime resolves its own installation relative to
            // where it was started from, so that one directory is readable. The
            // confinement plan still refuses a root that would contain the
            // managed root.
            if let Some(directory) = interpreter.parent() {
                program = program.with_read_root(directory.to_path_buf());
            }
            program
        }
    };
    program = program.with_read_root(content_dir);
    for root in &source.read_roots {
        program = program.with_read_root(root.clone());
    }
    // The instance's own directory is the program's writable root. It is created
    // private before the process exists, exactly as the confinement plan
    // requires, and it always stays inside the managed root the store owns.
    let write_root = source.instance_root(instance_id);
    ensure_private_directory(&write_root)?;
    program.write_root = write_root;
    program.env = source.env.clone();
    program.requires_descendants = source.descendants;
    Ok(program)
}

/// The entry a manifest declares, resolved inside the installed bytes.
fn declared_entry(content_dir: &Path, entry: &str) -> Result<PathBuf, ApplicationFailure> {
    let relative = Path::new(entry);
    let ordinary = !entry.is_empty()
        && !entry.contains('\\')
        && !entry.contains('\0')
        && !relative.is_absolute()
        && relative
            .components()
            .all(|component| matches!(component, Component::Normal(_) | Component::CurDir));
    if !ordinary {
        return Err(refusal("extension_program_entry_unavailable", STAGE)
            .with_field("runtime.entry")
            .with_presentation_arg("entry", entry));
    }
    let resolved = content_dir.join(relative);
    match std::fs::symlink_metadata(&resolved) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => Ok(resolved),
        _ => Err(path_refusal(
            "extension_program_entry_unavailable",
            "runtime.entry",
            &resolved,
        )),
    }
}

/// The interpreter a `user:` reference names, resolved on this host.
///
/// A reference the host would have to own is refused: this build bundles no
/// language runtime, and starting the entry with the wrong interpreter would be a
/// quieter failure than saying so.
fn user_interpreter(reference: &str, package_id: &str) -> Result<PathBuf, ApplicationFailure> {
    let Some(name) = reference.strip_prefix(USER_RUNTIME_PREFIX) else {
        return Err(runtime_refusal(package_id, reference));
    };
    let candidate = Path::new(name);
    let resolved = if candidate.is_absolute() {
        Some(candidate.to_path_buf())
    } else {
        search_path(name)
    };
    match resolved {
        Some(path) if path.is_file() && is_executable(&path) => Ok(path),
        _ => Err(runtime_refusal(package_id, reference)),
    }
}

fn runtime_refusal(package_id: &str, reference: &str) -> ApplicationFailure {
    refusal("extension_program_runtime_unavailable", STAGE)
        .with_field("runtime.runtimeRef")
        .with_presentation_arg("packageId", package_id)
        .with_presentation_arg("runtimeRef", reference)
}

/// Resolve a bare program name against the host `PATH`.
fn search_path(name: &str) -> Option<PathBuf> {
    if name.is_empty() || name.contains('/') {
        return None;
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file() && is_executable(candidate))
}

/// Whether this host may execute the file directly.
fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).is_ok_and(|metadata| metadata.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

fn path_refusal(code: &str, field: &str, path: &Path) -> ApplicationFailure {
    refusal(code, STAGE)
        .with_field(field)
        .with_presentation_arg("path", &path.display().to_string())
}
