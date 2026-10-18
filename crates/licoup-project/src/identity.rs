//! The declared identity of one authorized project.
//!
//! Nothing here resolves an identity from the environment. A caller supplies
//! every identifier, the owner bounds and stores it, and a request that asks for
//! resolution by inspection is refused by name
//! ([`ProjectIdentitySource::Discovered`]).

use crate::authority::ProjectAuthorityDirectory;
use crate::failure::ProjectFailure;
use serde::{Deserialize, Deserializer, Serialize};
use std::fmt;
use std::path::Path;

/// Largest declared project identity.
pub const MAX_PROJECT_ID_BYTES: usize = 128;
/// Largest declared workspace identity.
pub const MAX_WORKSPACE_ID_BYTES: usize = 128;
/// Largest declared plan identity.
pub const MAX_PLAN_ID_BYTES: usize = 128;
/// Largest declared work-item identity.
pub const MAX_WORK_ITEM_ID_BYTES: usize = 128;
/// Largest project display name.
pub const MAX_DISPLAY_NAME_BYTES: usize = 256;
/// Largest declared authorized root.
pub const MAX_AUTHORIZED_ROOT_BYTES: usize = 4096;
/// Largest authority reference.
pub const MAX_AUTHORITY_REFERENCE_BYTES: usize = 256;

/// One bounded, declared identifier.
///
/// The alphabet is the product's stable-identifier alphabet with no path
/// separator, so an identity that could be read as a location — `../etc`,
/// `a/b`, `/srv/synthetic-roots/workspace` — is refused as an identity rather than normalized into one.
pub(crate) fn declared_identifier(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && !value.contains('\0')
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'-' | b'_' | b'.' | b':')
        })
}

/// Declare the three bounded identifier newtypes over one rule.
///
/// Each one keeps its own refusal code, so a caller is told which declared
/// identity was unusable rather than that "an identity" was.
macro_rules! declared_identity {
    ($name:ident, $code:literal, $max:expr, $label:literal) => {
        #[doc = concat!("A declared ", $label, ".")]
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Accept a caller-declared identity, or refuse it by name.
            pub fn declare(value: impl Into<String>) -> Result<Self, ProjectFailure> {
                let value = value.into();
                if !declared_identifier(&value, $max) {
                    return Err(ProjectFailure::identity($code));
                }
                Ok(Self(value))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Self::declare(value).map_err(serde::de::Error::custom)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }
    };
}

declared_identity!(
    ProjectId,
    "project_identity_required",
    MAX_PROJECT_ID_BYTES,
    "project identity"
);
declared_identity!(
    WorkspaceId,
    "project_workspace_identity_required",
    MAX_WORKSPACE_ID_BYTES,
    "workspace identity"
);
declared_identity!(
    PlanId,
    "project_plan_identity_required",
    MAX_PLAN_ID_BYTES,
    "plan identity"
);
declared_identity!(
    WorkItemId,
    "project_work_item_identity_required",
    MAX_WORK_ITEM_ID_BYTES,
    "work-item identity"
);

/// The declared root this project is authorized to operate in.
///
/// The owner stores the declaration and never opens, lists or hashes it: a root
/// is evidence of the authorization the caller already holds, not an input the
/// owner may explore on its own.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct AuthorizedRoot(String);

impl AuthorizedRoot {
    /// Accept one declared absolute location.
    pub fn declare(value: impl Into<String>) -> Result<Self, ProjectFailure> {
        let value = value.into();
        if value.trim().is_empty()
            || value.len() > MAX_AUTHORIZED_ROOT_BYTES
            || value.contains('\0')
            || !Path::new(&value).is_absolute()
        {
            return Err(ProjectFailure::identity("project_authorized_root_required"));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for AuthorizedRoot {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::declare(value).map_err(serde::de::Error::custom)
    }
}

impl fmt::Display for AuthorizedRoot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Which existing authority owner a reference points into.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuthorityKind {
    /// A conversation membership, exactly as the shared actor claim names it.
    Membership,
    /// A maintainer role the deployment already publishes.
    Role,
    /// One explicit grant issued by the owner.
    Grant,
}

impl AuthorityKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Membership => "membership",
            Self::Role => "role",
            Self::Grant => "grant",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "membership" => Some(Self::Membership),
            "role" => Some(Self::Role),
            "grant" => Some(Self::Grant),
            _ => None,
        }
    }
}

/// A reference into the existing authority owner.
///
/// This is deliberately a *reference*: the record carries the name of a grant
/// the authority owner already answers for, never a credential, token or copied
/// permission set. Admitting the reference is
/// [`ProjectAuthorityDirectory`]'s answer, not this type's.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthorityReference {
    kind: AuthorityKind,
    reference: String,
}

