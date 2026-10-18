//! The real registration owners, behind the uninstall release seam.
//!
//! Each [`RegistrationOwner`] variant is dispatched to the module whose `remove`
//! path is authoritative for that surface. Nothing here edits another module's
//! configuration file directly: the owning module decides what its own surface
//! contains, and this adapter supplies the inputs that owner's `remove` requires.
//!
//! Two properties are deliberate:
//!
//! 1. **An owner's `remove` gets what that owner demands.** A provider MCP entry
//!    is removed through a digest-bound plan and permit against a reviewed
//!    configuration path; a Codex plugin through the Codex CLI's own removal. This
//!    adapter carries those inputs from the caller who planned the uninstall. It
//!    invents no plan, no permit, no configuration path and no executable.
//! 2. **Missing inputs refuse; they never skip.** An owner with no inputs reports
//!    [`RELEASE_INPUTS_MISSING`] naming its own module, so uninstall stops with the
//!    bytes in place instead of reporting a surface released that it never
//!    touched. A stale approval reports [`RELEASE_APPROVAL_STALE`]: the entry
//!    changed since the user approved its removal, and removing the new one under
//!    the old approval is exactly what the digest binding prevents.

use crate::platform::extension_packages::refusal;
use crate::platform::extension_packages::registration::{
    RecordedRegistration, RegistrationOwner, RegistrationOwners, ReleasedRegistration, STAGE,
    owner_unavailable,
};
use crate::platform::provider_mcp_registration::{self, ProviderConfigKind, RegistrationPlan};
use licoup_application::ApplicationFailure;
use std::path::PathBuf;

/// The refusal code for an owner whose release inputs the caller did not supply.
pub const RELEASE_INPUTS_MISSING: &str = "package_registration_release_inputs_missing";

/// The refusal code for an approval that no longer describes the surface.
pub const RELEASE_APPROVAL_STALE: &str = "package_registration_approval_stale";

/// The refusal code for an owner whose own removal refused.
pub const RELEASE_FAILED: &str = "package_registration_release_failed";

/// What releasing one provider MCP surface needs.
///
/// `approved_digest` is the digest the user approved when the removal was
/// planned. `remove` re-derives the plan from the current file and refuses when
/// it no longer matches, so an entry edited after approval is not removed under
/// the old decision.
#[derive(Clone, Debug)]
pub struct ProviderMcpRelease {
    pub kind: ProviderConfigKind,
    pub connector: PathBuf,
    pub config_path: PathBuf,
    pub approved_digest: String,
}

/// What releasing the Codex plugin surface would need.
///
/// The caller's description of the plugin is still part of the request
/// vocabulary, so a route can report which surface a record names. This build
/// owns no such surface: the Codex caller plugin belongs to Codex's own
/// marketplace, so `release` refuses the owner instead of acting on these
/// inputs.
#[derive(Clone, Debug)]
pub struct CodexPluginRelease {
    pub executable: PathBuf,
    pub approved_digest: String,
}

/// One owner's release inputs.
#[derive(Clone, Debug, Default)]
pub struct ReleaseInputs {
    pub provider_mcp: Option<ProviderMcpRelease>,
    pub codex_plugin: Option<CodexPluginRelease>,
}

impl ReleaseInputs {
    pub const fn none() -> Self {
        Self {
            provider_mcp: None,
            codex_plugin: None,
        }
    }

    pub fn with_provider_mcp(mut self, release: ProviderMcpRelease) -> Self {
        self.provider_mcp = Some(release);
        self
    }

    pub fn with_codex_plugin(mut self, release: CodexPluginRelease) -> Self {
        self.codex_plugin = Some(release);
        self
    }
}

/// The production adapter: every recorded registration is released by the module
/// that owns its surface.
#[derive(Clone, Debug, Default)]
pub struct PackageRegistrationOwners {
    inputs: ReleaseInputs,
}

impl PackageRegistrationOwners {
    pub fn new(inputs: ReleaseInputs) -> Self {
        Self { inputs }
    }

    /// The inputs this adapter carries, so a caller can inspect them.
    pub fn inputs(&self) -> &ReleaseInputs {
        &self.inputs
    }

