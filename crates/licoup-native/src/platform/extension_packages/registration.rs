//! What a package registered outside its own managed bytes, and who releases it.
//!
//! A package can leave durable facts in surfaces this module does not own: a
//! provider's user MCP configuration, a Codex plugin entry, a login item. Those
//! surfaces belong to the modules that write them, and uninstall must ask *those*
//! owners to remove the entry rather than delete a file it does not own.
//!
//! Two rules are enforced here:
//!
//! 1. **Release is driven by what was recorded, never by a guess.** An installed
//!    version records the registrations it created ([`RecordedRegistration`]);
//!    uninstall releases exactly those. A package that registered nothing — the
//!    ordinary case for a package the host starts itself — records nothing, and
//!    uninstall asks no owner for anything.
//! 2. **A release is reported, not assumed.** [`RegistrationOwners::release`]
//!    returns what the owner said it did. Uninstall reports the released set and
//!    refuses when an owner refuses; it never reports a surface as released
//!    because it intended to release it.
//!
//! `subagent_mcp_ensure` is deliberately absent from [`RegistrationOwner`]. It is
//! a reconciler that *plans and installs* registrations through the provider
//! managers; the durable entry it writes belongs to
//! [`RegistrationOwner::ClaudeCodeMcp`], [`RegistrationOwner::CursorMcp`] or
//! [`RegistrationOwner::AntigravityMcp`], and those are the owners that remove
//! it. Adding a second owner for one entry would let one removal path miss what
//! another wrote.

use crate::platform::extension_packages::refusal;
use licoup_application::ApplicationFailure;
use serde::{Deserialize, Serialize};

/// The stage every refusal from this module reports.
pub const STAGE: &str = "extension/package-registration";

/// The owner of one durable registration surface a package can write.
///
/// Each variant names the module whose `remove` path is authoritative for that
/// surface. There is no variant without an owner.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RegistrationOwner {
    /// A login/autostart item, owned by the Agent target declarations
    /// (`platform::client_autostart`).
    LoginItem,
    /// One entry in Claude Code's user MCP configuration
    /// (`platform::claude_code_subagent_mcp_manager`, which delegates to
    /// `platform::provider_mcp_registration`).
    ClaudeCodeMcp,
    /// One entry in Cursor's user MCP configuration
    /// (`platform::cursor_subagent_mcp_manager`).
    CursorMcp,
    /// One entry in Antigravity's user MCP configuration
    /// (`platform::antigravity_subagent_mcp_manager`).
    AntigravityMcp,
    /// One installed Codex plugin.
    ///
    /// The caller plugin is Codex's own marketplace artefact and the Codex
    /// adapter package owns that side. The kernel module that used to install and
    /// remove it (`platform::codex_plugin_manager`) is gone with the Codex
    /// package move, so `RegistrationOwners::release` refuses this surface
    /// instead of reporting configuration released that it never touched.
    CodexPlugin,
}

impl RegistrationOwner {
    /// The stable wire name, which is also the owner module's public name.
    pub const fn wire_name(self) -> &'static str {
        match self {
            Self::LoginItem => "login-item",
            Self::ClaudeCodeMcp => "claude-code-mcp",
            Self::CursorMcp => "cursor-mcp",
            Self::AntigravityMcp => "antigravity-mcp",
            Self::CodexPlugin => "codex-plugin",
        }
    }

    /// The command or module whose `remove` is authoritative for this surface.
    ///
    /// A surface no module in this tree can remove names the owner that would
    /// have to grow the route back instead. The Codex caller plugin is the one
    /// such surface: it belongs to Codex's own marketplace, the Codex adapter
    /// package owns that side, and this client installs none — so its name is
    /// the package rather than a kernel module.
    pub const fn owner_module(self) -> &'static str {
        match self {
            Self::LoginItem => "platform::client_autostart",
            Self::ClaudeCodeMcp => "platform::claude_code_subagent_mcp_manager",
            Self::CursorMcp => "platform::cursor_subagent_mcp_manager",
            Self::AntigravityMcp => "platform::antigravity_subagent_mcp_manager",
            Self::CodexPlugin => "licoup-agent-codex",
        }
    }

    /// Every owner, so a test can prove the adapter covers all of them.
    pub const ALL: [Self; 5] = [
        Self::LoginItem,
        Self::ClaudeCodeMcp,
        Self::CursorMcp,
        Self::AntigravityMcp,
        Self::CodexPlugin,
    ];
}

/// One registration a package version created.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordedRegistration {
    pub owner: RegistrationOwner,
    /// The key this package registered under inside the owner's surface. It is
    /// the owner's namespace, not a path: the owner resolves the path.
    pub key: String,
}

impl RecordedRegistration {
    pub fn new(owner: RegistrationOwner, key: impl Into<String>) -> Self {
        Self {
            owner,
            key: key.into(),
        }
    }

    /// A registration the owner can act on must name a key and stay bounded.
    pub fn validate(&self) -> Result<(), ApplicationFailure> {
        if self.key.is_empty() || self.key.len() > 256 || self.key.contains('\0') {
            return Err(refusal("package_registration_invalid", STAGE)
                .with_field("registrations")
                .with_presentation_arg("owner", self.owner.wire_name()));
        }
        Ok(())
    }
}

