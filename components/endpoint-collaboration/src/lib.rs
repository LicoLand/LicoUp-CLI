//! The optional endpoint collaboration package boundary.
//!
//! `org.licoland.feature.endpoint-collaboration` is the installable capability
//! that owns paired-device pairing, secure mesh relay delivery and remote work
//! control. The kernel owns custody, durable state, the Canonical Conversation
//! and authority; this package reaches them only through those ports, and a
//! client without the package keeps every local conversation, running work and
//! history it already has.
//!
//! Two rules are the whole reason this vocabulary exists:
//!
//! * **Absent and disabled are answers, not an absence of an answer.** The
//!   boundary names which of *missing*, *disabled*, *capability-undeclared* and
//!   *active* the host resolved, and a caller that only knows how to hide a user
//!   interface cannot answer either of the first three.
//! * **The outbound send is the boundary.** Outbound endpoint traffic is
//!   permitted only while the capability resolves active. Refusing at the send
//!   entry is what makes "disabled" a cut rather than a hidden control, and it is
//!   the one behavior a user interface can neither grant nor fake.
//!
//! Nothing here performs a cryptographic operation, opens a store or sends a
//! packet: it decides *whether the capability may act at all*, and the kernel's
//! own owners perform the operation once the authority is granted.

#![forbid(unsafe_code)]

pub mod capability_catalogue;
pub mod durable_delivery;

/// The namespaced identity of this package.
pub const PACKAGE_ID: &str = "org.licoland.feature.endpoint-collaboration";

/// The capability this package contributes. It is the identity an outbound send
/// is checked against, not a label a caller may grant itself.
pub const CAPABILITY_ID: &str = "endpoint.collaboration.v1";

/// The profile that declares [`CAPABILITY_ID`].
pub const CONTROL_PROFILE_ID: &str = "endpoint-control";

/// What the host resolved about the installed endpoint collaboration package.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Availability {
    /// Installed, switched on, and declaring the outbound capability.
    Active { version: String },
    /// Installed and switched off by the user. Its declaration is not consulted:
    /// the user's switch is the answer, and it is the actionable one.
    Disabled { version: String },
    /// Installed and switched on, but the installed manifest declares no
    /// `endpoint.collaboration.v1` profile. The bytes are present and they do not
    /// own this capability.
    CapabilityUndeclared { version: String },
    /// No installed version of this package is present in the host's store.
    Missing,
    /// The package store could not be read, so no honest answer exists. It
    /// resolves like `Missing` for the caller and is reported separately here so
    /// the host never describes a broken store as an uninstalled package.
    Unreadable,
}

impl Availability {
    /// The active version, when the capability is usable.
    #[must_use]
    pub fn active_version(&self) -> Option<&str> {
        match self {
            Self::Active { version } => Some(version),
            _ => None,
        }
    }
}

/// Why outbound endpoint traffic is refused.
///
/// Each refusal is a stable reason a caller publishes verbatim; a caller must not
/// translate one into "no paired device" or into a user-interface state that a
/// different action would clear.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutboundRefusal {
    /// The package is not installed in this client.
    PackageMissing,
    /// The installed package is switched off.
    PackageDisabled,
    /// The installed package does not declare the outbound capability.
    CapabilityUndeclared,
    /// The package store could not be read. Fail closed.
    StoreUnreadable,
}

impl OutboundRefusal {
    /// The stable reason string this refusal is published as.
    #[must_use]
    pub const fn reason(self) -> &'static str {
        match self {
            Self::PackageMissing => "endpoint_collaboration_package_absent",
            Self::PackageDisabled => "endpoint_collaboration_package_disabled",
            Self::CapabilityUndeclared => "endpoint_collaboration_capability_undeclared",
            Self::StoreUnreadable => "endpoint_collaboration_store_unreadable",
        }
    }
}

/// Whether this client may emit outbound endpoint traffic, and who owns it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OutboundAuthority {
    /// The installed package owns the outbound path and permits it.
    Package { version: String },
    /// This client build carries the kernel's own pre-package path and no package
    /// has taken the path over. It is not a grant: it names the implementation
    /// that is running, and it disappears when the package is installed.
    LegacyInKernel,
}