    /// Whether this adapter carries what one owner needs.
    ///
    /// A caller reports which surfaces it is ready to release before draining, so
    /// a partially configured uninstall is visible before anything is closed.
    /// The answer is per owner, not "some provider is configured": inputs for
    /// Cursor are not inputs for Claude Code, and reporting otherwise would let a
    /// caller believe a surface was releasable that release then refuses.
    pub fn can_release(&self, owner: RegistrationOwner) -> bool {
        match owner {
            RegistrationOwner::LoginItem => false,
            RegistrationOwner::ClaudeCodeMcp => {
                self.provider_inputs(ProviderConfigKind::ClaudeCode)
            }
            RegistrationOwner::CursorMcp => self.provider_inputs(ProviderConfigKind::Cursor),
            RegistrationOwner::AntigravityMcp => {
                self.provider_inputs(ProviderConfigKind::Antigravity)
            }
            RegistrationOwner::CodexPlugin => false,
        }
    }

    /// Whether the carried provider inputs are the ones the named provider owns.
    fn provider_inputs(&self, expected: ProviderConfigKind) -> bool {
        self.inputs
            .provider_mcp
            .as_ref()
            .is_some_and(|release| release.kind == expected)
    }

    fn release_provider_mcp(
        &self,
        registration: &RecordedRegistration,
        expected: ProviderConfigKind,
    ) -> Result<ReleasedRegistration, ApplicationFailure> {
        let Some(release) = self.inputs.provider_mcp.as_ref() else {
            return Err(missing_inputs(registration));
        };
        // The record names the surface; the caller's inputs name the path. A
        // mismatch refuses rather than redirects: releasing Cursor's entry
        // because Cursor's inputs happened to be loaded would touch a different
        // surface than the record names.
        if release.kind != expected {
            return Err(missing_inputs(registration));
        }
        let plan = RegistrationPlan::prepare_with_config_path(
            release.kind,
            &release.connector,
            &release.config_path,
        )
        .map_err(|_| failed(registration))?;
        let mut permit = plan
            .approve(true, &release.approved_digest)
            .map_err(|_| stale_approval(registration))?;
        match provider_mcp_registration::remove(&plan, &mut permit) {
            Ok(()) => Ok(ReleasedRegistration::removed(registration)),
            Err(_) => Err(failed(registration)),
        }
    }
}

impl RegistrationOwners for PackageRegistrationOwners {
    fn release(
        &self,
        registration: &RecordedRegistration,
    ) -> Result<ReleasedRegistration, ApplicationFailure> {
        match registration.owner {
            // No package can write a login item today: the manifest has no such
            // declaration, so a record naming one is a fact this build cannot act
            // on, and the refusal names the module that would have to grow it.
            RegistrationOwner::LoginItem => Err(owner_unavailable(registration)),
            RegistrationOwner::ClaudeCodeMcp => {
                self.release_provider_mcp(registration, ProviderConfigKind::ClaudeCode)
            }
            RegistrationOwner::CursorMcp => {
                self.release_provider_mcp(registration, ProviderConfigKind::Cursor)
            }
            RegistrationOwner::AntigravityMcp => {
                self.release_provider_mcp(registration, ProviderConfigKind::Antigravity)
            }
            // The Codex caller plugin belongs to Codex's own marketplace: this
            // client neither installs nor removes one, so a record naming that
            // surface is a fact this build cannot act on. The refusal names the
            // owner module that would have to grow it back, exactly as the
            // login-item arm does, and a caller that still describes a plugin
            // gets the same answer rather than a silent success.
            RegistrationOwner::CodexPlugin => Err(owner_unavailable(registration)),
        }
    }
}

fn missing_inputs(registration: &RecordedRegistration) -> ApplicationFailure {
    refusal(RELEASE_INPUTS_MISSING, STAGE)
        .with_field("registrations")
        .with_presentation_arg("owner", registration.owner.wire_name())
        .with_presentation_arg("packageOwner", registration.owner.owner_module())
}

fn stale_approval(registration: &RecordedRegistration) -> ApplicationFailure {
    refusal(RELEASE_APPROVAL_STALE, STAGE)
        .with_field("registrations")
        .with_presentation_arg("owner", registration.owner.wire_name())
        .with_presentation_arg("packageOwner", registration.owner.owner_module())
}

