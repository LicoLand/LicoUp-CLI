//! The selection policy in force, adopted durably and bound to new tasks.
//!
//! A new task is admitted under one candidate-ordering policy: the revision a
//! feedback owner adopted, superseded or revoked before the task arrived. This
//! module owns that record and the three transitions over it, so the policy a
//! task was admitted under is an inspectable, restorable fact instead of a
//! value a reader recomputes.
//!
//! # What is recorded, and why the predecessor is the point
//!
//! [`SelectionPolicyRevision`] carries its own identity, the *identity of the
//! revision it replaced* (`parent_revision_id`), the provenance that produced
//! it and the [`SelectionPolicyPreferences`] it contributes. The predecessor is
//! not decoration: [`SelectionPolicyRegister::revoke`] restores the policy in
//! force before a revision by looking that revision up in the retained history,
//! so a record that dropped the predecessor could only refuse — it could never
//! restore. A register whose history no longer holds the recorded predecessor
//! fails closed with [`SelectionPolicyFailure::PredecessorUnknown`] rather than
//! resetting to "no policy" and calling that a restore.
//!
//! # Three transitions, each with one named refusal
//!
//! * [`SelectionPolicyRegister::adopt`] — the first revision: nothing is in
//!   force, so nothing precedes it (`parent_revision_id` is `None`). A second
//!   adoption is refused ([`SelectionPolicyFailure::AlreadyAdopted`]); a first
//!   adoption that names a predecessor is refused
//!   ([`SelectionPolicyFailure::PredecessorUnexpected`]).
//! * [`SelectionPolicyRegister::supersede`] — replace the revision in force,
//!   *stating* the predecessor it observed. A revision that is not in force is
//!   refused ([`SelectionPolicyFailure::NotAdopted`]) and so is a proposal
//!   built on a predecessor that is no longer in force
//!   ([`SelectionPolicyFailure::SupersedeStale`]), so a stale producer cannot
//!   overwrite a newer adoption.
//! * [`SelectionPolicyRegister::revoke`] — retire the revision in force and put
//!   its recorded predecessor back in force, exactly as it was recorded. Only
//!   the revision in force may be revoked
//!   ([`SelectionPolicyFailure::NotInForce`]). Revoking the first adoption
//!   restores the unadopted state, which is the honest answer when nothing
//!   preceded it; the retired revision stays in the retained history.
//!
//! # Bound at admission, never re-evaluated
//!
//! [`SelectionPolicyBinding`] is the value a newly admitted task captures
//! ([`SelectionPolicyRegister::capture`], or [`current_binding`] for the
//! admission boundary). It is a snapshot: a later supersede or revoke changes
//! what the *next* task is admitted under and leaves the binding an in-flight
//! task already holds untouched. Nothing here re-reads the register for a task
//! that is already admitted.
//!
//! The binding also decides how the policy reaches a new task:
//! [`SelectionPolicyBinding::apply_to_filters`] fills only the preference slots
//! the request left unset. A preference orders; it does not authorise. The
//! adopted revision never adds, removes or relaxes a required constraint, so
//! adopting a policy cannot widen what a task is allowed to run.
//!
//! # Where it is read
//!
//! The route receipt a new task is admitted with stamps the revision in force
//! into its `sourceRevisions` (that list already freezes the exact source
//! revisions a ranking was decided against), and `conversation.profile.candidates`
//! applies the captured binding before it ranks. The receipt is what the
//! durable workflow admission stores with the run, so an in-flight run keeps
//! the revision it was admitted under across a later adoption.
//!
//! # Storage
//!
//! The register is one `settings` entry of the existing client-state collection
//! (`licoup-client-state`), written with that store's atomic write. Reading is
//! a read-only projection: an absent, unreadable or invalid record applies no
//! preference instead of inventing one, which is the fail-closed direction
//! because a policy that was never recorded can never be restored.

use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};

use super::CandidateFilters;
use crate::platform::client_state::ClientStateStore;

