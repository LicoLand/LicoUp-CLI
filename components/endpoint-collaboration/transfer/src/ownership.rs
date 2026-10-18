//! The declared ownership table a transfer classifies against.
//!
//! Classification is a declared table, not a guess: an area of the data root is
//! managed, an external reference, or a credential requirement because its owner
//! said so. A captured path that no declaration claims is reported as
//! unattributed rather than silently called managed, and a limitation that no
//! declaration explains is reported as unexplained rather than assumed benign.

use anyhow::{Result, ensure};

use crate::inventory::{CredentialCustody, ExternalOwner, ManagedDomain};

/// One owner declaration for the data root.
///
/// Prefixes are canonical data-root-relative POSIX paths without a trailing
/// slash, the same grammar the archive owner validates for a member path. The
/// longest matching prefix decides, so a specific area can refine a general one
/// without either declaration having to know the other's order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransferOwnership {
    managed: Vec<(String, ManagedDomain)>,
    external: Vec<(String, ExternalOwner)>,
    credentials: Vec<(String, CredentialCustody)>,
}

impl TransferOwnership {
    /// Build a table from explicit declarations.
    ///
    /// Every prefix is validated here so a malformed declaration is refused
    /// while the composition is being built, not discovered later as an
    /// unattributed path in a published inventory.
    pub fn new(
        managed: impl IntoIterator<Item = (String, ManagedDomain)>,
        external: impl IntoIterator<Item = (String, ExternalOwner)>,
        credentials: impl IntoIterator<Item = (String, CredentialCustody)>,
    ) -> Result<Self> {
        let managed = collect(managed, "transfer_ownership_prefix_invalid")?;
        let external = collect(external, "transfer_ownership_prefix_invalid")?;
        let credentials = collect(credentials, "transfer_ownership_prefix_invalid")?;
        Ok(Self {
            managed,
            external,
            credentials,
        })
    }

    /// The declared ownership of the areas the current owners write.
    ///
    /// The client-state spellings come from their owner's own constants; the
    /// remaining areas are the transfer owner's declaration. Adding an area to
    /// the data root is that owner's change: until it is declared here, its
    /// paths are reported as unattributed, so the inventory says it cannot
    /// describe them instead of claiming they travel safely.
    pub fn declared() -> Self {
        use licoup_client_state::policy::{ACTIVITY_DIR, CLIENT_STATE_DIR, SNAPSHOT_DIR};
        use licoup_foundation::core::full_data_root_archive::{
            CREDENTIAL_DOMAIN, CREDENTIAL_INVENTORY_PATH,
        };

        let managed = vec![
            // The optional-package data home and the install receipts beside it.
            ("packages".to_string(), ManagedDomain::PackageData),
            // Owner-installed resources that are not client state: the model
            // registry catalog and the Agent resource directories.
            ("model-registry".to_string(), ManagedDomain::Resource),
            ("lico-subagent-mcp".to_string(), ManagedDomain::Resource),
            // The areas the migration owner records at the data-root root rather
            // than under the client-state directory: the workspace manifest
            // (`workspace-manifest`), the group conversation records and the
            // Adaptive Flywheel strategy configuration.
            (
                ".licoup-workspace.json".to_string(),
                ManagedDomain::Workspace,
            ),
            (
                "group-conversations".to_string(),
                ManagedDomain::Conversation,
            ),
            (
                "adaptive-flywheel.toml".to_string(),
                ManagedDomain::ClientState,
            ),
            // The provider-key inventory document the archive owner captures as
            // non-secret metadata. Its own constant names the path, so the
            // classification cannot drift from the file the owner reads; the key
            // material it describes stays in the credential domain below.
            (
                CREDENTIAL_INVENTORY_PATH.to_string(),
                ManagedDomain::ClientState,
            ),
            // Client state, most specific first.
            (
                format!("{CLIENT_STATE_DIR}/conversations"),
                ManagedDomain::Conversation,
            ),
            (
                format!("{CLIENT_STATE_DIR}/migrations"),
                ManagedDomain::MigrationJournal,
            ),
            (CLIENT_STATE_DIR.to_string(), ManagedDomain::ClientState),
            (ACTIVITY_DIR.to_string(), ManagedDomain::Activity),
            (SNAPSHOT_DIR.to_string(), ManagedDomain::Snapshot),
            ("archives".to_string(), ManagedDomain::Archive),
            ("cache".to_string(), ManagedDomain::Cache),
            ("temp".to_string(), ManagedDomain::Temp),
            ("logs".to_string(), ManagedDomain::Logs),
        ];
        let external = vec![
            (
                "provider-retained-history".to_string(),
                ExternalOwner::ProviderRetainedHistory,
            ),
            (
                "project-workspace".to_string(),
                ExternalOwner::ProjectWorkspace,
            ),
            (
                "other-application".to_string(),
                ExternalOwner::OtherApplication,
            ),
            (
                "operating-system".to_string(),
                ExternalOwner::OperatingSystem,
            ),
        ];
        let credentials = vec![
            // The provider-key custody domain the archive owner already reports.
            // Its constant is the owner's own spelling of the domain.
            (
                CREDENTIAL_DOMAIN.to_string(),
                CredentialCustody::ProviderKeyReentry,
            ),
            // The endpoint's own non-exportable protocol custody.
            (
                "endpoint-identity-custody".to_string(),
                CredentialCustody::NonExportableReauthorization,
            ),
            // A provider account obtained through an interactive sign-in.
            (
                "provider-account-token".to_string(),
                CredentialCustody::TokenSignIn,
            ),
        ];
        Self::new(managed, external, credentials).expect("the declared table is well formed")
    }

