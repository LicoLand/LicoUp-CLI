//! The one seam every mutating maintenance operation passes through.
//!
//! Replacing an installed version, or activating one, changes state that running
//! work is reading. The decision that no such work is in flight belongs to one
//! owner: `UPDATE-IDLE-ADMISSION` composes the native idle guard from the
//! canonical execution and process owners and publishes the real verdict. This
//! module is the seam: it maps that verdict into the stable refusals this surface
//! publishes, and nothing else.
//!
//! - There is exactly one way to obtain a [`MaintenancePermit`]: ask
//!   [`PackageMaintenanceAdmission::admit`]. Its field is private, so no caller can
//!   construct one, and no route can hand itself permission.
//! - The verdict arrives as data, read for the data root the operation would
//!   change. A decision nobody read is a refusal — never an assumed idle host.
//! - Read-only work never comes here at all. Checking for an update and reading
//!   the catalogue are not mutations and are not gated.
//!
//! # Why the verdict is a parameter
//!
//! The guard's decision lives in the domain layer and the package store lives
//! here, and this layer does not reach into that one. The composition that knows
//! both — the native command surface — reads the guard for the data home a route
//! was given and hands the verdict in, exactly as the guard's own module
//! describes installing an answer at the composition point. The seam still owns
//! every refusal code, so a caller cannot publish its own vocabulary for a
//! decision it did not make.
//!
//! Asking is not holding. [`PackageMaintenanceAdmission::admit`] answers whether the
//! operation may proceed; the caller that actually changes installed state takes
//! the durable close-admission barrier itself through the guard's own entry
//! (`hold_package_activation_admission`) and releases it on success or abort.
//! Keeping the two apart is what lets a read-only preview ask the same question a
//! mutating route asks without closing admission for a host that was never going
//! to be changed.

use licoup_application::{ApplicationFailure, RecoveryAction};

/// The component a maintenance refusal names.
pub(crate) const COMPONENT: &str = "extension_packages_maintenance";

/// The refusal code for a maintenance decision this seam did not receive.
///
/// Distinct from "work is in flight": nothing was decided, and the safe answer is
/// still a refusal.
pub const ADMISSION_DECISION_UNREADABLE: &str = "package_maintenance_decision_unreadable";

/// The refusal code the guard reports for unfinished local work.
pub const ADMISSION_WORK_IN_FLIGHT: &str = "package_maintenance_work_in_flight";

/// The refusal code the guard reports when a maintenance switch already holds
/// the close-admission barrier.
///
/// Qualified with `PACKAGE_` because this is the package surface's own
/// vocabulary: the work-admission seam publishes `maintenance_admission_closed`
/// for the same situation on the migration and generation-replacement paths, and
/// the two codes must stay distinguishable.
pub const PACKAGE_ADMISSION_CLOSED: &str = "package_maintenance_admission_closed";

/// The stage every refusal from this seam reports.
pub const STAGE: &str = "extension/package-maintenance";

/// The owner of the real idle guard, named in every refusal so a caller is not
/// left guessing which decision refused it.
pub const GUARD_OWNER: &str = "UPDATE-IDLE-ADMISSION";

/// A mutating maintenance operation that needs the idle guard.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MaintenanceOperation {
    /// Replace the installed bytes of a package version with another.
    UpdateApply,
    /// Start a generation of an installed package version.
    Activation,
}

impl MaintenanceOperation {
    pub const fn wire_name(self) -> &'static str {
        match self {
            Self::UpdateApply => "update-apply",
            Self::Activation => "activation",
        }
    }
}

impl std::fmt::Display for MaintenanceOperation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.wire_name())
    }
}

/// One request for maintenance admission.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MaintenanceRequest {
    pub operation: MaintenanceOperation,
    pub package_id: String,
    pub version: String,
}

impl MaintenanceRequest {
    pub fn new(
        operation: MaintenanceOperation,
        package_id: impl Into<String>,
        version: impl Into<String>,
    ) -> Self {
        Self {
            operation,
            package_id: package_id.into(),
            version: version.into(),
        }
    }
}

/// The maintenance verdict, in this seam's vocabulary.
///
/// It is the seam's own type so the refusal codes have one owner. The native
/// guard's decision is mapped into it by the composition that reads the guard.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IdleVerdict {
    /// No locally owned work is in flight and no switch holds the barrier.
    Idle,
    /// Work is still unfinished; maintenance is refused and the reason travels.
    Busy,
    /// A maintenance switch already closed admission for this data root.
    Closed,
    /// The decision could not be read. Never an assumption of an idle host.
    Unavailable,
}

/// The native maintenance-admission seam for the package surface.
///
/// A zero-sized value: admission is a decision about shared state on disk, not
/// per-caller state, so there is nothing for a caller to hold. Holding one grants
/// nothing; the permit is what admits one operation.
///
/// Named for the surface it guards because the crate also carries the
/// generation-replacement admission port, `resources::MaintenanceAdmission`,
/// which the crate-root composition answers.
#[derive(Clone, Copy, Debug, Default)]
pub struct PackageMaintenanceAdmission;

/// Permission to perform one mutating maintenance operation.
///
/// Not constructible outside this module. Possessing one means the seam was given
/// an idle verdict for the data root the operation names.
#[derive(Debug)]
pub struct MaintenancePermit {
    operation: MaintenanceOperation,
}

impl MaintenancePermit {
    pub fn operation(&self) -> MaintenanceOperation {
        self.operation
    }
}

impl PackageMaintenanceAdmission {
    pub const fn new() -> Self {
        Self
    }

