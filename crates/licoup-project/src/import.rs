//! The canonical plan document and its correspondence to a source.
//!
//! One typed import contract stands between an arbitrary source document and the
//! project domain. The source may be prose, a spreadsheet, an issue list or a
//! generated roadmap, and it stays exactly as it is: this owner never opens it,
//! never rewrites it and never keeps a copy of it. What crosses the boundary is
//! one canonical JSON document ([`PlanDocument`]) that a caller deliberately
//! produces, plus the attribution that says which source and which place in it
//! every work item came from.
//!
//! Five rules keep the boundary trustworthy:
//!
//! - **Identity is declared, never derived.** Project, plan, work item, source,
//!   role and capability are bounded caller-declared identities. Nothing here
//!   walks a directory or reads a document to name something.
//! - **Declarations are not facts.** A plan work item declares an outcome, the
//!   acceptance criteria it will be judged by, the results it takes and the
//!   roles that may act on it. It cannot carry, and this owner refuses to admit,
//!   any assertion that a runtime task ran, completed or was accepted: those
//!   facts belong to the work owner that observed them and are never imported.
//! - **Source correspondence is attribution.** Every work item carries the
//!   anchor it was read from, and every admitted work item is reported back with
//!   that anchor, so a disagreement can be taken to the exact place in the
//!   source instead of to a whole document.
//! - **Ambiguity is reported, never guessed.** An unknown field, a foreign
//!   schema, a duplicate identity, an input that names a work item this plan does
//!   not declare, and a cycle between declared inputs are all reported by code
//!   and by path before any effect exists. An empty declaration is refused by
//!   name rather than read as "remove everything".
//! - **Resolution is bounded and happens once.** [`PlanDocument::admit`] indexes
//!   the submitted work items and resolves every in-plan input against that one
//!   index, in work proportional to the document, and returns the whole report
//!   before anything is stored.
//!
//! Canonical JSON is the transport of this one model, not a second source
//! format family: the same types serialize the preview a caller can show and the
//! payload the apply path stores, so a field cannot mean one thing on the way in
//! and another on the way out.

use crate::dependency::{ArtifactReference, WorkRef, render_dependency_path};
use crate::failure::ProjectFailure;
use crate::identity::{PlanId, ProjectId, WorkItemId, declared_identifier};
use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;

/// The one canonical plan schema this owner admits.
///
/// A document that declares another schema is a different format family, and it
/// is refused by name so a caller learns that it must be converted deliberately
/// rather than trusting a best-effort reading.
pub const PLAN_DOCUMENT_SCHEMA: &str = "licoup.project-plan/v1";

/// Where an import that could not be admitted was refused.
///
/// The stage is declared once, next to every other project refusal, so a caller
/// that already switches on [`crate::ProjectFailure`] stages keeps one list.
pub use crate::failure::IMPORT_STAGE as PLAN_IMPORT_STAGE;

/// Largest number of work items one plan document may declare.
pub const MAX_PLAN_WORK_ITEMS: usize = 512;
/// Largest number of declared inputs one plan document may carry in total.
pub const MAX_PLAN_INPUTS: usize = 4096;
/// Largest number of declared acceptance criteria one work item may carry.
pub const MAX_PLAN_ACCEPTANCE: usize = 32;
/// Largest number of role references one work item may carry.
pub const MAX_PLAN_ROLES: usize = 16;
/// Largest declared outcome, acceptance criterion or source anchor.
pub const MAX_PLAN_TEXT_BYTES: usize = 4096;
/// Largest declared source locator.
///
/// The locator is attribution a person reads, so it is allowed to look like a
/// path or a heading. It is never opened, joined onto a root or listed.
pub const MAX_SOURCE_LOCATOR_BYTES: usize = 4096;