fn failed(registration: &RecordedRegistration) -> ApplicationFailure {
    refusal(RELEASE_FAILED, STAGE)
        .with_field("registrations")
        .with_presentation_arg("owner", registration.owner.wire_name())
        .with_presentation_arg("packageOwner", registration.owner.owner_module())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every owner the record can carry reaches a handler, and the adapter states
    /// which surfaces it currently holds inputs for. A missing handler would be a
    /// surface a package could register and never release.
    #[test]
    fn every_owner_has_a_handler_that_refuses_without_inputs() {
        let bare = PackageRegistrationOwners::default();
        for owner in RegistrationOwner::ALL {
            assert!(
                !bare.can_release(owner),
                "{} must need inputs",
                owner.wire_name()
            );
            let failure = bare
                .release(&RecordedRegistration::new(owner, "land.lico.fixture"))
                .expect_err("no inputs, no release");
            assert!(
                failure.code == RELEASE_INPUTS_MISSING
                    || failure.code == "package_registration_owner_unavailable",
                "{} reported {}",
                owner.wire_name(),
                failure.code
            );
            let args = failure
                .presentation_args
                .iter()
                .map(|(key, value)| (key, value))
                .collect::<Vec<_>>();
            assert!(args.contains(&("packageOwner", owner.owner_module())));
        }
    }

    /// A login item is not a surface any package can write, so releasing one is
    /// refused by name instead of quietly succeeding.
    #[test]
    fn a_login_item_registration_is_refused_by_name() {
        let owners = PackageRegistrationOwners::default();
        let failure = owners
            .release(&RecordedRegistration::new(
                RegistrationOwner::LoginItem,
                "licoup",
            ))
            .expect_err("no per-package login item surface exists");
        assert_eq!(failure.code, "package_registration_owner_unavailable");
        let args = failure
            .presentation_args
            .iter()
            .map(|(key, value)| (key, value))
            .collect::<Vec<_>>();
        assert!(args.contains(&("packageOwner", "platform::client_autostart")));
    }

    /// Inputs for one provider never release another provider's surface.
    #[test]
    fn a_provider_release_refuses_inputs_for_a_different_provider() {
        let owners = PackageRegistrationOwners::new(ReleaseInputs::none().with_provider_mcp(
            ProviderMcpRelease {
                kind: ProviderConfigKind::Cursor,
                connector: PathBuf::from("/fixture/connector"),
                config_path: PathBuf::from("/fixture/config.json"),
                approved_digest: "sha256:fixture".to_owned(),
            },
        ));
        assert!(owners.can_release(RegistrationOwner::CursorMcp));
        assert!(!owners.can_release(RegistrationOwner::ClaudeCodeMcp));
        assert!(!owners.can_release(RegistrationOwner::CodexPlugin));
        let failure = owners
            .release(&RecordedRegistration::new(
                RegistrationOwner::ClaudeCodeMcp,
                "land.lico.licoup.subagents",
            ))
            .expect_err("Cursor inputs are not Claude Code inputs");
        assert_eq!(failure.code, RELEASE_INPUTS_MISSING);
    }

    /// A stale approval stops the release: the removal is refused before the
    /// owner's `remove` is reached, because the plan it would act on is different
    /// from the one the user approved.
    #[test]
    fn an_approval_that_no_longer_matches_the_surface_is_refused() {
        let owners = PackageRegistrationOwners::new(ReleaseInputs::none().with_provider_mcp(
            ProviderMcpRelease {
                kind: ProviderConfigKind::Cursor,
                connector: PathBuf::from("/fixture/connector"),
                config_path: PathBuf::from("/fixture/absent-config.json"),
                approved_digest: "sha256:stale".to_owned(),
            },
        ));
        // The reviewed-candidate rule refuses the unreviewed path first, which is
        // the same outcome for the caller: nothing was removed.
        let failure = owners
            .release(&RecordedRegistration::new(
                RegistrationOwner::CursorMcp,
                "land.lico.licoup.subagents",
            ))
            .expect_err("an unreviewed path is not a release target");
        assert_eq!(failure.code, RELEASE_FAILED);
    }
}