/// The `settings` entry this owner reads and writes.
pub const SELECTION_POLICY_SETTINGS_KEY: &str = "selectionPolicyAdoption";

/// The revision name reported when no revision is in force.
pub const UNADOPTED_REVISION: &str = "unadopted";

const SETTINGS_COLLECTION: &str = "settings";

/// The candidate-ordering preferences one adopted revision contributes.
///
/// Every member is a preference, never a requirement: it orders the candidates
/// a request already allows, and the request's own preferences win.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SelectionPolicyPreferences {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_environment: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub preferred_skills: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub preferred_capabilities: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_task: Option<String>,
}

impl SelectionPolicyPreferences {
    /// Whether this revision states no preference at all.
    pub fn is_empty(&self) -> bool {
        self.preferred_model.is_none()
            && self.preferred_environment.is_none()
            && self.preferred_skills.is_empty()
            && self.preferred_capabilities.is_empty()
            && self.preferred_task.is_none()
    }
}

/// One adopted selection-policy revision and the identity it replaced.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SelectionPolicyRevision {
    /// The stable identity of this revision, chosen by its producer.
    pub revision_id: String,
    /// The identity of the revision in force before this one. `None` is the
    /// first adoption: nothing preceded it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_revision_id: Option<String>,
    /// The producer's own evidence reference for this revision. It is opaque
    /// here and never interpreted.
    pub provenance: String,
    pub preferences: SelectionPolicyPreferences,
}

/// One named refusal. Each transition refuses for its own reason, so a caller
/// never has to read a generic failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SelectionPolicyFailure {
    /// A revision identity or provenance is empty.
    InvalidRevision,
    /// A revision with this identity was already adopted.
    RevisionConflict,
    /// A revision is already in force; use `supersede` to replace it.
    AlreadyAdopted,
    /// A first adoption named a predecessor.
    PredecessorUnexpected,
    /// No revision is in force, so there is nothing to supersede or revoke.
    NotAdopted,
    /// The named predecessor is not the revision in force.
    SupersedeStale,
    /// The named revision is not the one in force.
    NotInForce,
    /// The recorded predecessor is no longer in the retained history, so this
    /// revoke cannot restore the policy that preceded the revision in force.
    PredecessorUnknown,
    /// The stored register is absent, unreadable or invalid.
    Unavailable,
}

impl SelectionPolicyFailure {
    /// The stable code a caller keys its report on.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidRevision => "selection_policy_invalid_revision",
            Self::RevisionConflict => "selection_policy_revision_conflict",
            Self::AlreadyAdopted => "selection_policy_already_adopted",
            Self::PredecessorUnexpected => "selection_policy_predecessor_unexpected",
            Self::NotAdopted => "selection_policy_not_adopted",
            Self::SupersedeStale => "selection_policy_supersede_stale",
            Self::NotInForce => "selection_policy_not_in_force",
            Self::PredecessorUnknown => "selection_policy_predecessor_unknown",
            Self::Unavailable => "selection_policy_unavailable",
        }
    }
}

impl Display for SelectionPolicyFailure {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl std::error::Error for SelectionPolicyFailure {}

/// The selection policy a newly admitted task is bound to.
///
/// It is a captured value, not a view: the task keeps it while later adoptions
/// change what the next task is admitted under.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectionPolicyBinding {
    /// The revision in force when the binding was captured. `None` means no
    /// revision was in force, so no preference is applied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision_id: Option<String>,
    #[serde(default, skip_serializing_if = "SelectionPolicyPreferences::is_empty")]
    pub preferences: SelectionPolicyPreferences,
}

impl SelectionPolicyBinding {
    /// The revision name a receipt reports for this binding.
    pub fn revision_name(&self) -> &str {
        self.revision_id.as_deref().unwrap_or(UNADOPTED_REVISION)
    }