/// Declare the bounded identifier newtypes this module adds.
///
/// The rule is the identity rule the project owner already applies
/// ([`declared_identifier`]); only the refusal code and the bound differ, so a
/// role reference cannot smuggle a location or a credential that a project
/// identity would have refused.
macro_rules! import_identifier {
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
                    return Err(ProjectFailure::import($code));
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

import_identifier!(
    SourceId,
    "project_plan_source_identity_required",
    128,
    "source identity"
);
import_identifier!(
    SourceKind,
    "project_plan_source_kind_required",
    64,
    "source kind"
);
import_identifier!(
    RoleId,
    "project_plan_role_identity_required",
    128,
    "role identity"
);
import_identifier!(
    CapabilityId,
    "project_plan_capability_identity_required",
    128,
    "capability identity"
);

/// Where in a source document one declaration was read from.
///
/// The rule is deliberately looser than an identity's: attribution a person
/// reads may contain spaces, punctuation, a heading or a path-like locator. What
/// it may not be is absent, oversized or NUL-bearing. Nothing in this owner
/// opens it, joins it onto a root or lists it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct SourceLocator(String);

impl SourceLocator {
    /// Accept one declared locator, or refuse it by name.
    pub fn declare(value: impl Into<String>) -> Result<Self, ProjectFailure> {
        let value = value.into();
        if value.trim().is_empty() || value.len() > MAX_SOURCE_LOCATOR_BYTES || value.contains('\0')
        {
            return Err(ProjectFailure::import(
                "project_plan_source_locator_required",
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for SourceLocator {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::declare(value).map_err(serde::de::Error::custom)
    }
}

impl fmt::Display for SourceLocator {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Where a declared role or capability reference applies.
///
/// The scope is part of the reference, not a grant: it says which level of the
/// plan the caller declares the role for, and admitting it still asks the
/// existing role and connection policy that already answers for that role.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RoleScope {
    /// The role applies to the plan as a whole.
    Plan,
    /// The role applies to every work item of the plan's project.
    Project,
    /// The role applies to one declared work item.
    WorkItem,
}

impl RoleScope {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Plan => "plan",
            Self::Project => "project",
            Self::WorkItem => "work-item",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "plan" => Some(Self::Plan),
            "project" => Some(Self::Project),
            "work-item" => Some(Self::WorkItem),
            _ => None,
        }
    }
}

/// One reference into the role or capability policy that already exists.
///
/// This is a reference and nothing more: a role identity, the scope it is
/// declared for, and the capability it must hold when the plan names one. No
/// credential, membership or permission set is carried, and no field here could
/// hold one.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RoleReference {
    /// The role the existing policy already answers for.
    pub role_id: RoleId,
    /// The scope the caller declares the role for.
    pub scope: RoleScope,
    /// The capability the role must hold in that scope, when one is named.
    pub capability: Option<CapabilityId>,
}

/// Where the canonical document was read from.
///
/// The locator is attribution only. The owner stores it, reports it and never
/// opens it: the caller read the source under its own authorization, and this
/// record exists so a disagreement can be traced back to the place it came from.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceIdentity {
    /// The stable identity of the source document.
    pub source_id: SourceId,
    /// The declared kind of the source, in the caller's own vocabulary.
    pub source_kind: SourceKind,
    /// Where the source is, as a person reads it. Never opened here.
    pub locator: SourceLocator,
}

/// One declared work item of a plan.
///
/// The declaration is total and it is a *declaration*: an outcome, the criteria
/// it will be judged by, the results it takes, the roles declared for it, and
/// the anchor in the source it was read from. There is deliberately no field
/// that could assert a run, a completion or an acceptance.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlanWorkItem {
    /// The declared work-item identity, unique inside the plan.
    pub work_item_id: WorkItemId,
    /// What this work item is for.
    pub outcome: String,
    /// The criteria the work will be judged by. Declarations, not evidence:
    /// satisfying one is established by the owner that observed it.
    pub acceptance: Vec<String>,
    /// The declared results this work item takes from other work items.
    pub inputs: Vec<ArtifactReference>,
    /// The roles declared for this work item, as references.
    pub roles: Vec<RoleReference>,
    /// The anchor in the source this work item was read from.
    pub source_anchor: String,
}