    /// Decide one mutating maintenance operation from the guard's verdict.
    ///
    /// Read-only callers never reach this: a preview, a catalogue read or an
    /// update check is not maintenance and is not gated.
    ///
    /// A permit is a decision, not a hold. The caller that changes installed
    /// state takes the close-admission barrier through the guard's own entry and
    /// releases it when the change succeeds or aborts.
    pub fn admit(
        &self,
        verdict: IdleVerdict,
        request: &MaintenanceRequest,
    ) -> Result<MaintenancePermit, ApplicationFailure> {
        let refusal_code = match verdict {
            IdleVerdict::Idle => {
                return Ok(MaintenancePermit {
                    operation: request.operation,
                });
            }
            IdleVerdict::Busy => ADMISSION_WORK_IN_FLIGHT,
            IdleVerdict::Closed => PACKAGE_ADMISSION_CLOSED,
            // Not "idle" and not "busy": no decision arrived, and that is the one
            // value that cannot be mistaken for a safe answer.
            IdleVerdict::Unavailable => ADMISSION_DECISION_UNREADABLE,
        };
        // Retryable and with a real next step in every case: unsettled work
        // settles, a held barrier is released by its holder, and an unreadable
        // decision is readable once its store is. `NotAttempted` is exact here —
        // nothing on the mutation path ran.
        Err(ApplicationFailure::retryable(refusal_code, STAGE)
            .with_recovery(RecoveryAction::RetryAfterRecovery)
            .with_component(COMPONENT)
            .with_field("maintenance")
            .with_presentation_arg("operation", request.operation.wire_name())
            .with_presentation_arg("package", request.package_id.as_str())
            .with_presentation_arg("guardOwner", GUARD_OWNER))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(operation: MaintenanceOperation) -> MaintenanceRequest {
        MaintenanceRequest::new(operation, "example.specialist.echo", "2.0.0")
    }

    /// The guard's idle verdict is what admits: both mutating operations may
    /// proceed, and the permit names the operation it was issued for.
    #[test]
    fn an_idle_verdict_admits_both_mutating_operations() {
        let admission = PackageMaintenanceAdmission::new();
        for operation in [
            MaintenanceOperation::UpdateApply,
            MaintenanceOperation::Activation,
        ] {
            let permit = admission
                .admit(IdleVerdict::Idle, &request(operation))
                .expect("an idle host admits maintenance");
            assert_eq!(permit.operation(), operation);
        }
    }

    /// Every other verdict refuses with its own stable code, and the code says
    /// which answer arrived rather than collapsing them into one.
    #[test]
    fn every_non_idle_verdict_refuses_with_its_own_stable_code() {
        let admission = PackageMaintenanceAdmission::new();
        for (verdict, code) in [
            (IdleVerdict::Busy, ADMISSION_WORK_IN_FLIGHT),
            (IdleVerdict::Closed, PACKAGE_ADMISSION_CLOSED),
            (IdleVerdict::Unavailable, ADMISSION_DECISION_UNREADABLE),
        ] {
            for operation in [
                MaintenanceOperation::UpdateApply,
                MaintenanceOperation::Activation,
            ] {
                let failure = admission
                    .admit(verdict, &request(operation))
                    .expect_err("a non-idle verdict refuses");
                assert_eq!(failure.code, code);
                assert_eq!(failure.stage, STAGE);
                assert_eq!(failure.component.as_ref(), COMPONENT);
                assert_eq!(failure.field.as_deref(), Some("maintenance"));
                assert!(
                    failure.retryable,
                    "every non-idle verdict has a real next step"
                );
                assert_eq!(
                    failure.effect,
                    licoup_application::EffectCertainty::NotAttempted,
                    "nothing on the mutation path ran"
                );
            }
        }
        assert_eq!(
            ADMISSION_WORK_IN_FLIGHT,
            "package_maintenance_work_in_flight"
        );
        assert_eq!(
            PACKAGE_ADMISSION_CLOSED, "package_maintenance_admission_closed",
            "the refusal is a maintenance one, not the guard's own release code"
        );
        assert_eq!(
            ADMISSION_DECISION_UNREADABLE,
            "package_maintenance_decision_unreadable"
        );
    }

    /// Every refusal names what was refused and who owns the guard, so it is a
    /// report and not an opaque failure.
    #[test]
    fn every_refusal_names_the_operation_the_package_and_the_guard_owner() {
        assert_eq!(GUARD_OWNER, "UPDATE-IDLE-ADMISSION");
        let failure = PackageMaintenanceAdmission::new()
            .admit(
                IdleVerdict::Closed,
                &request(MaintenanceOperation::UpdateApply),
            )
            .expect_err("refused");
        let args = failure
            .presentation_args
            .iter()
            .map(|(key, value)| (key, value))
            .collect::<Vec<_>>();
        assert!(args.contains(&("operation", "update-apply")));
        assert!(args.contains(&("package", "example.specialist.echo")));
        assert!(args.contains(&("guardOwner", "UPDATE-IDLE-ADMISSION")));
    }

    /// The wire names are the ones the generated bridge publishes.
    #[test]
    fn the_wire_names_are_stable() {
        assert_eq!(
            MaintenanceOperation::UpdateApply.wire_name(),
            "update-apply"
        );
        assert_eq!(MaintenanceOperation::Activation.wire_name(), "activation");
        assert_eq!(
            MaintenanceOperation::UpdateApply.to_string(),
            "update-apply"
        );
        assert_eq!(MaintenanceOperation::Activation.to_string(), "activation");
    }
}