/// What one owner did when asked to release a registration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReleasedRegistration {
    pub owner: RegistrationOwner,
    pub key: String,
    /// The owner found the entry and removed it.
    pub removed: bool,
    /// The owner reported no such entry: it was already gone, which is not a
    /// failure and is reported so the caller can tell the two apart.
    pub already_absent: bool,
}

impl ReleasedRegistration {
    pub fn removed(registration: &RecordedRegistration) -> Self {
        Self {
            owner: registration.owner,
            key: registration.key.clone(),
            removed: true,
            already_absent: false,
        }
    }

    pub fn already_absent(registration: &RecordedRegistration) -> Self {
        Self {
            owner: registration.owner,
            key: registration.key.clone(),
            removed: false,
            already_absent: true,
        }
    }
}

/// The owners that can release a package's registrations.
///
/// Uninstall holds one of these for the duration of [`release_all`]. There is no
/// default implementation that releases nothing: a no-op would report success for
/// work it did not do, so a caller must name an owner.
///
/// [`release_all`]: RegistrationOwners::release_all
pub trait RegistrationOwners {
    /// Ask one owner to remove one registration.
    fn release(
        &self,
        registration: &RecordedRegistration,
    ) -> Result<ReleasedRegistration, ApplicationFailure>;

    /// Release every registration a package version recorded, in order.
    ///
    /// The first refusal stops the sequence and travels unchanged: the caller
    /// learns which owner refused and nothing is reported as released after it.
    fn release_all(
        &self,
        registrations: &[RecordedRegistration],
    ) -> Result<Vec<ReleasedRegistration>, ApplicationFailure> {
        let mut released = Vec::with_capacity(registrations.len());
        for registration in registrations {
            registration.validate()?;
            released.push(self.release(registration)?);
        }
        Ok(released)
    }
}

/// An owner asked to release a surface no package can register.
///
/// Reached only when a record names a surface the current owners cannot write.
/// Refusing is the honest answer: reporting it released would be a claim about
/// someone else's configuration file.
pub fn owner_unavailable(registration: &RecordedRegistration) -> ApplicationFailure {
    refusal("package_registration_owner_unavailable", STAGE)
        .with_field("registrations")
        .with_presentation_arg("owner", registration.owner.wire_name())
        .with_presentation_arg("packageOwner", registration.owner.owner_module())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Recorder;

    impl RegistrationOwners for Recorder {
        fn release(
            &self,
            registration: &RecordedRegistration,
        ) -> Result<ReleasedRegistration, ApplicationFailure> {
            if registration.key == "absent" {
                return Ok(ReleasedRegistration::already_absent(registration));
            }
            Ok(ReleasedRegistration::removed(registration))
        }
    }

    #[test]
    fn a_release_reports_what_the_owner_said_and_not_what_was_intended() {
        let registrations = [
            RecordedRegistration::new(RegistrationOwner::CursorMcp, "land.lico.licoup.subagents"),
            RecordedRegistration::new(RegistrationOwner::CodexPlugin, "absent"),
        ];
        let released = Recorder.release_all(&registrations).expect("released");
        assert!(released[0].removed);
        assert!(!released[0].already_absent);
        assert!(!released[1].removed);
        assert!(released[1].already_absent);
    }

    #[test]
    fn every_owner_names_the_module_that_can_remove_its_surface() {
        // Four surfaces belong to a kernel module; the fifth belongs to the Codex
        // adapter package, because the Codex caller plugin is Codex's own
        // marketplace artefact and this client installs none. Every name is
        // asserted exactly: a prefix check let a retired module path
        // (`platform::codex_plugin_manager`) survive here after its module moved.
        assert_eq!(RegistrationOwner::ALL.len(), 5);
        for owner in RegistrationOwner::ALL {
            assert!(!owner.wire_name().is_empty());
        }
        for (owner, module) in [
            (RegistrationOwner::LoginItem, "platform::client_autostart"),
            (
                RegistrationOwner::ClaudeCodeMcp,
                "platform::claude_code_subagent_mcp_manager",
            ),
            (
                RegistrationOwner::CursorMcp,
                "platform::cursor_subagent_mcp_manager",
            ),
            (
                RegistrationOwner::AntigravityMcp,
                "platform::antigravity_subagent_mcp_manager",
            ),
            (RegistrationOwner::CodexPlugin, "licoup-agent-codex"),
        ] {
            assert_eq!(owner.owner_module(), module, "{}", owner.wire_name());
        }
    }

    #[test]
    fn a_registration_without_a_key_is_refused_before_any_owner_is_asked() {
        let registrations = [RecordedRegistration::new(RegistrationOwner::LoginItem, "")];
        assert_eq!(
            Recorder
                .release_all(&registrations)
                .expect_err("no key")
                .code,
            "package_registration_invalid"
        );
    }

    #[test]
    fn an_unsupported_surface_refuses_and_names_its_owner_module() {
        let registration = RecordedRegistration::new(RegistrationOwner::LoginItem, "licoup");
        let failure = owner_unavailable(&registration);
        assert_eq!(failure.code, "package_registration_owner_unavailable");
        let args = failure
            .presentation_args
            .iter()
            .map(|(key, value)| (key, value))
            .collect::<Vec<_>>();
        assert!(args.contains(&("owner", "login-item")));
        assert!(args.contains(&("packageOwner", "platform::client_autostart")));
    }
}