    /// The request's filters under this policy.
    ///
    /// Only the preference slots the request left unset are filled. Required
    /// constraints are copied unchanged, so adopting a policy cannot widen,
    /// narrow or authorise a request that was decided above this owner.
    pub fn apply_to_filters(&self, filters: &CandidateFilters) -> CandidateFilters {
        let mut effective = filters.clone();
        if effective.preferred_model.is_none() {
            effective.preferred_model = self.preferences.preferred_model.clone();
        }
        if effective.preferred_environment.is_none() {
            effective.preferred_environment = self.preferences.preferred_environment.clone();
        }
        if effective.preferred_task.is_none() {
            effective.preferred_task = self.preferences.preferred_task.clone();
        }
        if effective.preferred_skills.is_empty() {
            effective.preferred_skills = self.preferences.preferred_skills.clone();
        }
        if effective.preferred_capabilities.is_empty() {
            effective.preferred_capabilities = self.preferences.preferred_capabilities.clone();
        }
        effective
    }
}

/// The durable record: the revision in force and the history that preceded it.
///
/// The history retains every revision this host ever adopted, in adoption
/// order, so a revoke restores the predecessor the revision recorded — exactly,
/// and including a predecessor that was itself superseded earlier. A revoked
/// revision stays in the history: nothing here rewrites what was adopted.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SelectionPolicyRegister {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    in_force_revision_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    history: Vec<SelectionPolicyRevision>,
}

impl SelectionPolicyRegister {
    /// The revision in force, if any.
    pub fn in_force(&self) -> Option<&SelectionPolicyRevision> {
        let in_force = self.in_force_revision_id.as_deref()?;
        self.history
            .iter()
            .find(|revision| revision.revision_id == in_force)
    }

    /// Every revision this host ever adopted, oldest first.
    pub fn history(&self) -> &[SelectionPolicyRevision] {
        &self.history
    }

    /// Capture the policy in force as the binding a new task is admitted under.
    pub fn capture(&self) -> SelectionPolicyBinding {
        match self.in_force() {
            Some(revision) => SelectionPolicyBinding {
                revision_id: Some(revision.revision_id.clone()),
                preferences: revision.preferences.clone(),
            },
            None => SelectionPolicyBinding::default(),
        }
    }

    /// Adopt the first revision, with nothing preceding it.
    pub fn adopt(
        &mut self,
        revision: SelectionPolicyRevision,
    ) -> Result<SelectionPolicyBinding, SelectionPolicyFailure> {
        if self.in_force_revision_id.is_some() {
            return Err(SelectionPolicyFailure::AlreadyAdopted);
        }
        if revision.parent_revision_id.is_some() {
            return Err(SelectionPolicyFailure::PredecessorUnexpected);
        }
        self.push(revision)?;
        let adopted = self
            .history
            .last()
            .map(|revision| revision.revision_id.clone());
        self.in_force_revision_id = adopted;
        Ok(self.capture())
    }

    /// Replace the revision in force with one built on the predecessor it
    /// states.
    pub fn supersede(
        &mut self,
        revision: SelectionPolicyRevision,
    ) -> Result<SelectionPolicyBinding, SelectionPolicyFailure> {
        let Some(in_force) = self.in_force_revision_id.clone() else {
            return Err(SelectionPolicyFailure::NotAdopted);
        };
        if revision.parent_revision_id.as_deref() != Some(in_force.as_str()) {
            return Err(SelectionPolicyFailure::SupersedeStale);
        }
        self.push(revision)?;
        let adopted = self
            .history
            .last()
            .map(|revision| revision.revision_id.clone());
        self.in_force_revision_id = adopted;
        Ok(self.capture())
    }