/// One canonical plan document.
///
/// The document names the project and plan it belongs to as declarations. That
/// the plan identity is the registered project's own plan identity is decided at
/// admission against the store, not here.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlanDocument {
    /// The schema this document declares. Exactly [`PLAN_DOCUMENT_SCHEMA`].
    pub schema: String,
    /// The registered project this plan belongs to.
    pub project_id: ProjectId,
    /// The plan identity this document is a slice of.
    pub plan_id: PlanId,
    /// Where the document was read from.
    pub source: SourceIdentity,
    /// The declared work items, at least one.
    pub work_items: Vec<PlanWorkItem>,
}

/// One refusal, with the document path that produced it.
///
/// The path is part of the answer: "an unsupported field" is not actionable,
/// `workItems[2].status` is. Diagnostics never carry the source document's text
/// beyond the caller's own declaration.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportDiagnostic {
    /// The stable code a caller branches on.
    pub code: &'static str,
    /// The document path the diagnostic is about, when one applies.
    pub path: String,
    /// Why it was refused, in the owner's own words.
    pub detail: Option<String>,
}

impl ImportDiagnostic {
    fn at(code: &'static str, path: impl Into<String>) -> Self {
        Self {
            code,
            path: path.into(),
            detail: None,
        }
    }

    fn because(code: &'static str, path: impl Into<String>, detail: impl Into<String>) -> Self {
        let detail = detail.into();
        Self {
            code,
            path: path.into(),
            detail: (!detail.is_empty()).then_some(detail),
        }
    }
}

impl fmt::Display for ImportDiagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} at {}", self.code, self.path)?;
        match &self.detail {
            Some(detail) => write!(formatter, " ({detail})"),
            None => Ok(()),
        }
    }
}

/// One admitted work item with the source place it came from.
///
/// The mapping is the attribution the whole boundary exists for: a work item
/// that turns out to be wrong can be taken back to one anchored place instead of
/// to the document as a whole.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceMapping {
    /// The admitted work item.
    pub work_item_id: WorkItemId,
    /// The source document it was read from.
    pub source_id: SourceId,
    /// The anchor inside that source.
    pub source_anchor: String,
}

/// One plan document that passed admission.
///
/// Admission resolves the document's own references and reports what it
/// resolved. It stores nothing: the apply path decides what becomes durable, and
/// until it does, this value is the preview a caller can show a person.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanAdmission {
    /// The document as admitted.
    pub document: PlanDocument,
    /// One mapping per admitted work item, in document order.
    pub mapping: Vec<SourceMapping>,
    /// How many work items were admitted.
    pub work_item_count: usize,
    /// How many declared inputs were resolved against the plan index.
    pub input_count: usize,
}

impl PlanAdmission {
    /// The work items this admission declares, in document order.
    pub fn work_items(&self) -> &[PlanWorkItem] {
        &self.document.work_items
    }

    /// The declared inputs one admitted work item takes, by its identity.
    pub fn inputs_of(&self, work_item_id: &WorkItemId) -> Option<&[ArtifactReference]> {
        self.document
            .work_items
            .iter()
            .find(|item| &item.work_item_id == work_item_id)
            .map(|item| item.inputs.as_slice())
    }

    /// The anchor one admitted work item was read from.
    pub fn anchor_of(&self, work_item_id: &WorkItemId) -> Option<&SourceMapping> {
        self.mapping
            .iter()
            .find(|mapping| &mapping.work_item_id == work_item_id)
    }
}

/// The registered project one import names is not registered.
pub const IMPORT_PROJECT_UNAUTHORIZED: &str = "project_plan_import_project_unauthorized";
/// The named plan is not the plan identity the registered project carries.
pub const IMPORT_PLAN_MISMATCH: &str = "project_plan_import_plan_mismatch";
/// The state the caller expected is not the state the owner holds.
pub const IMPORT_STALE_APPLY: &str = "project_plan_import_stale_apply";
/// A stored import row could not be read back as the model it declares.
pub const IMPORT_RECORD_INVALID: &str = "project_plan_import_record_invalid";

