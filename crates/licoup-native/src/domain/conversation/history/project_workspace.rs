//! One admission rule for a project directory recorded in agent history.
//!
//! Agent stores keep whatever working directory the agent happened to run in,
//! including residue from a turn that was launched against the filesystem root,
//! the home directory, or a personal media library. A local agent indexes the
//! directory it is started in, so such a value must never reach the client as a
//! conversation's project directory: binding it once makes the next turn walk
//! the whole personal tree and writes the same bad value back into the agent
//! store.
//!
//! `crate::platform::agent_workspace` owns the same rule for the send path.
//! This module is the read path applying that rule to history metadata, so both
//! directions accept exactly the same set of directories.

use std::path::Path;

use crate::platform::agent_workspace::is_unbounded_agent_workspace;

/// Trimmed absolute project directory, or `None` when the recorded value is
/// empty, relative, or an unbounded personal root.
pub(crate) fn bounded_project_workspace(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let path = Path::new(trimmed);
    if !path.is_absolute() {
        return None;
    }
    let home = licoup_foundation::platform::paths::user_home_from_env();
    if is_unbounded_agent_workspace(path, home.as_deref()) {
        return None;
    }
    Some(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_and_empty_records_are_rejected() {
        assert_eq!(bounded_project_workspace(""), None);
        assert_eq!(bounded_project_workspace("   "), None);
        assert_eq!(bounded_project_workspace("relative/project"), None);
    }

    #[test]
    fn the_filesystem_root_is_never_a_project_directory() {
        assert_eq!(bounded_project_workspace("/"), None);
    }

    #[test]
    fn a_concrete_project_directory_survives_trimming() {
        assert_eq!(
            bounded_project_workspace("  /fixture-root/projects/alpha  "),
            Some("/fixture-root/projects/alpha".to_string())
        );
    }
}
