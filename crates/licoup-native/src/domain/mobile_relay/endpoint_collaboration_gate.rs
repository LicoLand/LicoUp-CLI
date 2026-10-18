//! The endpoint collaboration authority check every outbound send passes.
//!
//! [`crate::platform::extension_packages::endpoint_collaboration`] resolves what
//! the host knows about the optional endpoint collaboration package, and this
//! module is the one place the sending half of
//! [`crate::domain::mobile_relay`] asks it. Nothing here opens a store, performs a
//! cryptographic operation or classifies a transport outcome: it answers whether
//! this client may emit outbound endpoint traffic at all, and refuses with the
//! owner's own stable reason when it may not.
//!
//! The check is deliberately at the send entries rather than in the user
//! interface: a disabled or uninstalled package must cut the capability, and a
//! hidden control is not a cut.

use anyhow::{Result, anyhow};

use crate::platform::extension_packages::endpoint_collaboration::{
    EndpointCollaborationGate, EndpointOutboundAuthority, endpoint_collaboration_gate,
};

/// The outbound authority one gate currently holds.
///
/// [`EndpointOutboundAuthority::LegacyInKernel`] is the pre-package path: the
/// composition has provisioned no package authority, so the kernel's own
/// implementation is what is running. It is not a grant, and it is named as such
/// rather than reported as package authority.
pub(in crate::domain::mobile_relay) fn authority_of(
    gate: &EndpointCollaborationGate,
) -> Result<EndpointOutboundAuthority> {
    gate.authority()
        .map_err(|refusal| anyhow!("{}", refusal.reason()))
}

/// The outbound authority this process currently holds.
#[cfg(test)]
pub(in crate::domain::mobile_relay) fn outbound_authority() -> Result<EndpointOutboundAuthority> {
    authority_of(endpoint_collaboration_gate())
}

/// Refuse an outbound send one gate's authority does not permit.
pub(in crate::domain::mobile_relay) fn ensure_permitted_by(
    gate: &EndpointCollaborationGate,
) -> Result<()> {
    authority_of(gate).map(|_| ())
}

/// Refuse an outbound send the installed package authority does not permit.
pub(in crate::domain::mobile_relay) fn ensure_outbound_permitted() -> Result<()> {
    ensure_permitted_by(endpoint_collaboration_gate())
}

#[cfg(test)]
mod tests {
    use super::{ensure_permitted_by, outbound_authority};
    use crate::platform::extension_packages::endpoint_collaboration::{
        EndpointCollaborationAvailability, EndpointCollaborationGate, EndpointOutboundAuthority,
    };

    /// One test's own gate, so no test depends on the process-wide installation
    /// order: the production gate belongs to the composition.
    fn gate(availability: Option<EndpointCollaborationAvailability>) -> EndpointCollaborationGate {
        let gate = EndpointCollaborationGate::new();
        if let Some(availability) = availability {
            gate.install(availability);
        }
        gate
    }

    #[test]
    fn a_package_that_owns_the_path_permits_the_send() {
        let gate = gate(Some(EndpointCollaborationAvailability::Active {
            version: "0.3.0".to_owned(),
        }));

        assert_eq!(
            gate.authority(),
            Ok(EndpointOutboundAuthority::Package {
                version: "0.3.0".to_owned()
            })
        );
        assert!(ensure_permitted_by(&gate).is_ok());
    }

    #[test]
    fn a_disabled_package_reaches_the_send_as_the_owners_own_reason() {
        let gate = gate(Some(EndpointCollaborationAvailability::Disabled {
            version: "0.3.0".to_owned(),
        }));

        let refused = ensure_permitted_by(&gate).unwrap_err().to_string();
        assert_eq!(refused, "endpoint_collaboration_package_disabled");
    }

    #[test]
    fn a_missing_package_refuses_the_send_rather_than_hiding_it() {
        let gate = gate(Some(EndpointCollaborationAvailability::Missing));

        let refused = ensure_permitted_by(&gate).unwrap_err().to_string();
        assert_eq!(refused, "endpoint_collaboration_package_absent");
    }

    #[test]
    fn an_unprovisioned_process_reports_the_pre_package_path_as_such() {
        // The production gate is read, never installed into: which package the
        // running client has is the composition's answer, not this suite's.
        // `outbound_authority` answers with an error type that carries no
        // equality, so the variant is matched rather than compared.
        assert!(matches!(
            outbound_authority(),
            Ok(EndpointOutboundAuthority::LegacyInKernel)
        ));
    }
}