/// One source-owned plan slice as the durable owner holds it.
///
/// The slice belongs to one `(project, source)` pair and to nothing else: a
/// second source over the same project owns a second slice, and a source over
/// another project cannot reach this one. The revision is how many admitted
/// documents this source has produced, and the digest is the content of the
/// last one.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportSlice {
    /// The registered project this slice belongs to.
    pub project_id: ProjectId,
    /// The plan identity the slice's documents declare.
    pub plan_id: PlanId,
    /// The stable identity of the source.
    pub source_id: SourceId,
    /// The declared kind of the source, in the caller's own vocabulary.
    pub source_kind: SourceKind,
    /// Where the source is, as a person reads it. Never opened.
    pub source_locator: SourceLocator,
    /// How many admitted documents this source has produced.
    pub revision: u64,
    /// The digest of the document admitted at that revision.
    pub digest: String,
    /// The work items the slice holds, in admission order.
    pub work_item_ids: Vec<WorkItemId>,
}

/// What one explicit import changes, or would change.
///
/// The same value answers the preview and the applied result, so a caller
/// showing a person what an import would do and the owner reporting what it did
/// cannot disagree about the fields.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanImportChange {
    /// The registered project the document belongs to.
    pub project_id: ProjectId,
    /// The plan identity the document declares.
    pub plan_id: PlanId,
    /// The source the document was read from.
    pub source_id: SourceId,
    /// The state this call starts from: the revision an apply must expect.
    pub revision: u64,
    /// The digest of the submitted document.
    pub digest: String,
    /// Whether the submitted document is exactly the stored revision.
    pub replayed: bool,
    /// Declared work items this slice did not hold.
    pub added: Vec<WorkItemId>,
    /// Declared work items this slice already held.
    pub unchanged: Vec<WorkItemId>,
    /// Stored work items the document omits. Retained, never deleted: an
    /// omission is not a deletion, and a source cannot cancel admitted work.
    pub retained: Vec<WorkItemId>,
    /// One anchor per declared work item, in document order.
    pub mapping: Vec<SourceMapping>,
    /// How many declared inputs the document carries.
    pub input_count: usize,
}

/// What one explicit import did.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanImportOutcome {
    /// Whether durable state changed. A replay of the stored revision changes
    /// nothing, so re-submitting the same document twice is one effect.
    pub applied: bool,
    /// The slice state after the call, with its source correspondence.
    pub change: PlanImportChange,
}

/// The fields the canonical document defines, by level.
///
/// The lists exist so an unknown field is reported by name instead of being
/// dropped by a permissive parser: a document carrying a field this model does
/// not define is a document whose meaning is not fully known.
const DOCUMENT_FIELDS: &[&str] = &["schema", "projectId", "planId", "source", "workItems"];
const SOURCE_FIELDS: &[&str] = &["sourceId", "sourceKind", "locator"];
const WORK_ITEM_FIELDS: &[&str] = &[
    "workItemId",
    "outcome",
    "acceptance",
    "inputs",
    "roles",
    "sourceAnchor",
];
const INPUT_FIELDS: &[&str] = &[
    "kind",
    "producerWorkItemId",
    "path",
    "projectId",
    "workItemId",
];
const ROLE_FIELDS: &[&str] = &["roleId", "scope", "capability"];

/// Field names that assert a run, a completion or an acceptance.
///
/// A source document commonly carries a progress column, and a conversion that
/// dropped it silently would lose the only meaning the source had. A conversion
/// that stored it would let an import mark work executed. Both are refusals: the
/// field is reported by name, and the caller decides what the plan should
/// declare instead.
const PROGRESS_FIELDS: &[&str] = &[
    "status",
    "state",
    "progress",
    "percentComplete",
    "done",
    "complete",
    "completed",
    "completedAt",
    "finished",
    "finishedAt",
    "startedAt",
    "executed",
    "execution",
    "accepted",
    "acceptanceState",
    "observed",
    "observation",
];

impl PlanDocument {
    /// Read one canonical document from JSON text.
    ///
    /// The text is parsed once, scanned once for fields this model does not
    /// define, and then converted into the types above. A document that carries
    /// an unknown or progress-bearing field is refused with every diagnostic it
    /// produced, because acting on part of an ambiguous document is exactly the
    /// silent invention this boundary exists to prevent.
    pub fn from_json(text: &str) -> Result<Self, Vec<ImportDiagnostic>> {
        let value: serde_json::Value = serde_json::from_str(text).map_err(|error| {
            vec![ImportDiagnostic::because(
                "project_plan_document_invalid",
                "",
                error.to_string(),
            )]
        })?;
        Self::from_value(value)
    }

