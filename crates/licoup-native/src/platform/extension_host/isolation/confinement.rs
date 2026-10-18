//! The confinement plan: which roots a confined process may read and write,
//! which binaries it may execute, and how that becomes a real OS profile.
//!
//! Validation happens before a process exists, and it is strict on purpose:
//!
//! - Every root is absolute, canonical and free of characters that cannot be
//!   represented in a seatbelt profile.
//! - A read root may not be an ancestor of the managed root (declaring `/` or a
//!   user home as readable is not a confinement plan), and the executable may
//!   not live inside the writable root.
//! - The write root must be a directory *inside* the managed root and is created
//!   private before the process starts.
//! - A restricted program may declare its own environment, but not the
//!   variables the host owns: where its home, temporary directory and working
//!   directory are, which path it resolves tools from, and which loader or
//!   interpreter control variables are set. Declaring one is refused before
//!   anything is created, instead of being ignored or silently overridden. A
//!   trusted local program (the user's own software) may declare them; that is
//!   the declared difference between the two modes.
//! - A restricted program may not declare that it needs descendants: the profile
//!   denies `process-fork` (which covers fork, vfork, `posix_spawn` and
//!   everything built on them), so the instance stays one process that the
//!   supervised wait can account for completely. Threads are unaffected.
//!
//! The macOS profile is built from canonical paths because seatbelt matches the
//! resolved filesystem path, not the symlinked spelling (`/tmp` is
//! `/private/tmp`). It denies everything by default, denies network, and then
//! allows exactly: executing the declared binaries, reading the declared roots,
//! metadata on the path chain (so a runtime may resolve its own location without
//! reading user data), and writing the instance's own root. A loopback grant
//! adds outbound TCP to one local port.

use std::path::{Path, PathBuf};
use std::process::Command;

use licoup_application::ApplicationFailure;

use super::super::{refusal, uncertain};
use super::capability::{IsolationMode, PlatformConfinement, Support};
use super::declaration::NetworkGrant;
use super::program::ResolvedProgram;

/// Whether `/usr/bin/sandbox-exec` is present and trustworthy.
///
/// The same check the existing strategy profile uses: a root-owned, non-symlink,
/// not-group-or-world-writable file. A binary that fails this is not a mechanism
/// this build will claim.
#[cfg(target_os = "macos")]
pub(crate) fn sandbox_exec_is_trusted() -> bool {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";
    match std::fs::symlink_metadata(SANDBOX_EXEC) {
        Ok(metadata) => {
            metadata.is_file()
                && !metadata.file_type().is_symlink()
                && metadata.uid() == 0
                && metadata.permissions().mode() & 0o022 == 0
        }
        Err(_) => false,
    }
}

/// The refusal for a restricted run on a host without the mechanism.
pub(crate) fn confinement_unavailable(confinement: &PlatformConfinement) -> ApplicationFailure {
    let reason = confinement
        .filesystem_scopes
        .reason()
        .unwrap_or("no filesystem confinement is available on this host");
    refusal("extension_isolation_unavailable", "extension/isolation")
        .with_field("mode")
        .with_presentation_arg("mode", IsolationMode::Restricted.id())
        .with_presentation_arg("reason", reason)
}

/// Variables the host owns for a restricted instance.
///
/// The granted envelope decides where the instance lives; a third-party program
/// does not get to redeclare that, and it does not get to steer the loader or
/// the interpreter that the host chose to start it with. Trusted local programs
/// are the user's own software and are not held to this list.
const RESTRICTED_RESERVED_VARIABLES: &[&str] = &[
    "HOME",
    "TMPDIR",
    "TMP",
    "TEMP",
    "PWD",
    "PATH",
    "PATHEXT",
    "SYSTEMROOT",
    "LD_PRELOAD",
    "LD_LIBRARY_PATH",
    "LD_AUDIT",
    "LD_DEBUG",
    "DYLD_INSERT_LIBRARIES",
    "DYLD_LIBRARY_PATH",
    "DYLD_FRAMEWORK_PATH",
    "DYLD_FALLBACK_LIBRARY_PATH",
    "DYLD_PRINT_LIBRARIES",
    "PYTHONHOME",
    "PYTHONPATH",
    "PYTHONSTARTUP",
    "PYTHONEXECUTABLE",
    "NODE_OPTIONS",
    "NODE_PATH",
    "PERL5LIB",
    "PERL5OPT",
    "RUBYOPT",
    "RUBYLIB",
    "JAVA_TOOL_OPTIONS",
    "_JAVA_OPTIONS",
    "CLASSPATH",
];