    /// Retire the named revision and restore the predecessor it recorded.
    pub fn revoke(
        &mut self,
        revision_id: &str,
    ) -> Result<SelectionPolicyBinding, SelectionPolicyFailure> {
        let Some(in_force) = self.in_force_revision_id.clone() else {
            return Err(SelectionPolicyFailure::NotAdopted);
        };
        if in_force != revision_id {
            return Err(SelectionPolicyFailure::NotInForce);
        }
        let retired = self
            .history
            .iter()
            .find(|revision| revision.revision_id == revision_id)
            .ok_or(SelectionPolicyFailure::NotInForce)?;
        match retired.parent_revision_id.clone() {
            None => {
                self.in_force_revision_id = None;
            }
            Some(predecessor) => {
                // The predecessor is restored from the retained record, never
                // rebuilt from the revoked revision's proposal.
                if !self
                    .history
                    .iter()
                    .any(|revision| revision.revision_id == predecessor)
                {
                    return Err(SelectionPolicyFailure::PredecessorUnknown);
                }
                self.in_force_revision_id = Some(predecessor);
            }
        }
        Ok(self.capture())
    }

    /// Whether this record is internally consistent: unique named revisions,
    /// every recorded predecessor an earlier entry of the history, and an
    /// in-force revision that the history still holds.
    fn is_consistent(&self) -> bool {
        for (position, revision) in self.history.iter().enumerate() {
            if revision.revision_id.is_empty() || revision.provenance.is_empty() {
                return false;
            }
            if let Some(parent) = revision.parent_revision_id.as_deref() {
                if !self.history[..position]
                    .iter()
                    .any(|earlier| earlier.revision_id == parent)
                {
                    return false;
                }
            }
            if self
                .history
                .iter()
                .filter(|candidate| candidate.revision_id == revision.revision_id)
                .count()
                != 1
            {
                return false;
            }
        }
        match self.in_force_revision_id.as_deref() {
            None => true,
            Some(in_force) => self
                .history
                .iter()
                .any(|revision| revision.revision_id == in_force),
        }
    }

    fn push(&mut self, revision: SelectionPolicyRevision) -> Result<(), SelectionPolicyFailure> {
        if revision.revision_id.is_empty() || revision.provenance.is_empty() {
            return Err(SelectionPolicyFailure::InvalidRevision);
        }
        if self
            .history
            .iter()
            .any(|adopted| adopted.revision_id == revision.revision_id)
        {
            return Err(SelectionPolicyFailure::RevisionConflict);
        }
        self.history.push(revision);
        Ok(())
    }
}

/// The register this host holds, read without opening or initializing a store.
pub fn load() -> Result<SelectionPolicyRegister, SelectionPolicyFailure> {
    let store =
        ClientStateStore::portable_read_only().map_err(|_| SelectionPolicyFailure::Unavailable)?;
    let settings = store
        .read_collection_read_only(SETTINGS_COLLECTION)
        .map_err(|_| SelectionPolicyFailure::Unavailable)?;
    register_from_settings(&settings)
}

/// The register a caller already opened, for a read-modify-write transition.
pub fn load_from_store(
    store: &ClientStateStore,
) -> Result<SelectionPolicyRegister, SelectionPolicyFailure> {
    let settings = store
        .read_collection(SETTINGS_COLLECTION)
        .map_err(|_| SelectionPolicyFailure::Unavailable)?;
    register_from_settings(&settings)
}

/// Persist the register where the admission boundary reads it.
pub fn store(
    register: &SelectionPolicyRegister,
) -> Result<SelectionPolicyRegister, SelectionPolicyFailure> {
    let store = ClientStateStore::portable().map_err(|_| SelectionPolicyFailure::Unavailable)?;
    store_in(store, register)
}

/// Persist the register through a store the caller already opened.
pub fn store_in(
    store: ClientStateStore,
    register: &SelectionPolicyRegister,
) -> Result<SelectionPolicyRegister, SelectionPolicyFailure> {
    if !register.is_consistent() {
        return Err(SelectionPolicyFailure::Unavailable);
    }
    let settings = store
        .read_collection(SETTINGS_COLLECTION)
        .map_err(|_| SelectionPolicyFailure::Unavailable)?;
    let mut settings = settings.as_object().cloned().unwrap_or_default();
    settings.insert(
        SELECTION_POLICY_SETTINGS_KEY.to_owned(),
        serde_json::to_value(register).map_err(|_| SelectionPolicyFailure::Unavailable)?,
    );
    store
        .write_collection(SETTINGS_COLLECTION, serde_json::Value::Object(settings))
        .map_err(|_| SelectionPolicyFailure::Unavailable)?;
    Ok(register.clone())
}