    /// Read one canonical document from an already parsed JSON value.
    pub fn from_value(value: serde_json::Value) -> Result<Self, Vec<ImportDiagnostic>> {
        let mut diagnostics = Vec::new();
        scan_document(&value, &mut diagnostics);
        if !diagnostics.is_empty() {
            return Err(diagnostics);
        }
        serde_json::from_value(value).map_err(|error| {
            vec![ImportDiagnostic::because(
                "project_plan_document_invalid",
                "",
                error.to_string(),
            )]
        })
    }

    /// The canonical content digest of this document.
    ///
    /// The digest is what makes a repeated import recognisable as the same
    /// import: an apply that submits a document whose digest equals the stored
    /// revision's is a no-op rather than a second effect. The encoding is the
    /// document's own declared fields in declaration order, with a NUL between
    /// them — a byte no declared identity, locator or text may contain — so two
    /// different documents cannot encode to the same bytes by concatenation.
    pub fn digest(&self) -> String {
        let mut hasher = Sha256::new();
        let separate = |hasher: &mut Sha256, value: &str| {
            hasher.update(value.as_bytes());
            hasher.update([0]);
        };
        separate(&mut hasher, &self.schema);
        separate(&mut hasher, self.project_id.as_str());
        separate(&mut hasher, self.plan_id.as_str());
        separate(&mut hasher, self.source.source_id.as_str());
        separate(&mut hasher, self.source.source_kind.as_str());
        separate(&mut hasher, self.source.locator.as_str());
        for item in &self.work_items {
            hasher.update([1]);
            separate(&mut hasher, item.work_item_id.as_str());
            separate(&mut hasher, &item.outcome);
            for criterion in &item.acceptance {
                separate(&mut hasher, criterion);
            }
            for role in &item.roles {
                separate(&mut hasher, role.role_id.as_str());
                separate(&mut hasher, role.scope.as_str());
                separate(
                    &mut hasher,
                    role.capability
                        .as_ref()
                        .map(CapabilityId::as_str)
                        .unwrap_or(""),
                );
            }
            for input in &item.inputs {
                separate(&mut hasher, input.kind());
                separate(&mut hasher, &input.producer(&self.project_id).to_string());
                separate(&mut hasher, input.local_path().unwrap_or(""));
            }
            separate(&mut hasher, &item.source_anchor);
        }
        format!("{:x}", hasher.finalize())
    }