impl OutboundAuthority {
    /// Whether an installed package owns the outbound path.
    #[must_use]
    pub fn is_package_owned(&self) -> bool {
        matches!(self, Self::Package { .. })
    }
}

/// The action that would make the capability usable again.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecoveryAction {
    /// Nothing to recover.
    None,
    /// Install the endpoint collaboration package.
    InstallPackage,
    /// Switch the installed package on. Installing already decided availability,
    /// so a disabled package is recovered by the user's own switch.
    EnablePackage,
    /// Install a package version whose manifest declares the capability.
    InstallCapableVersion,
    /// Repair or reinstall the package store before anything else is claimed.
    RepairStore,
}

/// The honest state report for a client whose endpoint collaboration capability
/// is unavailable.
///
/// `local_client_usable` is a constant `true` on purpose: the local client,
/// its conversations, its running work and its history never depend on this
/// optional package, and no caller may report otherwise.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoveryReport {
    pub local_client_usable: bool,
    pub capability_available: bool,
    pub refusal: Option<OutboundRefusal>,
    pub action: RecoveryAction,
}

/// The boundary over one resolved [`Availability`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EndpointCollaborationBoundary {
    availability: Availability,
}

impl EndpointCollaborationBoundary {
    /// The boundary of a client with no installed package.
    #[must_use]
    pub const fn missing() -> Self {
        Self {
            availability: Availability::Missing,
        }
    }

    /// The boundary of an installed, switched-on, capability-declaring package.
    #[must_use]
    pub fn active(version: impl Into<String>) -> Self {
        Self {
            availability: Availability::Active {
                version: version.into(),
            },
        }
    }

    /// The boundary of an installed package the user switched off.
    #[must_use]
    pub fn disabled(version: impl Into<String>) -> Self {
        Self {
            availability: Availability::Disabled {
                version: version.into(),
            },
        }
    }

    /// The boundary of an installed package that declares no outbound capability.
    #[must_use]
    pub fn capability_undeclared(version: impl Into<String>) -> Self {
        Self {
            availability: Availability::CapabilityUndeclared {
                version: version.into(),
            },
        }
    }

    /// The boundary of an unreadable package store.
    #[must_use]
    pub const fn unreadable() -> Self {
        Self {
            availability: Availability::Unreadable,
        }
    }

    /// The boundary over an already resolved availability.
    #[must_use]
    pub const fn over(availability: Availability) -> Self {
        Self { availability }
    }

    #[must_use]
    pub const fn availability(&self) -> &Availability {
        &self.availability
    }

    /// Whether outbound endpoint traffic may leave this client.
    pub fn outbound_authority(&self) -> Result<OutboundAuthority, OutboundRefusal> {
        match &self.availability {
            Availability::Active { version } => Ok(OutboundAuthority::Package {
                version: version.clone(),
            }),
            Availability::Disabled { .. } => Err(OutboundRefusal::PackageDisabled),
            Availability::CapabilityUndeclared { .. } => Err(OutboundRefusal::CapabilityUndeclared),
            Availability::Missing => Err(OutboundRefusal::PackageMissing),
            Availability::Unreadable => Err(OutboundRefusal::StoreUnreadable),
        }
    }