/// The policy a task being admitted now is bound to.
///
/// An unreadable or invalid record applies no preference: this owner cannot
/// restore what it never recorded, and it never invents one.
pub fn current_binding() -> SelectionPolicyBinding {
    load()
        .map(|register| register.capture())
        .unwrap_or_default()
}

/// Adopt the first revision durably.
pub fn adopt_policy(
    revision: SelectionPolicyRevision,
) -> Result<SelectionPolicyBinding, SelectionPolicyFailure> {
    let store = ClientStateStore::portable().map_err(|_| SelectionPolicyFailure::Unavailable)?;
    let mut register = load_from_store(&store)?;
    register.adopt(revision)?;
    store_in(store, &register)?;
    Ok(register.capture())
}

/// Supersede the revision in force durably.
pub fn supersede_policy(
    revision: SelectionPolicyRevision,
) -> Result<SelectionPolicyBinding, SelectionPolicyFailure> {
    let store = ClientStateStore::portable().map_err(|_| SelectionPolicyFailure::Unavailable)?;
    let mut register = load_from_store(&store)?;
    register.supersede(revision)?;
    store_in(store, &register)?;
    Ok(register.capture())
}

/// Revoke the revision in force durably and restore its predecessor.
pub fn revoke_policy(revision_id: &str) -> Result<SelectionPolicyBinding, SelectionPolicyFailure> {
    let store = ClientStateStore::portable().map_err(|_| SelectionPolicyFailure::Unavailable)?;
    let mut register = load_from_store(&store)?;
    register.revoke(revision_id)?;
    store_in(store, &register)?;
    Ok(register.capture())
}