    /// The managed area owning `path`, when a declaration claims it.
    pub(crate) fn managed_domain(&self, path: &str) -> Option<ManagedDomain> {
        longest_match(&self.managed, path).copied()
    }

    /// The external owner of the limitation domain `domain`.
    pub(crate) fn external_owner(&self, domain: &str) -> Option<ExternalOwner> {
        self.external
            .iter()
            .find(|(declared, _)| declared == domain)
            .map(|(_, owner)| *owner)
    }

    /// The credential custody the limitation domain `domain` requires.
    pub(crate) fn credential_custody(&self, domain: &str) -> Option<CredentialCustody> {
        self.credentials
            .iter()
            .find(|(declared, _)| declared == domain)
            .map(|(_, custody)| *custody)
    }
}

fn collect<T: Copy>(
    declarations: impl IntoIterator<Item = (String, T)>,
    code: &'static str,
) -> Result<Vec<(String, T)>> {
    let mut collected = Vec::new();
    for (prefix, value) in declarations {
        ensure!(portable_prefix(&prefix), "{}", code);
        ensure!(
            !collected.iter().any(|(seen, _): &(String, T)| seen == &prefix),
            "{}",
            code
        );
        collected.push((prefix, value));
    }
    Ok(collected)
}

/// Whether a declared prefix is a canonical data-root-relative POSIX prefix.
fn portable_prefix(prefix: &str) -> bool {
    !prefix.is_empty()
        && !prefix.starts_with('/')
        && !prefix.ends_with('/')
        && prefix.split('/').all(|part| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && !part.contains('\\')
                && !part.contains(':')
                && !part.chars().any(char::is_control)
        })
}

/// The longest declared prefix that owns `path`, or `None`.
fn longest_match<'a, T: Copy>(declarations: &'a [(String, T)], path: &str) -> Option<&'a T> {
    let mut best: Option<(&str, &T)> = None;
    for (prefix, value) in declarations {
        let owns = path == prefix
            || (path.len() > prefix.len()
                && path.starts_with(prefix.as_str())
                && path.as_bytes()[prefix.len()] == b'/');
        if owns && best.is_none_or(|(seen, _)| prefix.len() > seen.len()) {
            best = Some((prefix.as_str(), value));
        }
    }
    best.map(|(_, value)| value)
}
