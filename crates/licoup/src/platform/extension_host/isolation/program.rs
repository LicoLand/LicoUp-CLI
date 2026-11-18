//! Turning an instance spec into the program to run.
//!
//! The host prepares an instance from a package, a version, a generation and a
//! profile set ([`CarrierSpec`]); the *program* — the executable, its arguments
//! and the runtime roots it may read — is resolved here. The composition owns
//! that resolution: the real one resolves the installed package's manifest
//! `runtime.entry` (and its `runtimeRef` interpreter) inside the managed root.
//! This module owns the shape, the fixture source, and the refusal for a package
//! with no runnable program.
//!
//! Nothing here bundles an interpreter. A package that needs Python, Node or a
//! shell names it through [`ResolvedProgram::exec_paths`]; the core ships no
//! language runtime of its own, and an absent interpreter stays absent instead
//! of being emulated.

use std::collections::BTreeMap;
use std::path::PathBuf;

use licoup_application::ApplicationFailure;

use crate::platform::extension_host::carrier::CarrierSpec;

use super::super::actionable;

/// One process the host can run for an instance.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedProgram {
    /// The program to execute. Canonicalized and validated before any spawn.
    pub executable: PathBuf,
    pub args: Vec<String>,
    /// Every binary this program may execute, including `executable` itself.
    /// A runtime that re-execs a helper (a framework `Python.app`, a shim)
    /// declares it here; nothing outside this list may be executed under
    /// restricted mode.
    pub exec_paths: Vec<PathBuf>,
    /// Environment entries the package itself declares. They are added on top of
    /// a minimal, scrubbed environment — which is never the host's full
    /// environment — so a credential in the host process is not inherited by
    /// accident.
    pub env: Vec<(String, String)>,
    /// Readable roots for restricted mode. The composition declares the package
    /// directory and the interpreter's runtime root; declaring a user home or an
    /// ancestor of the managed root is refused.
    pub read_roots: Vec<PathBuf>,
    /// The instance's writable directory, inside the managed root.
    pub write_root: PathBuf,
    /// The pinned working directory; defaults to the write root.
    pub working_directory: Option<PathBuf>,
    /// Whether this program needs to create descendants (subprocesses).
    ///
    /// A restricted instance is a single process by construction; a program that
    /// declares this cannot run restricted and is refused before anything is
    /// created, instead of failing at its first `fork`.
    pub requires_descendants: bool,
}

impl ResolvedProgram {
    pub fn new(executable: impl Into<PathBuf>) -> Self {
        let executable = executable.into();
        Self {
            exec_paths: vec![executable.clone()],
            executable,
            args: Vec::new(),
            env: Vec::new(),
            read_roots: Vec::new(),
            write_root: PathBuf::new(),
            working_directory: None,
            requires_descendants: false,
        }
    }

    pub fn with_args(mut self, args: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.args = args.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_exec_path(mut self, path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        if !self.exec_paths.contains(&path) {
            self.exec_paths.push(path);
        }
        self
    }

    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    pub fn with_read_root(mut self, path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        if !self.read_roots.contains(&path) {
            self.read_roots.push(path);
        }
        self
    }

    pub fn with_write_root(mut self, path: impl Into<PathBuf>) -> Self {
        self.write_root = path.into();
        self
    }

    pub fn with_working_directory(mut self, path: impl Into<PathBuf>) -> Self {
        self.working_directory = Some(path.into());
        self
    }

    /// Declare that this program creates descendants. Restricted mode refuses
    /// it; trusted local mode allows it with the scoped release that implies.
    pub fn with_requires_descendants(mut self) -> Self {
        self.requires_descendants = true;
        self
    }
}

/// How the composition maps an instance to a runnable program.
pub trait ProgramSource: Send + Sync {
    fn resolve(&self, spec: &CarrierSpec) -> Result<ResolvedProgram, ApplicationFailure>;
}

/// A source over an explicit `(package, version)` table.
///
/// Used by fixtures and by a composition that has already resolved installed
/// packages itself; it holds no second package catalogue.
#[derive(Default)]
pub struct StaticPrograms {
    programs: BTreeMap<(String, String), ResolvedProgram>,
}

impl StaticPrograms {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(
        &mut self,
        package_id: impl Into<String>,
        package_version: impl Into<String>,
        program: ResolvedProgram,
    ) {
        self.programs
            .insert((package_id.into(), package_version.into()), program);
    }

    pub fn len(&self) -> usize {
        self.programs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.programs.is_empty()
    }
}

impl ProgramSource for StaticPrograms {
    fn resolve(&self, spec: &CarrierSpec) -> Result<ResolvedProgram, ApplicationFailure> {
        self.programs
            .get(&(spec.package_id.clone(), spec.package_version.clone()))
            .cloned()
            .ok_or_else(|| program_unavailable(&spec.package_id, &spec.package_version))
    }
}

/// The refusal for a package version with no runnable program.
pub(crate) fn program_unavailable(package_id: &str, package_version: &str) -> ApplicationFailure {
    actionable(
        "extension_program_unavailable",
        "extension/isolation",
        "packageId",
    )
    .with_presentation_arg("packageId", package_id)
    .with_presentation_arg("packageVersion", package_version)
}