fn register_from_settings(
    settings: &serde_json::Value,
) -> Result<SelectionPolicyRegister, SelectionPolicyFailure> {
    let Some(value) = settings.get(SELECTION_POLICY_SETTINGS_KEY) else {
        return Ok(SelectionPolicyRegister::default());
    };
    let register: SelectionPolicyRegister =
        serde_json::from_value(value.clone()).map_err(|_| SelectionPolicyFailure::Unavailable)?;
    if !register.is_consistent() {
        return Err(SelectionPolicyFailure::Unavailable);
    }
    Ok(register)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;

    fn revision(id: &str, parent: Option<&str>, model: &str) -> SelectionPolicyRevision {
        SelectionPolicyRevision {
            revision_id: id.to_owned(),
            parent_revision_id: parent.map(str::to_owned),
            provenance: format!("feedback:outcome/{id}"),
            preferences: SelectionPolicyPreferences {
                preferred_model: Some(model.to_owned()),
                ..SelectionPolicyPreferences::default()
            },
        }
    }

    /// A data home of this test's own. The override is thread-local, so one
    /// test never reads another's stored policy.
    struct DataHomeGuard {
        previous: Option<PathBuf>,
        root: PathBuf,
    }

    impl DataHomeGuard {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "licoup-selection-policy-{name}-{}",
                uuid::Uuid::new_v4()
            ));
            let previous = licoup_foundation::platform::paths::set_portable_data_dir_override(
                Some(root.clone()),
            );
            Self { previous, root }
        }
    }

    impl Drop for DataHomeGuard {
        fn drop(&mut self) {
            licoup_foundation::platform::paths::set_portable_data_dir_override(
                self.previous.take(),
            );
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn ranking_snapshot(
        membership_id: &str,
        model: &str,
    ) -> super::super::MembershipProfileSnapshot {
        serde_json::from_value(json!({
            "conversationId": "conversation:admission",
            "membershipId": membership_id,
            "agentId": membership_id,
            "intentRevision": 1,
            "responsibility": "member",
            "model": model,
            "capabilities": ["conversationDriver:supported"],
        }))
        .unwrap()
    }

    fn receipt_revision(receipt: &serde_json::Value) -> String {
        receipt["sourceRevisions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|source| source["source"] == "selectionPolicy")
            .expect("the receipt states the selection policy it was decided under")["revision"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    /// The falsification this test exists for: without the recorded predecessor
    /// identity a revoke can only refuse. It must never reset to "no policy"
    /// and report that as the restored predecessor.
    #[test]
    fn revoke_restores_the_predecessor_policy_exactly() {
        let mut register = SelectionPolicyRegister::default();
        let first = revision("policy:r1", None, "model-a");
        register.adopt(first.clone()).unwrap();
        let under_first = register.capture();
        register
            .supersede(revision("policy:r2", Some("policy:r1"), "model-b"))
            .unwrap();
        assert_eq!(register.capture().revision_id.as_deref(), Some("policy:r2"));

        let restored = register.revoke("policy:r2").unwrap();
        assert_eq!(
            restored, under_first,
            "identity and content must both return"
        );
        assert_eq!(register.in_force(), Some(&first));
        assert_eq!(
            register.history().len(),
            2,
            "history is retained, not rewritten"
        );
        assert!(register.is_consistent());

        // A record whose history lost the predecessor cannot restore it: the
        // revoke refuses instead of inventing an unadopted state.
        let mut orphaned: SelectionPolicyRegister = serde_json::from_value(json!({
            "inForceRevisionId": "policy:r2",
            "history": [{
                "revisionId": "policy:r2",
                "parentRevisionId": "policy:r1",
                "provenance": "feedback:outcome/policy:r2",
                "preferences": {"preferredModel": "model-b"},
            }],
        }))
        .unwrap();
        assert_eq!(
            orphaned.revoke("policy:r2"),
            Err(SelectionPolicyFailure::PredecessorUnknown)
        );
        assert_eq!(
            orphaned.capture().revision_id.as_deref(),
            Some("policy:r2"),
            "a refused revoke changes nothing"
        );
        assert!(!orphaned.is_consistent());
    }

    /// The falsification this test exists for: if a task's binding were read
    /// from the register when it is used, this admission would follow the later
    /// supersede. It must keep the revision it was admitted under.
    #[test]
    fn a_new_task_binding_is_frozen_at_admission() {
        let mut register = SelectionPolicyRegister::default();
        register
            .adopt(revision("policy:r1", None, "model-a"))
            .unwrap();
        let admitted = register.capture();
        let admitted_receipt =
            super::super::route_receipt_under("conversation:admission", &[], &admitted);

        register
            .supersede(revision("policy:r2", Some("policy:r1"), "model-b"))
            .unwrap();

        assert_eq!(admitted.revision_id.as_deref(), Some("policy:r1"));
        assert_eq!(
            admitted.preferences.preferred_model.as_deref(),
            Some("model-a")
        );
        assert_eq!(receipt_revision(&admitted_receipt), "policy:r1");
        assert_eq!(
            receipt_revision(&super::super::route_receipt_under(
                "conversation:admission",
                &[],
                &admitted,
            )),
            "policy:r1",
            "an admitted task keeps its binding after the next adoption"
        );

        let next = register.capture();
        assert_eq!(next.revision_id.as_deref(), Some("policy:r2"));
        assert_eq!(
            receipt_revision(&super::super::route_receipt_under(
                "conversation:admission",
                &[],
                &next,
            )),
            "policy:r2",
            "only the task admitted after the supersede runs under it"
        );
    }

    /// The policy reaches a new task as an ordering, and never as authority.
    #[test]
    fn the_adopted_preferences_order_a_new_task_without_widening_it() {
        let mut register = SelectionPolicyRegister::default();
        register
            .adopt(SelectionPolicyRevision {
                revision_id: "policy:prefer-b".to_owned(),
                parent_revision_id: None,
                provenance: "feedback:outcome/policy:prefer-b".to_owned(),
                preferences: SelectionPolicyPreferences {
                    preferred_model: Some("model-b".to_owned()),
                    preferred_skills: vec!["skill-b".to_owned()],
                    ..SelectionPolicyPreferences::default()
                },
            })
            .unwrap();
        let binding = register.capture();

        let snapshots = vec![
            ranking_snapshot("membership:a", "model-a"),
            ranking_snapshot("membership:b", "model-b"),
        ];
        let requested = CandidateFilters {
            required_capabilities: vec!["conversationDriver:supported".to_owned()],
            ..CandidateFilters::default()
        };
        let unpoliced = super::super::rank_candidates(snapshots.clone(), &requested).unwrap();
        let policed =
            super::super::rank_candidates(snapshots, &binding.apply_to_filters(&requested))
                .unwrap();
        assert_eq!(unpoliced[0].membership_id, "membership:a");
        assert_eq!(
            policed[0].membership_id, "membership:b",
            "the adopted preference orders the candidates a new task is admitted from"
        );
        assert_eq!(
            policed.len(),
            2,
            "a preference orders the allowed set, it does not shrink or widen it"
        );

        // The request's own preference wins, and required constraints are
        // copied unchanged: no adoption adds, removes or relaxes a requirement.
        let explicit = CandidateFilters {
            preferred_model: Some("model-a".to_owned()),
            preferred_capabilities: vec!["capability:requested".to_owned()],
            required_model: Some("model-a".to_owned()),
            ..CandidateFilters::default()
        };
        let effective = binding.apply_to_filters(&explicit);
        assert_eq!(effective.preferred_model.as_deref(), Some("model-a"));
        assert_eq!(
            effective.preferred_capabilities,
            vec!["capability:requested"]
        );
        assert_eq!(effective.required_model.as_deref(), Some("model-a"));
        assert_eq!(effective.preferred_skills, vec!["skill-b"]);
    }

    /// The record survives a restart, and the admission read finds it there.
    #[test]
    fn the_adopted_policy_is_persisted_for_the_next_admission() {
        let _data_home = DataHomeGuard::new("round-trip");
        assert_eq!(current_binding().revision_id, None);
        assert_eq!(current_binding().revision_name(), UNADOPTED_REVISION);

        let first = adopt_policy(revision("policy:r1", None, "model-a")).unwrap();
        assert_eq!(current_binding(), first);

        let second = supersede_policy(revision("policy:r2", Some("policy:r1"), "model-b")).unwrap();
        assert_eq!(current_binding(), second);

        let restored = revoke_policy("policy:r2").unwrap();
        assert_eq!(restored, first, "the restore is exact across the store");

        assert_eq!(
            adopt_policy(revision("policy:r3", None, "model-c")),
            Err(SelectionPolicyFailure::AlreadyAdopted)
        );
        assert_eq!(
            supersede_policy(revision("policy:r4", Some("policy:r2"), "model-d")),
            Err(SelectionPolicyFailure::SupersedeStale)
        );
        assert_eq!(
            revoke_policy("policy:r2"),
            Err(SelectionPolicyFailure::NotInForce)
        );
        assert_eq!(current_binding(), first);
    }

    /// An unreadable or invalid record applies no preference and cannot be
    /// silently adopted over.
    #[test]
    fn an_invalid_stored_register_applies_no_preference_and_is_not_overwritten() {
        let _data_home = DataHomeGuard::new("invalid");
        let store = ClientStateStore::portable().unwrap();
        let settings = store.read_collection(SETTINGS_COLLECTION).unwrap();
        let mut settings = settings.as_object().cloned().unwrap_or_default();
        settings.insert(
            SELECTION_POLICY_SETTINGS_KEY.to_owned(),
            json!({"inForceRevisionId": "policy:missing", "history": []}),
        );
        store
            .write_collection(SETTINGS_COLLECTION, serde_json::Value::Object(settings))
            .unwrap();

        assert_eq!(current_binding(), SelectionPolicyBinding::default());
        assert_eq!(
            adopt_policy(revision("policy:r1", None, "model-a")),
            Err(SelectionPolicyFailure::Unavailable),
            "an invalid record is not read as an empty one"
        );
    }
}
