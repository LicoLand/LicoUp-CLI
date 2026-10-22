//! Plan identity: what a lowered plan is bound to.
//!
//! The identity machinery is `acativa`'s: the semantics types, the key, the
//! recorded form, the profile and the retained plan are the engine's own
//! vocabulary, re-exported here. What this module owns is only the binding:
//! [`LICOUP_PLAN_IDENTITY`] carries this host's persisted line names
//! (`licoup.workflow.compiler`, `licoup.workflow.engine`,
//! `licoup.workflow.plan-key.v1`), and the helpers below close over it so no
//! caller can produce a record under a line this host cannot read back.
//!
//! The identity-taking boundaries the helpers cover are the ones where the
//! lines are written or read: a recorded binding types only against this
//! host's identity, and a binding digest commits the plan-key schema this
//! host persists. `acativa`'s `generic_*` shorthands and the `From<&PlanKey>`
//! impl on [`RecordedPlanKey`] bind the *generic* `acativa.workflow.*` lines
//! instead — they stay available for what they are, and nothing here records
//! through them.

use acativa::PlanIdentity;

pub use acativa::{
    CompiledWorkflow, CompilerSemantics, DefinitionRevision, EngineSemantics, HandoffReason,
    InterpreterProfile, LoweringCapabilities, LoweringRefusal, PlanInputError, PlanKey,
    PlanMismatch, RecordedPlanKey, RetainedPlan, RunAdmission,
};

/// The persisted identity lines this host binds.
///
/// Every plan record this build writes or reads is under these lines: the
/// compiler semantics records as `licoup.workflow.compiler.v1`, the engine
/// semantics as `licoup.workflow.engine.v1`, and binding digests commit the
/// `licoup.workflow.plan-key.v1` encoding. A record that declares a different
/// line — including the generic `acativa.workflow.*` lines — is not this
/// host's record and is refused rather than reinterpreted.
pub const LICOUP_PLAN_IDENTITY: PlanIdentity = PlanIdentity::new(
    "licoup.workflow.compiler",
    "licoup.workflow.engine",
    "licoup.workflow.plan-key.v1",
);

/// Name a version of this host's compiler semantics line.
///
/// [`CompilerSemantics::version`] takes the identity explicitly; this helper
/// binds it to [`LICOUP_PLAN_IDENTITY`]. `TryFrom<u16>` on the type names a
/// version of the *generic* line instead — the numeric check is the same, but
/// call sites naming this host's version use this helper so the boundary is
/// visible.
pub fn compiler_semantics_version(
    version: u16,
) -> std::result::Result<CompilerSemantics, PlanInputError> {
    CompilerSemantics::version(&LICOUP_PLAN_IDENTITY, version)
}

/// The recorded form of `semantics` under this host's lines:
/// `licoup.workflow.compiler.v{version}`.
pub fn compiler_semantics_wire(semantics: CompilerSemantics) -> String {
    semantics.wire(&LICOUP_PLAN_IDENTITY)
}

/// Type a recorded compiler semantics under this host's lines. A value that
/// is not a version of `licoup.workflow.compiler` — a content digest, a
/// different line's version — is refused.
pub fn compiler_semantics_from_wire(
    value: &str,
) -> std::result::Result<CompilerSemantics, PlanInputError> {
    CompilerSemantics::from_wire(&LICOUP_PLAN_IDENTITY, value)
}

/// Name a version of this host's engine semantics line.
pub fn engine_semantics_version(
    version: u16,
) -> std::result::Result<EngineSemantics, PlanInputError> {
    EngineSemantics::version(&LICOUP_PLAN_IDENTITY, version)
}

/// The recorded form of `semantics` under this host's lines:
/// `licoup.workflow.engine.v{version}`.
pub fn engine_semantics_wire(semantics: EngineSemantics) -> String {
    semantics.wire(&LICOUP_PLAN_IDENTITY)
}

/// Type a recorded engine semantics under this host's lines.
pub fn engine_semantics_from_wire(
    value: &str,
) -> std::result::Result<EngineSemantics, PlanInputError> {
    EngineSemantics::from_wire(&LICOUP_PLAN_IDENTITY, value)
}

/// The stable identity of `key` under this host's lines.
///
/// The digest commits `licoup.workflow.plan-key.v1`, so it is the identity a
/// persisted run of this host stays bound to; the same key digests
/// differently under any other identity on purpose.
pub fn plan_binding_digest(key: &PlanKey) -> String {
    key.binding_digest(&LICOUP_PLAN_IDENTITY)
}

/// The recorded form of `key` under this host's lines — what a checkpoint of
/// this host persists.
///
/// `RecordedPlanKey::from(&key)` binds the generic lines and produces a
/// record this host cannot read back; records this host writes go through
/// this helper.
pub fn record_plan_key(key: &PlanKey) -> RecordedPlanKey {
    RecordedPlanKey::record(key, &LICOUP_PLAN_IDENTITY)
}

/// Type a recorded binding under this host's lines. A record that declares a
/// different line is refused rather than reinterpreted.
pub fn typed_plan_key(recorded: &RecordedPlanKey) -> std::result::Result<PlanKey, PlanInputError> {
    recorded.typed(&LICOUP_PLAN_IDENTITY)
}

/// Whether `profile` may advance a run from the binding a checkpoint
/// recorded, typed under this host's lines.
pub fn admit_recorded_plan(
    profile: &InterpreterProfile,
    recorded: &RecordedPlanKey,
) -> RunAdmission {
    profile.admit_recorded(recorded, &LICOUP_PLAN_IDENTITY)
}

/// The identity a retained plan stays bound to, digested under this host's
/// lines.
pub fn retained_binding_digest(plan: &RetainedPlan) -> String {
    plan.binding_digest(&LICOUP_PLAN_IDENTITY)
}

/// Rebuild the lowered plan for a retained definition under this host's
/// schema identity — the same [`crate::WORKFLOW_SCHEMA_VERSION`] the original
/// lowering was admitted under.
pub fn rebuild_retained_plan(
    plan: &RetainedPlan,
) -> std::result::Result<CompiledWorkflow, crate::WorkflowValidationFailure> {
    plan.rebuild(&crate::workflow_schema())
        .map_err(crate::WorkflowValidationFailure::from)
}
