use licoup_conversation::continuity::{
    ContinuityCandidateIdentity, ContinuityDatasetSplit, ContinuityFailure, ContinuityFailureCode,
    ContinuityQualificationRecord, ContinuityQualificationResult, QualificationPort,
};

use super::eligibility::{RequestKind, admit_automatic_from_record};
use super::evaluate::{QualificationAssessment, apply_assessment, evaluate_bundle};
use super::evidence::{EvidenceBundle, EvidenceStore, admit_identity, identity_changed};
use super::failure::invalid_request;
use super::policy::QualificationPolicy;

#[derive(Clone, Debug)]
pub struct QualificationService {
    policy: QualificationPolicy,
    store: EvidenceStore,
}

impl Default for QualificationService {
    fn default() -> Self {
        Self::draft_port()
    }
}

impl QualificationService {
    pub fn with_policy(policy: QualificationPolicy) -> Self {
        Self {
            policy,
            store: EvidenceStore::default(),
        }
    }

    pub fn draft_port() -> Self {
        Self::with_policy(QualificationPolicy::draft_1())
    }

    pub fn policy(&self) -> &QualificationPolicy {
        &self.policy
    }

    pub fn ingest_immutable(&mut self, bundle: EvidenceBundle) -> Result<(), ContinuityFailure> {
        self.store.ingest(bundle)
    }

    pub fn has_evidence(
        &self,
        responsibility_id: &str,
        identity: &ContinuityCandidateIdentity,
    ) -> bool {
        self.store.get(responsibility_id, identity).is_some()
    }

    pub fn withdraw_for_identity_change(
        &mut self,
        responsibility_id: &str,
        previous: &ContinuityCandidateIdentity,
        current: &ContinuityCandidateIdentity,
    ) -> Result<(), ContinuityFailure> {
        admit_identity(previous)?;
        admit_identity(current)?;
        if !identity_changed(previous, current) {
            return Ok(());
        }
        self.store.withdraw(responsibility_id, previous)
    }

    pub fn assess(
        &self,
        responsibility_id: &str,
        identity: &ContinuityCandidateIdentity,
    ) -> Result<QualificationAssessment, ContinuityFailure> {
        let query = query_record(responsibility_id, identity.clone());
        let record = self.lookup_record(&query)?;
        let stale = record.result == ContinuityQualificationResult::Stale;
        let bundle = self
            .store
            .get(responsibility_id, identity)
            .cloned()
            .unwrap_or_else(|| EvidenceBundle {
                responsibility_id: responsibility_id.to_owned(),
                identity: identity.clone(),
                observations: Vec::new(),
                evidence_class: super::evidence::EvidenceClass::Synthetic,
                provenance: None,
            });
        Ok(evaluate_bundle(&bundle, &self.policy, stale))
    }

    pub fn lookup_record(
        &self,
        record: &ContinuityQualificationRecord,
    ) -> Result<ContinuityQualificationRecord, ContinuityFailure> {
        if record.responsibility_id.trim().is_empty() {
            return Err(invalid_request(ContinuityFailureCode::InvalidRequest));
        }
        admit_identity(&record.candidate_identity)?;
        if record.revoked
            || self
                .store
                .is_withdrawn(&record.responsibility_id, &record.candidate_identity)
        {
            return Ok(stale_view(record));
        }
        if let Some(bundle) = self
            .store
            .get(&record.responsibility_id, &record.candidate_identity)
        {
            let assessment = evaluate_bundle(bundle, &self.policy, false);
            return Ok(apply_assessment(record, &assessment));
        }
        if presented_prior_evaluation(record) {
            return Ok(stale_view(record));
        }
        Ok(unknown_view(record))
    }

    pub fn admit_automatic_advancement(
        &self,
        record: &ContinuityQualificationRecord,
    ) -> Result<(), ContinuityFailure> {
        let current = self.lookup_record(record)?;
        admit_automatic_from_record(&current)
    }

    pub fn admit_execution(
        &self,
        record: &ContinuityQualificationRecord,
        kind: RequestKind,
    ) -> Result<(), ContinuityFailure> {
        match kind {
            RequestKind::ExplicitlyRequested => super::eligibility::admit_requested_execution(),
            RequestKind::AutomaticAdvancement => self.admit_automatic_advancement(record),
        }
    }
}

impl QualificationPort for QualificationService {
    fn lookup(
        &self,
        record: &ContinuityQualificationRecord,
    ) -> Result<ContinuityQualificationRecord, ContinuityFailure> {
        self.lookup_record(record)
    }
}

fn presented_prior_evaluation(record: &ContinuityQualificationRecord) -> bool {
    record.observation_count > 0
        || matches!(
            record.result,
            ContinuityQualificationResult::Qualified
                | ContinuityQualificationResult::Unqualified
                | ContinuityQualificationResult::Stale
        )
}

fn stale_view(record: &ContinuityQualificationRecord) -> ContinuityQualificationRecord {
    ContinuityQualificationRecord {
        responsibility_id: record.responsibility_id.clone(),
        candidate_identity: record.candidate_identity.clone(),
        dataset_family_split: record.dataset_family_split,
        observation_count: record.observation_count,
        policy_revision: record.policy_revision.clone(),
        result: ContinuityQualificationResult::Stale,
        expires_at: None,
        revoked: record.revoked,
    }
}

fn unknown_view(record: &ContinuityQualificationRecord) -> ContinuityQualificationRecord {
    ContinuityQualificationRecord {
        responsibility_id: record.responsibility_id.clone(),
        candidate_identity: record.candidate_identity.clone(),
        dataset_family_split: ContinuityDatasetSplit::Heldout,
        observation_count: 0,
        policy_revision: record.policy_revision.clone(),
        result: ContinuityQualificationResult::Unknown,
        expires_at: None,
        revoked: false,
    }
}

pub fn query_record(
    responsibility_id: &str,
    identity: ContinuityCandidateIdentity,
) -> ContinuityQualificationRecord {
    ContinuityQualificationRecord {
        responsibility_id: responsibility_id.to_owned(),
        candidate_identity: identity,
        dataset_family_split: ContinuityDatasetSplit::Heldout,
        observation_count: 0,
        policy_revision: String::new(),
        result: ContinuityQualificationResult::Unknown,
        expires_at: None,
        revoked: false,
    }
}