    /// What a client in this state reports to its user.
    #[must_use]
    pub fn recovery(&self) -> RecoveryReport {
        let (capability_available, refusal, action) = match &self.availability {
            Availability::Active { .. } => (true, None, RecoveryAction::None),
            Availability::Disabled { .. } => (
                false,
                Some(OutboundRefusal::PackageDisabled),
                RecoveryAction::EnablePackage,
            ),
            Availability::CapabilityUndeclared { .. } => (
                false,
                Some(OutboundRefusal::CapabilityUndeclared),
                RecoveryAction::InstallCapableVersion,
            ),
            Availability::Missing => (
                false,
                Some(OutboundRefusal::PackageMissing),
                RecoveryAction::InstallPackage,
            ),
            Availability::Unreadable => (
                false,
                Some(OutboundRefusal::StoreUnreadable),
                RecoveryAction::RepairStore,
            ),
        };
        RecoveryReport {
            // The local client never depends on this package.
            local_client_usable: true,
            capability_available,
            refusal,
            action,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Availability, EndpointCollaborationBoundary, OutboundAuthority, OutboundRefusal,
        RecoveryAction,
    };

    #[test]
    fn a_missing_package_refuses_outbound_and_reports_an_unavailable_recovery() {
        let boundary = EndpointCollaborationBoundary::missing();

        assert_eq!(
            boundary.outbound_authority(),
            Err(OutboundRefusal::PackageMissing)
        );
        let report = boundary.recovery();
        assert!(report.local_client_usable, "the local client stays usable");
        assert!(!report.capability_available);
        assert_eq!(report.refusal, Some(OutboundRefusal::PackageMissing));
        assert_eq!(report.action, RecoveryAction::InstallPackage);
    }

    #[test]
    fn a_disabled_package_is_a_cut_and_not_a_hidden_control() {
        let boundary = EndpointCollaborationBoundary::disabled("0.3.0");

        assert_eq!(
            boundary.outbound_authority(),
            Err(OutboundRefusal::PackageDisabled)
        );
        assert!(!boundary.recovery().capability_available);
        assert_eq!(boundary.recovery().action, RecoveryAction::EnablePackage);
    }

    #[test]
    fn an_installed_package_that_never_declared_the_capability_does_not_own_it() {
        let boundary = EndpointCollaborationBoundary::capability_undeclared("0.3.0");

        assert_eq!(
            boundary.outbound_authority(),
            Err(OutboundRefusal::CapabilityUndeclared)
        );
        assert_eq!(
            boundary.recovery().action,
            RecoveryAction::InstallCapableVersion
        );
    }

    #[test]
    fn an_unreadable_store_fails_closed_without_claiming_the_package_is_absent() {
        let boundary = EndpointCollaborationBoundary::unreadable();

        assert_eq!(
            boundary.outbound_authority(),
            Err(OutboundRefusal::StoreUnreadable)
        );
        assert_eq!(boundary.recovery().action, RecoveryAction::RepairStore);
    }

    #[test]
    fn an_active_package_permits_outbound_and_names_the_version_that_owns_it() {
        let boundary = EndpointCollaborationBoundary::active("0.3.0");

        assert_eq!(
            boundary.outbound_authority(),
            Ok(OutboundAuthority::Package {
                version: "0.3.0".to_owned()
            })
        );
        assert!(boundary.outbound_authority().unwrap().is_package_owned());
        assert_eq!(boundary.recovery().action, RecoveryAction::None);
        assert_eq!(boundary.availability().active_version(), Some("0.3.0"));
    }

    #[test]
    fn only_an_active_package_reports_an_active_version() {
        for boundary in [
            EndpointCollaborationBoundary::missing(),
            EndpointCollaborationBoundary::disabled("0.3.0"),
            EndpointCollaborationBoundary::capability_undeclared("0.3.0"),
            EndpointCollaborationBoundary::unreadable(),
        ] {
            assert_eq!(boundary.availability().active_version(), None);
            assert!(boundary.recovery().local_client_usable);
            assert!(boundary.outbound_authority().is_err());
        }
    }

    #[test]
    fn every_refusal_has_a_distinct_stable_reason() {
        let reasons = [
            OutboundRefusal::PackageMissing.reason(),
            OutboundRefusal::PackageDisabled.reason(),
            OutboundRefusal::CapabilityUndeclared.reason(),
            OutboundRefusal::StoreUnreadable.reason(),
        ];
        for (index, reason) in reasons.iter().enumerate() {
            assert!(reason.starts_with("endpoint_collaboration_"));
            assert!(
                !reasons[index + 1..].contains(reason),
                "{reason} names exactly one refusal"
            );
        }
    }

    #[test]
    fn the_boundary_carries_the_declared_availability_unchanged() {
        let boundary = EndpointCollaborationBoundary::over(Availability::Active {
            version: "9.9.9".to_owned(),
        });
        assert_eq!(boundary.availability().active_version(), Some("9.9.9"));
    }
}