/// Whether a declared variable is host-owned in restricted mode.
///
/// The exact list above plus the loader namespaces (`LD_*`, `DYLD_*`), which are
/// prefixes no fixed list can enumerate.
pub(crate) fn reserved_restricted_variable(key: &str) -> bool {
    let upper = key.to_ascii_uppercase();
    RESTRICTED_RESERVED_VARIABLES
        .iter()
        .any(|reserved| upper == *reserved)
        || upper.starts_with("LD_")
        || upper.starts_with("DYLD_")
}

/// The refusal for a declaration that tries to redeclare a host-owned variable.
pub(crate) fn reserved_env_refusal(key: &str, mode: IsolationMode) -> ApplicationFailure {
    refusal("extension_isolation_env_reserved", "extension/isolation")
        .with_field("env")
        .with_presentation_arg("variable", key)
        .with_presentation_arg("mode", mode.id())
}

/// The roots and executables one confined process really gets.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ValidatedProgram {
    pub executable: PathBuf,
    pub args: Vec<String>,
    pub exec_paths: Vec<PathBuf>,
    pub env: Vec<(String, String)>,
    pub read_roots: Vec<PathBuf>,
    pub write_root: PathBuf,
    pub working_directory: PathBuf,
}

fn invalid(field: &str) -> ApplicationFailure {
    refusal("extension_isolation_path_invalid", "extension/isolation").with_field(field)
}

fn out_of_bounds(field: &str, path: &Path) -> ApplicationFailure {
    refusal(
        "extension_isolation_root_out_of_bounds",
        "extension/isolation",
    )
    .with_field(field)
    .with_presentation_arg("path", &path.display().to_string())
}

/// What a declared path has to be.
#[derive(Clone, Copy, Eq, PartialEq)]
enum Expect {
    Any,
    Directory,
    File,
}

/// Canonicalize one declared path, refusing anything that is not a real,
/// representable path of the expected kind.
fn canonical(path: &Path, field: &str, expect: Expect) -> Result<PathBuf, ApplicationFailure> {
    if !path.is_absolute() {
        return Err(invalid(field));
    }
    let Some(text) = path.to_str() else {
        return Err(invalid(field));
    };
    if text.is_empty() || text.chars().any(char::is_control) {
        return Err(invalid(field));
    }
    let resolved = std::fs::canonicalize(path).map_err(|_| invalid(field))?;
    let metadata = std::fs::metadata(&resolved).map_err(|_| invalid(field))?;
    match expect {
        Expect::Directory if !metadata.is_dir() => return Err(invalid(field)),
        Expect::File if !metadata.is_file() => return Err(invalid(field)),
        Expect::Any => {}
        _ => {}
    }
    Ok(resolved)
}

/// Validate one program for one mode against the managed root and return its
/// real envelope.
///
/// This is the single gate before a process exists: a declaration that cannot
/// be honoured is refused here, so no later stage has to weaken one silently.
pub(crate) fn validate_program(
    mode: IsolationMode,
    managed_root: &Path,
    program: &ResolvedProgram,
) -> Result<ValidatedProgram, ApplicationFailure> {
    if mode == IsolationMode::Restricted {
        // A restricted instance is one process; a program that needs children is
        // refused here rather than failing at its first spawn.
        if program.requires_descendants {
            return Err(refusal(
                "extension_isolation_descendants_unsupported",
                "extension/isolation",
            )
            .with_field("requiresDescendants")
            .with_presentation_arg("mode", mode.id()));
        }
        for (key, _) in &program.env {
            if reserved_restricted_variable(key) {
                return Err(reserved_env_refusal(key, mode));
            }
        }
    }
    let root = canonical(managed_root, "managedRoot", Expect::Directory)?;

    let executable = canonical(&program.executable, "executable", Expect::File)?;
    let mut exec_paths = vec![executable.clone()];
    for declared in &program.exec_paths {
        let resolved = canonical(declared, "execPath", Expect::File)?;
        if !exec_paths.contains(&resolved) {
            exec_paths.push(resolved);
        }
    }

    let mut read_roots: Vec<PathBuf> = Vec::new();
    for declared in &program.read_roots {
        let resolved = canonical(declared, "readRoot", Expect::Any)?;
        // A root that contains the managed root would make the confinement
        // plan meaningless; the managed root itself is allowed.
        if resolved != root && root.starts_with(&resolved) {
            return Err(out_of_bounds("readRoot", &resolved));
        }
        if !read_roots.contains(&resolved) {
            read_roots.push(resolved);
        }
    }

    // The write root is the instance's own directory inside the managed root;
    // it is created private before the process exists. It is resolved through
    // its existing parent first, so a refused declaration never creates a
    // directory outside the root.
    if !program.write_root.is_absolute() {
        return Err(invalid("writeRoot"));
    }
    let write_root = match std::fs::canonicalize(&program.write_root) {
        Ok(path) => path,
        Err(_) => {
            let parent = program
                .write_root
                .parent()
                .ok_or_else(|| invalid("writeRoot"))?;
            let name = program
                .write_root
                .file_name()
                .ok_or_else(|| invalid("writeRoot"))?;
            canonical(parent, "writeRoot", Expect::Directory)?.join(name)
        }
    };
    if write_root == root || !write_root.starts_with(&root) {
        return Err(out_of_bounds("writeRoot", &write_root));
    }
    crate::platform::extension_packages::ensure_private_directory(&write_root)?;
    let write_root = canonical(&write_root, "writeRoot", Expect::Directory)?;
    for exec_path in &exec_paths {
        if exec_path.starts_with(&write_root) {
            return Err(out_of_bounds("execPath", exec_path));
        }
    }

    let working_directory = match &program.working_directory {
        Some(directory) => {
            let resolved = canonical(directory, "workingDirectory", Expect::Directory)?;
            if resolved != write_root && !resolved.starts_with(&write_root) {
                return Err(out_of_bounds("workingDirectory", &resolved));
            }
            resolved
        }
        None => write_root.clone(),
    };

    Ok(ValidatedProgram {
        executable,
        args: program.args.clone(),
        exec_paths,
        env: program.env.clone(),
        read_roots,
        write_root,
        working_directory,
    })
}