    /// Resolve this document's own declarations and report the correspondence.
    ///
    /// Every check runs against an index built from the submitted work items, so
    /// the work is proportional to the document and no part of the durable store
    /// is read or written. What admission cannot decide here — whether the plan
    /// identity belongs to a registered project, and whether a cross-project
    /// input names a registered project — is decided by the apply path against
    /// the store, with the same refusal vocabulary the dependency owner already
    /// publishes.
    pub fn admit(&self) -> Result<PlanAdmission, Vec<ImportDiagnostic>> {
        let mut diagnostics = Vec::new();
        let mut index: BTreeMap<&WorkItemId, usize> = BTreeMap::new();
        for (position, item) in self.work_items.iter().enumerate() {
            if index.insert(&item.work_item_id, position).is_some() {
                diagnostics.push(ImportDiagnostic::because(
                    "project_plan_work_item_duplicate",
                    format!("workItems[{position}].workItemId"),
                    item.work_item_id.to_string(),
                ));
            }
        }
        let mut edges: Vec<(WorkRef, WorkRef)> = Vec::new();
        for (position, item) in self.work_items.iter().enumerate() {
            let consumer = WorkRef::new(self.project_id.clone(), item.work_item_id.clone());
            let mut seen_inputs: BTreeSet<(String, String)> = BTreeSet::new();
            let mut seen_roles: BTreeSet<(String, &'static str, Option<String>)> = BTreeSet::new();
            for (role_position, role) in item.roles.iter().enumerate() {
                let key = (
                    role.role_id.to_string(),
                    role.scope.as_str(),
                    role.capability.as_ref().map(ToString::to_string),
                );
                if !seen_roles.insert(key) {
                    diagnostics.push(ImportDiagnostic::because(
                        "project_plan_role_duplicate",
                        format!("workItems[{position}].roles[{role_position}].roleId"),
                        role.role_id.to_string(),
                    ));
                }
            }
            for (input_position, input) in item.inputs.iter().enumerate() {
                let path = format!("workItems[{position}].inputs[{input_position}]");
                let producer = input.producer(&self.project_id);
                let key = (producer.to_string(), input.kind().to_owned());
                if !seen_inputs.insert(key) {
                    diagnostics.push(ImportDiagnostic::because(
                        "project_plan_input_duplicate",
                        path.clone(),
                        producer.to_string(),
                    ));
                }
                let inside_plan = match input {
                    ArtifactReference::Local { .. } => true,
                    ArtifactReference::CrossProject { project_id, .. } => {
                        project_id == &self.project_id
                    }
                };
                if inside_plan && !index.contains_key(&producer.work_item_id) {
                    diagnostics.push(ImportDiagnostic::because(
                        "project_plan_input_unknown_work_item",
                        path,
                        producer.to_string(),
                    ));
                    continue;
                }
                edges.push((consumer.clone(), producer));
            }
        }
        for (consumer, producer) in &edges {
            if let Some(cycle) = closing_cycle(&edges, consumer, producer) {
                diagnostics.push(ImportDiagnostic::because(
                    "project_plan_dependency_cycle",
                    "workItems",
                    render_dependency_path(&cycle),
                ));
                break;
            }
        }
        if !diagnostics.is_empty() {
            return Err(diagnostics);
        }
        let mapping = self
            .work_items
            .iter()
            .map(|item| SourceMapping {
                work_item_id: item.work_item_id.clone(),
                source_id: self.source.source_id.clone(),
                source_anchor: item.source_anchor.clone(),
            })
            .collect::<Vec<_>>();
        Ok(PlanAdmission {
            work_item_count: self.work_items.len(),
            input_count: self.work_items.iter().map(|item| item.inputs.len()).sum(),
            mapping,
            document: self.clone(),
        })
    }
}

/// The path that would close a cycle in one edge set.
///
/// The search follows the declared dependency direction, so the rendered path
/// reads as the edges a caller would have to remove. The edge set is a parameter
/// because admission asks the question about a document that is not stored yet.
fn closing_cycle(
    edges: &[(WorkRef, WorkRef)],
    consumer: &WorkRef,
    producer: &WorkRef,
) -> Option<Vec<WorkRef>> {
    if consumer == producer {
        return Some(vec![consumer.clone(), producer.clone()]);
    }
    let mut dependencies_of: BTreeMap<WorkRef, Vec<WorkRef>> = BTreeMap::new();
    for (edge_consumer, edge_producer) in edges {
        dependencies_of
            .entry(edge_consumer.clone())
            .or_default()
            .push(edge_producer.clone());
    }
    let mut parents: BTreeMap<WorkRef, Option<WorkRef>> =
        BTreeMap::from([(producer.clone(), None)]);
    let mut pending = VecDeque::from([producer.clone()]);
    while let Some(work) = pending.pop_front() {
        if &work == consumer {
            let mut path = Vec::new();
            let mut current = Some(work);
            while let Some(step) = current {
                path.push(step.clone());
                current = parents.get(&step).cloned().flatten();
            }
            path.reverse();
            let mut cycle = vec![consumer.clone()];
            cycle.extend(path);
            return Some(cycle);
        }
        for next in dependencies_of.get(&work).into_iter().flatten() {
            if !parents.contains_key(next) {
                parents.insert(next.clone(), Some(work.clone()));
                pending.push_back(next.clone());
            }
        }
    }
    None
}

/// Report every field the canonical model does not define.
///
/// The scan is deliberately shallow and total: every object in the document is
/// visited once, and a foreign key is reported with its full path. Progress-like
/// keys get their own code, because "this model has no such field" and "an
/// import cannot assert progress" call for different corrections from the
/// caller.
fn scan_document(value: &serde_json::Value, diagnostics: &mut Vec<ImportDiagnostic>) {
    let Some(document) = value.as_object() else {
        diagnostics.push(ImportDiagnostic::at("project_plan_document_required", ""));
        return;
    };
    scan_fields(document, DOCUMENT_FIELDS, "", diagnostics);
    match document.get("schema") {
        Some(serde_json::Value::String(schema)) if schema == PLAN_DOCUMENT_SCHEMA => {}
        Some(serde_json::Value::String(schema)) => diagnostics.push(ImportDiagnostic::because(
            "project_plan_schema_unsupported",
            "schema",
            schema.clone(),
        )),
        Some(_) => diagnostics.push(ImportDiagnostic::at(
            "project_plan_schema_required",
            "schema",
        )),
        None => diagnostics.push(ImportDiagnostic::at(
            "project_plan_schema_required",
            "schema",
        )),
    }
    match document.get("source") {
        Some(serde_json::Value::Object(source)) => {
            scan_fields(source, SOURCE_FIELDS, "source", diagnostics);
            scan_text(source, "locator", "source.locator", diagnostics);
        }
        Some(_) => diagnostics.push(ImportDiagnostic::at(
            "project_plan_source_required",
            "source",
        )),
        None => diagnostics.push(ImportDiagnostic::at(
            "project_plan_source_required",
            "source",
        )),
    }
    match document.get("workItems") {
        Some(serde_json::Value::Array(items)) => {
            if items.is_empty() {
                diagnostics.push(ImportDiagnostic::because(
                    "project_plan_work_items_required",
                    "workItems",
                    "an empty declaration is not a deletion",
                ));
            }
            if items.len() > MAX_PLAN_WORK_ITEMS {
                diagnostics.push(ImportDiagnostic::because(
                    "project_plan_too_large",
                    "workItems",
                    items.len().to_string(),
                ));
            }
            let mut inputs = 0usize;
            for (position, item) in items.iter().enumerate() {
                let path = format!("workItems[{position}]");
                let Some(work) = item.as_object() else {
                    diagnostics.push(ImportDiagnostic::at(
                        "project_plan_work_item_required",
                        &path,
                    ));
                    continue;
                };
                scan_fields(work, WORK_ITEM_FIELDS, &path, diagnostics);
                scan_text(work, "outcome", &format!("{path}.outcome"), diagnostics);
                scan_text(
                    work,
                    "sourceAnchor",
                    &format!("{path}.sourceAnchor"),
                    diagnostics,
                );
                match work.get("acceptance") {
                    Some(serde_json::Value::Array(criteria)) => {
                        if criteria.len() > MAX_PLAN_ACCEPTANCE {
                            diagnostics.push(ImportDiagnostic::because(
                                "project_plan_too_large",
                                format!("{path}.acceptance"),
                                criteria.len().to_string(),
                            ));
                        }
                        for (criterion, value) in criteria.iter().enumerate() {
                            scan_declared_text(
                                value,
                                &format!("{path}.acceptance[{criterion}]"),
                                diagnostics,
                            );
                        }
                    }
                    Some(_) => diagnostics.push(ImportDiagnostic::at(
                        "project_plan_acceptance_required",
                        format!("{path}.acceptance"),
                    )),
                    None => {}
                }
                if let Some(serde_json::Value::Array(declared)) = work.get("inputs") {
                    inputs += declared.len();
                    if inputs > MAX_PLAN_INPUTS {
                        diagnostics.push(ImportDiagnostic::because(
                            "project_plan_too_large",
                            format!("{path}.inputs"),
                            inputs.to_string(),
                        ));
                    }
                    for (position, input) in declared.iter().enumerate() {
                        scan_input(input, &format!("{path}.inputs[{position}]"), diagnostics);
                    }
                }
                if let Some(serde_json::Value::Array(roles)) = work.get("roles") {
                    if roles.len() > MAX_PLAN_ROLES {
                        diagnostics.push(ImportDiagnostic::because(
                            "project_plan_too_large",
                            format!("{path}.roles"),
                            roles.len().to_string(),
                        ));
                    }
                    for (position, role) in roles.iter().enumerate() {
                        scan_role(role, &format!("{path}.roles[{position}]"), diagnostics);
                    }
                }
            }
        }
        Some(_) => diagnostics.push(ImportDiagnostic::at(
            "project_plan_work_items_required",
            "workItems",
        )),
        None => diagnostics.push(ImportDiagnostic::at(
            "project_plan_work_items_required",
            "workItems",
        )),
    }
}

/// Report every field one nested object does not define.
fn scan_fields(
    object: &serde_json::Map<String, serde_json::Value>,
    known: &[&str],
    path: &str,
    diagnostics: &mut Vec<ImportDiagnostic>,
) {
    for field in object.keys() {
        if known.contains(&field.as_str()) {
            continue;
        }
        let field_path = if path.is_empty() {
            field.clone()
        } else {
            format!("{path}.{field}")
        };
        if PROGRESS_FIELDS.contains(&field.as_str()) {
            diagnostics.push(ImportDiagnostic::because(
                "project_plan_progress_not_admitted",
                field_path,
                "a plan declares work; a run, a completion and an acceptance are established by the owner that observed them",
            ));
        } else {
            diagnostics.push(ImportDiagnostic::at(
                "project_plan_unsupported_field",
                field_path,
            ));
        }
    }
}

/// Report one declared input that this model does not define.
fn scan_input(value: &serde_json::Value, path: &str, diagnostics: &mut Vec<ImportDiagnostic>) {
    let Some(input) = value.as_object() else {
        diagnostics.push(ImportDiagnostic::at("project_plan_input_required", path));
        return;
    };
    scan_fields(input, INPUT_FIELDS, path, diagnostics);
    match input.get("kind") {
        Some(serde_json::Value::String(kind)) if kind == "local" => {
            scan_text(input, "path", &format!("{path}.path"), diagnostics);
        }
        Some(serde_json::Value::String(kind)) if kind == "cross-project" => {}
        Some(serde_json::Value::String(kind)) => diagnostics.push(ImportDiagnostic::because(
            "project_plan_input_kind_unsupported",
            format!("{path}.kind"),
            kind.clone(),
        )),
        Some(_) => diagnostics.push(ImportDiagnostic::at(
            "project_plan_input_kind_unsupported",
            format!("{path}.kind"),
        )),
        None => diagnostics.push(ImportDiagnostic::at(
            "project_plan_input_kind_unsupported",
            format!("{path}.kind"),
        )),
    }
}

/// Report one declared role reference that this model does not define.
fn scan_role(value: &serde_json::Value, path: &str, diagnostics: &mut Vec<ImportDiagnostic>) {
    let Some(role) = value.as_object() else {
        diagnostics.push(ImportDiagnostic::at(
            "project_plan_role_reference_required",
            path,
        ));
        return;
    };
    scan_fields(role, ROLE_FIELDS, path, diagnostics);
}

/// Report one declared text field that is absent, empty or oversized.
fn scan_text(
    object: &serde_json::Map<String, serde_json::Value>,
    field: &str,
    path: &str,
    diagnostics: &mut Vec<ImportDiagnostic>,
) {
    match object.get(field) {
        Some(value) => scan_declared_text(value, path, diagnostics),
        None => diagnostics.push(ImportDiagnostic::at("project_plan_text_required", path)),
    }
}

/// Report one declared text value that is absent, empty or oversized.
fn scan_declared_text(
    value: &serde_json::Value,
    path: &str,
    diagnostics: &mut Vec<ImportDiagnostic>,
) {
    match value {
        serde_json::Value::String(text)
            if !text.trim().is_empty()
                && text.len() <= MAX_PLAN_TEXT_BYTES
                && !text.contains('\0') => {}
        _ => diagnostics.push(ImportDiagnostic::at("project_plan_text_required", path)),
    }
}
