//! Frozen in-memory store substitute. No dependence on other M1 stores.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use licoup_conversation::continuity::{
    ContinuityAgreement, ContinuityCommitBasis, ContinuityFailure, ContinuityFailureCode,
    ContinuityFailureStage, ContinuityMatter, ContinuityParentContextGrant,
    ContinuityParentGrantStatus, ContinuityReadPort, ContinuityTaskConversationRelation,
    ContinuityUtf8ByteSpan, admit_page_limit,
};

use super::super::cognition::{ContextRecord, continuity_failure};

#[derive(Default)]
struct StoreInner {
    bases: HashMap<String, ContinuityCommitBasis>,
    matters: HashMap<String, Vec<ContinuityMatter>>,
    relations: HashMap<String, ContinuityTaskConversationRelation>,
    child_relations: HashMap<String, Vec<ContinuityTaskConversationRelation>>,
    grants: Vec<ContinuityParentContextGrant>,
    records: Vec<ContextRecord>,
    attention: HashMap<String, String>,
    prompt_cache_expired: HashMap<String, bool>,
    binding_usable: HashMap<String, bool>,
    deliberate_fork: HashMap<String, bool>,
    acl_generation: HashMap<String, i64>,
    recipient_revocation: HashMap<String, i64>,
}

#[derive(Clone, Default)]
pub struct FrozenContextStore {
    inner: Arc<Mutex<StoreInner>>,
}

fn lock(inner: &Mutex<StoreInner>) -> MutexGuard<'_, StoreInner> {
    inner
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl FrozenContextStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_commit_basis(&self, basis: ContinuityCommitBasis) {
        lock(&self.inner)
            .bases
            .insert(basis.conversation_id.clone(), basis);
    }

    pub fn insert_matter(&self, matter: ContinuityMatter) {
        lock(&self.inner)
            .matters
            .entry(matter.conversation_id.clone())
            .or_default()
            .push(matter);
    }

    pub fn insert_relation(&self, relation: ContinuityTaskConversationRelation) {
        let mut inner = lock(&self.inner);
        inner
            .child_relations
            .entry(relation.parent_conversation_id.clone())
            .or_default()
            .push(relation.clone());
        inner.relations.insert(relation.goal_id.clone(), relation);
    }

    pub fn insert_grant(&self, grant: ContinuityParentContextGrant) {
        let mut inner = lock(&self.inner);
        let key = recipient_key(
            &grant.recipient_conversation_id,
            &grant.recipient_membership_id,
        );
        inner
            .recipient_revocation
            .entry(key)
            .and_modify(|generation| *generation = (*generation).max(grant.revocation_generation))
            .or_insert(grant.revocation_generation);
        inner.grants.push(grant);
    }

    pub fn insert_record(&self, record: ContextRecord) {
        let mut inner = lock(&self.inner);
        if record.is_current_input {
            for existing in &mut inner.records {
                if existing.conversation_id == record.conversation_id {
                    existing.is_current_input = false;
                }
            }
        }
        inner.records.push(record);
    }

    pub fn set_attention(&self, conversation_id: &str, matter_id: &str) {
        lock(&self.inner)
            .attention
            .insert(conversation_id.into(), matter_id.into());
    }

    pub fn set_prompt_cache_expired(&self, conversation_id: &str, expired: bool) {
        lock(&self.inner)
            .prompt_cache_expired
            .insert(conversation_id.into(), expired);
    }

    pub fn set_binding_usable(&self, conversation_id: &str, matter_id: &str, usable: bool) {
        lock(&self.inner)
            .binding_usable
            .insert(format!("{conversation_id}:{matter_id}"), usable);
    }

    pub fn set_deliberate_fork(&self, conversation_id: &str, fork: bool) {
        lock(&self.inner)
            .deliberate_fork
            .insert(conversation_id.into(), fork);
    }

    pub fn set_acl_generation(&self, conversation_id: &str, generation: i64) {
        lock(&self.inner)
            .acl_generation
            .insert(conversation_id.into(), generation);
    }

    pub fn set_recipient_revocation(
        &self,
        conversation_id: &str,
        membership_id: &str,
        generation: i64,
    ) {
        lock(&self.inner)
            .recipient_revocation
            .insert(recipient_key(conversation_id, membership_id), generation);
    }

    pub fn revoke_grant(&self, grant_id: &str) {
        for grant in &mut lock(&self.inner).grants {
            if grant.grant_id == grant_id {
                grant.status = ContinuityParentGrantStatus::Revoked;
                for source in &mut grant.source_refs {
                    source.validity =
                        licoup_conversation::continuity::ContinuitySourceValidity::Revoked;
                }
            }
        }
    }

    pub fn bump_grant_generation(&self, grant_id: &str, generation: i64) {
        let mut inner = lock(&self.inner);
        let mut recipient = None;
        for grant in &mut inner.grants {
            if grant.grant_id == grant_id {
                grant.revocation_generation = generation;
                recipient = Some(recipient_key(
                    &grant.recipient_conversation_id,
                    &grant.recipient_membership_id,
                ));
            }
        }
        if let Some(key) = recipient {
            inner.recipient_revocation.insert(key, generation);
        }
    }

    pub fn shrink_grant_span(&self, grant_id: &str, span: ContinuityUtf8ByteSpan) {
        for grant in &mut lock(&self.inner).grants {
            if grant.grant_id == grant_id {
                for source in &mut grant.source_refs {
                    source.span = Some(span.clone());
                }
            }
        }
    }

    pub fn replace_agreement(&self, agreement_id: &str, next: ContinuityAgreement) {
        for record in &mut lock(&self.inner).records {
            if record
                .agreement
                .as_ref()
                .is_some_and(|current| current.id == agreement_id)
            {
                if let Some(previous) = record.agreement.as_mut() {
                    previous.valid_until = Some(next.valid_from);
                }
                record.source = next.statement_ref.clone();
                record.agreement = Some(next.clone());
            }
        }
    }

    pub fn records(&self) -> Vec<ContextRecord> {
        lock(&self.inner).records.clone()
    }

    pub fn attention_matter(&self, conversation_id: &str) -> Option<String> {
        lock(&self.inner).attention.get(conversation_id).cloned()
    }

    pub fn prompt_cache_expired(&self, conversation_id: &str) -> bool {
        lock(&self.inner)
            .prompt_cache_expired
            .get(conversation_id)
            .copied()
            .unwrap_or(false)
    }

    pub fn binding_usable(&self, conversation_id: &str, matter_id: &str) -> bool {
        lock(&self.inner)
            .binding_usable
            .get(&format!("{conversation_id}:{matter_id}"))
            .copied()
            .unwrap_or(true)
    }

    pub fn deliberate_fork(&self, conversation_id: &str) -> bool {
        lock(&self.inner)
            .deliberate_fork
            .get(conversation_id)
            .copied()
            .unwrap_or(false)
    }

    pub fn acl_generation(&self, conversation_id: &str) -> i64 {
        lock(&self.inner)
            .acl_generation
            .get(conversation_id)
            .copied()
            .unwrap_or(1)
    }

    pub fn recipient_revocation(&self, conversation_id: &str, membership_id: &str) -> i64 {
        lock(&self.inner)
            .recipient_revocation
            .get(&recipient_key(conversation_id, membership_id))
            .copied()
            .unwrap_or(0)
    }
}