/// Build the command for one instance and report what confinement it carries.
pub(crate) fn build_command(
    mode: IsolationMode,
    network: NetworkGrant,
    program: &ValidatedProgram,
    confinement: &PlatformConfinement,
) -> Result<(Command, Support), ApplicationFailure> {
    match mode {
        IsolationMode::TrustedLocal => {
            let mut command = Command::new(&program.executable);
            command.args(&program.args);
            Ok((
                command,
                Support::unavailable("trusted local program runs without OS confinement"),
            ))
        }
        IsolationMode::Restricted => {
            #[cfg(target_os = "macos")]
            {
                if !confinement.supports_restricted() {
                    return Err(confinement_unavailable(confinement));
                }
                let profile = seatbelt_profile(network, program)?;
                let mut command = Command::new("/usr/bin/sandbox-exec");
                command.args(["-p", &profile]).arg(&program.executable);
                command.args(&program.args);
                Ok((command, Support::enforced("macos-seatbelt")))
            }
            #[cfg(not(target_os = "macos"))]
            {
                let _ = (network, program);
                Err(confinement_unavailable(confinement))
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn seatbelt_profile(
    network: NetworkGrant,
    program: &ValidatedProgram,
) -> Result<String, ApplicationFailure> {
    use crate::platform::process_sandbox::{SandboxError, seatbelt_literal};

    let literal = |path: &Path| -> Result<String, ApplicationFailure> {
        seatbelt_literal(path).map_err(|error| match error {
            SandboxError::PathInvalid => invalid("path"),
            SandboxError::Unavailable => {
                refusal("extension_isolation_unavailable", "extension/isolation").with_field("path")
            }
        })
    };

    let execs = program
        .exec_paths
        .iter()
        .map(|path| literal(path))
        .collect::<Result<Vec<_>, _>>()?;
    let reads = program
        .read_roots
        .iter()
        .map(|path| literal(path))
        .collect::<Result<Vec<_>, _>>()?;
    let write = literal(&program.write_root)?;

    let mut profile =
        String::from("(version 1)(deny default)(import \"system.sb\")(deny network*)");
    // A restricted instance is one process. `process-fork` denied covers fork,
    // vfork, posix_spawn and everything built on them (measured on this host),
    // so no descendant can exist that the supervised wait would not account
    // for, and the per-process limits really bound the instance. Threads live
    // inside this process and stay allowed.
    profile.push_str("(deny process-fork)");
    profile.push_str("(allow process-exec");
    for path in &execs {
        profile.push_str(&format!(" (literal \"{path}\")"));
    }
    profile.push_str(")(allow signal (target self))");
    // Metadata on the path chain lets a runtime resolve its own binary without
    // granting it the contents of paths outside the declared roots.
    profile.push_str("(allow file-read-metadata (subpath \"/\"))");
    profile.push_str("(allow file-read* file-test-existence");
    for path in &execs {
        profile.push_str(&format!(" (literal \"{path}\")"));
    }
    for path in &reads {
        profile.push_str(&format!(" (subpath \"{path}\")"));
    }
    profile.push_str(&format!(")(allow file-write* (subpath \"{write}\"))"));
    if let NetworkGrant::Loopback { port } = network {
        profile.push_str(&format!(
            "(allow network-outbound (remote tcp \"localhost:{port}\"))"
        ));
    }
    Ok(profile)
}

/// The refusal for a release whose process death was not observed.
pub(crate) fn release_unconfirmed(detail: &str) -> ApplicationFailure {
    uncertain(
        "extension_isolation_release_unconfirmed",
        "extension/isolation",
    )
    .with_presentation_arg("detail", detail)
}