impl AuthorityReference {
    /// Declare one reference, bounded and non-empty.
    pub fn declare(
        kind: AuthorityKind,
        reference: impl Into<String>,
    ) -> Result<Self, ProjectFailure> {
        let reference = reference.into();
        if reference.trim().is_empty()
            || reference.len() > MAX_AUTHORITY_REFERENCE_BYTES
            || reference.contains('\0')
        {
            return Err(ProjectFailure::identity(
                "project_authority_reference_required",
            ));
        }
        Ok(Self { kind, reference })
    }

    /// The reference a verified membership claim carries.
    pub fn membership(reference: impl Into<String>) -> Result<Self, ProjectFailure> {
        Self::declare(AuthorityKind::Membership, reference)
    }

    pub fn kind(&self) -> AuthorityKind {
        self.kind
    }

    /// The authority owner's own identifier, unchanged.
    ///
    /// The kind is carried beside it rather than inside it: the identifier is
    /// already the one the owning authority publishes (`membership:owner`),
    /// so rewriting it here would invent a second spelling of the same grant.
    pub fn reference(&self) -> &str {
        &self.reference
    }
}

impl<'de> Deserialize<'de> for AuthorityReference {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Raw {
            kind: AuthorityKind,
            reference: String,
        }
        let raw = Raw::deserialize(deserializer)?;
        Self::declare(raw.kind, raw.reference).map_err(serde::de::Error::custom)
    }
}

impl fmt::Display for AuthorityReference {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} {}", self.kind.as_str(), self.reference)
    }
}

/// How a registration names the project identity.
///
/// The two variants are the whole rule: an identity is either declared by the
/// caller or it does not exist. There is no third variant that walks a
/// directory, because a discovered identity would be an authorization the
/// caller never made.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum ProjectIdentitySource {
    /// The caller declared the project identity.
    Declared { project_id: String },
    /// The caller asked the owner to derive the identity from the authorized
    /// root. Refused: the owner never reads a directory to name a project.
    Discovered { authorized_root: String },
}

/// One explicit request to register an authorized project.
///
/// Unknown fields are refused rather than ignored: a payload that carries a
/// credential or a permission set alongside the declaration is not a
/// registration this owner can honour, and silently dropping the extra field
/// would record a request the caller never made.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectRegistration {
    /// How the project identity is named.
    pub identity: ProjectIdentitySource,
    /// The name a person reads. Display only; never an identity.
    pub display_name: String,
    /// The declared root this project is authorized to operate in.
    pub authorized_root: String,
    /// The authority the caller registers under. Absent is a refusal, not a
    /// default.
    pub authority: Option<AuthorityReference>,
    /// The workspace identity this project belongs to.
    pub workspace_id: String,
    /// The plan identity carried by this registration.
    pub plan_id: String,
}

impl ProjectRegistration {
    /// Admit this declaration against the authority owner and return the record
    /// it names.
    ///
    /// Every refusal happens before the store is reached, so a refused
    /// registration leaves no row and no partial write behind.
    pub fn declare(
        &self,
        authorities: &dyn ProjectAuthorityDirectory,
    ) -> Result<RegisteredProject, ProjectFailure> {
        let project_id = match &self.identity {
            ProjectIdentitySource::Declared { project_id } => ProjectId::declare(project_id)?,
            ProjectIdentitySource::Discovered { .. } => {
                return Err(ProjectFailure::registration(
                    "project_identity_scan_required",
                ));
            }
        };
        let display_name = self.display_name.trim();
        if display_name.is_empty()
            || display_name.len() > MAX_DISPLAY_NAME_BYTES
            || display_name.contains('\0')
        {
            return Err(ProjectFailure::registration(
                "project_display_name_required",
            ));
        }
        let authorized_root = AuthorizedRoot::declare(self.authorized_root.clone())?;
        let workspace_id = WorkspaceId::declare(self.workspace_id.clone())?;
        let plan_id = PlanId::declare(self.plan_id.clone())?;
        let authority = self
            .authority
            .clone()
            .ok_or_else(|| ProjectFailure::registration("project_authority_reference_required"))?;
        if !authorities.admits(&authority) {
            return Err(ProjectFailure::registration(
                "project_authority_unauthorized",
            ));
        }
        Ok(RegisteredProject {
            project_id,
            display_name: display_name.to_owned(),
            authorized_root,
            authority,
            workspace_id,
            plan_id,
            registration_sequence: 0,
        })
    }
}

/// One durable registration, as the owner stores and returns it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisteredProject {
    pub project_id: ProjectId,
    pub display_name: String,
    pub authorized_root: AuthorizedRoot,
    /// The authority reference this project was registered under. A reference,
    /// never the credential behind it.
    pub authority: AuthorityReference,
    pub workspace_id: WorkspaceId,
    pub plan_id: PlanId,
    /// Registration order, assigned by the store. It gives listings a stable
    /// order without inventing a timestamp.
    pub registration_sequence: u64,
}