fn recipient_key(conversation_id: &str, membership_id: &str) -> String {
    format!("{conversation_id}:{membership_id}")
}

fn page<'a, T>(items: &'a [T], after: Option<&str>, limit: usize, id: impl Fn(&T) -> &str) -> Vec<T>
where
    T: Clone,
{
    let skipped = match after {
        Some(cursor) => items
            .iter()
            .position(|item| id(item) == cursor)
            .map(|index| index + 1)
            .unwrap_or(0),
        None => 0,
    };
    items.iter().skip(skipped).take(limit).cloned().collect()
}

impl ContinuityReadPort for FrozenContextStore {
    fn commit_basis(
        &self,
        conversation_id: &str,
    ) -> Result<ContinuityCommitBasis, ContinuityFailure> {
        lock(&self.inner)
            .bases
            .get(conversation_id)
            .cloned()
            .ok_or_else(|| {
                continuity_failure(
                    ContinuityFailureCode::SourceUnavailable,
                    ContinuityFailureStage::ContinuityAdmission,
                )
            })
    }

    fn list_matters(
        &self,
        conversation_id: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<ContinuityMatter>, ContinuityFailure> {
        let inner = lock(&self.inner);
        let items = inner
            .matters
            .get(conversation_id)
            .cloned()
            .unwrap_or_default();
        Ok(page(&items, after, limit, |matter| matter.id.as_str()))
    }

    fn relation_for_goal(
        &self,
        goal_id: &str,
    ) -> Result<ContinuityTaskConversationRelation, ContinuityFailure> {
        lock(&self.inner)
            .relations
            .get(goal_id)
            .cloned()
            .ok_or_else(|| {
                continuity_failure(
                    ContinuityFailureCode::SourceUnavailable,
                    ContinuityFailureStage::ContinuityAdmission,
                )
            })
    }

    fn list_child_relations(
        &self,
        parent_conversation_id: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<ContinuityTaskConversationRelation>, ContinuityFailure> {
        let inner = lock(&self.inner);
        let items = inner
            .child_relations
            .get(parent_conversation_id)
            .cloned()
            .unwrap_or_default();
        Ok(page(&items, after, limit, |relation| {
            relation.goal_id.as_str()
        }))
    }

    fn list_parent_grants(
        &self,
        recipient_conversation_id: &str,
        recipient_membership_id: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<ContinuityParentContextGrant>, ContinuityFailure> {
        let limit = admit_page_limit(limit as u64)?;
        let inner = lock(&self.inner);
        let items: Vec<ContinuityParentContextGrant> = inner
            .grants
            .iter()
            .filter(|grant| {
                grant.recipient_conversation_id == recipient_conversation_id
                    && grant.recipient_membership_id == recipient_membership_id
            })
            .cloned()
            .collect();
        Ok(page(&items, after, limit, |grant| grant.grant_id.as_str()))
    }
}
