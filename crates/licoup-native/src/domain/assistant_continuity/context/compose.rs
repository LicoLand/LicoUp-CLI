//! Bounded orientation → scoped retrieval → refinement → dispatch recheck.
//!
//! Candidate ranking follows the same filter-then-score order used by
//! LlamaIndex exact-id lookup before similarity and by Elasticsearch
//! filter context before ranked query: exact SourceRef identity, then
//! granted parent refs, then current agreements, then entity overlap as
//! a clue only, then recency. Lexical clues never set speech act or Goal.

use std::collections::HashSet;
use std::sync::Arc;

use licoup_conversation::continuity::{
    CONTINUITY_MAX_ORIENTATION_ITEMS, CONTINUITY_MAX_REFS, ContextCompositionPort,
    ContinuityContextCompositionRequest, ContinuityContextManifest, ContinuityContextTransition,
    ContinuityFailure, ContinuityFailureCode, ContinuityFailureStage, ContinuityParentContextGrant,
    ContinuityParentGrantBasis, ContinuityParentGrantStatus, ContinuityReadPort,
    ContinuitySourceOwnerKind, ContinuitySourceRef, ContinuitySourceValidity,
    PENDING_OBLIGATION_PAGE_SIZE, admit_composition_request, admit_parent_context_grant,
    admit_source_ref,
};

use super::super::cognition::{
    AssemblySnapshot, ContextRecord, InformationClass, continuity_failure, source_key,
};
use super::store::FrozenContextStore;
use super::workspace::{ContinuityWorkspace, invocation_id, replay_key};

pub struct UnavailableContextCompositionService {
    workspace: Arc<ContinuityWorkspace>,
}

impl UnavailableContextCompositionService {
    pub fn empty() -> Self {
        Self {
            workspace: ContinuityWorkspace::empty(),
        }
    }

    pub fn from_workspace(workspace: Arc<ContinuityWorkspace>) -> Self {
        Self { workspace }
    }

    pub fn refine_authorized(
        &self,
        request: &ContinuityContextCompositionRequest,
        prior: &licoup_conversation::continuity::ContinuityInterpretationProposal,
    ) -> Result<ContinuityContextManifest, ContinuityFailure> {
        admit_composition_request(request)?;
        let store = &self.workspace.store;
        let grants = listed_grants(store, request)?;
        let mut retrieved = Vec::new();
        let mut records = Vec::new();
        let mut seen = HashSet::new();
        let all = store.records();
        for wanted in &prior.requested_reads {
            if !seen.insert(source_identity_key(wanted)) {
                continue;
            }
            if seen.len() > CONTINUITY_MAX_REFS {
                return Err(continuity_failure(
                    ContinuityFailureCode::InvalidRequest,
                    ContinuityFailureStage::ContinuityAdmission,
                ));
            }
            let Some(record) = authorized_record(store, request, wanted, &grants, &all)? else {
                return Err(continuity_failure(
                    ContinuityFailureCode::SourceUnavailable,
                    ContinuityFailureStage::ContinuityAdmission,
                ));
            };
            recheck_record(request, &record, &grants)?;
            retrieved.push(record.source.clone());
            records.push(record);
        }
        let prior_input_key = prior
            .envelope
            .source_event_refs
            .first()
            .map(source_identity_key);
        let prior_id = invocation_id(request, prior_input_key.as_deref(), false);
        let mut snapshot = self.workspace.merge_reads(&prior_id, retrieved, records)?;
        let current_input = snapshot
            .records
            .iter()
            .find(|record| record.is_current_input)
            .cloned();
        snapshot.records = apply_budget(snapshot.records, current_input.as_ref());
        snapshot.retrieved_reads.retain(|source| {
            snapshot
                .records
                .iter()
                .any(|record| source_identity_key(&record.source) == source_identity_key(source))
        });
        for record in &snapshot.records {
            recheck_record(request, record, &grants)?;
        }
        let refined_input_key = snapshot.input_refs.first().map(source_identity_key);
        snapshot.invocation_id = invocation_id(request, refined_input_key.as_deref(), true);
        self.workspace.remember_assembly(snapshot.clone());
        manifest_from(&snapshot, store, request)
    }

    pub fn recheck_dispatch(
        &self,
        manifest: &ContinuityContextManifest,
    ) -> Result<(), ContinuityFailure> {
        let snapshot = self.workspace.assembly_snapshot(&manifest.invocation_id)?;
        let store = &self.workspace.store;
        let basis = store.commit_basis(&snapshot.request.conversation_id)?;
        if basis.revision != snapshot.observed_revision {
            return Err(continuity_failure(
                ContinuityFailureCode::StaleRevision,
                ContinuityFailureStage::ContinuityAdmission,
            ));
        }
        if basis.designation_epoch != snapshot.designation_epoch {
            return Err(continuity_failure(
                ContinuityFailureCode::DesignationChanged,
                ContinuityFailureStage::ContinuityAdmission,
            ));
        }
        if manifest.recipient_binding != snapshot.request.recipient_membership_id {
            return Err(continuity_failure(
                ContinuityFailureCode::ScopeDenied,
                ContinuityFailureStage::ContinuityAdmission,
            ));
        }
        let current_acl_generation = store.acl_generation(&snapshot.request.conversation_id);
        if manifest.acl_generation != current_acl_generation {
            return Err(continuity_failure(
                ContinuityFailureCode::StaleRevision,
                ContinuityFailureStage::ContinuityAdmission,
            ));
        }
        let current_generation = store.recipient_revocation(
            &snapshot.request.conversation_id,
            &snapshot.request.recipient_membership_id,
        );
        if current_generation != snapshot.request.revocation_generation
            || current_generation != manifest.revocation_generation
        {
            return Err(continuity_failure(
                ContinuityFailureCode::StaleRevision,
                ContinuityFailureStage::ContinuityAdmission,
            ));
        }
        let expected_sources: HashSet<_> = snapshot
            .records
            .iter()
            .map(|record| source_identity_key(&record.source))
            .collect();
        let manifest_sources: HashSet<_> =
            manifest.sources.iter().map(source_identity_key).collect();
        if expected_sources.len() != snapshot.records.len()
            || manifest_sources.len() != manifest.sources.len()
            || expected_sources != manifest_sources
        {
            return Err(continuity_failure(
                ContinuityFailureCode::StaleRevision,
                ContinuityFailureStage::ContinuityAdmission,
            ));
        }
        let grants = listed_grants(store, &snapshot.request)?;
        let current_records = store.records();
        for record in &snapshot.records {
            let current = current_records
                .iter()
                .find(|item| source_reference_matches(item, &record.source))
                .ok_or_else(|| {
                    continuity_failure(
                        ContinuityFailureCode::SourceUnavailable,
                        ContinuityFailureStage::ContinuityAdmission,
                    )
                })?;
            let current = project_record(current, &record.source);
            if current.class == InformationClass::Agreement
                && !latest_agreement(&current_records, &current)
            {
                return Err(continuity_failure(
                    ContinuityFailureCode::StaleRevision,
                    ContinuityFailureStage::ContinuityAdmission,
                ));
            }
            recheck_record(&snapshot.request, &current, &grants)?;
        }
        Ok(())
    }
}

impl ContextCompositionPort for UnavailableContextCompositionService {
    fn compose_authorized(
        &self,
        request: &ContinuityContextCompositionRequest,
    ) -> Result<ContinuityContextManifest, ContinuityFailure> {
        admit_composition_request(request)?;
        let store = &self.workspace.store;
        let basis = store.commit_basis(&request.conversation_id)?;
        let current_generation =
            store.recipient_revocation(&request.conversation_id, &request.recipient_membership_id);
        if request.revocation_generation != current_generation {
            return Err(continuity_failure(
                ContinuityFailureCode::StaleRevision,
                ContinuityFailureStage::ContinuityAdmission,
            ));
        }
        let grants = listed_grants(store, request)?;
        let all = store.records();
        let current_input = all
            .iter()
            .filter(|record| {
                record.conversation_id == request.conversation_id && record.is_current_input
            })
            .max_by_key(|record| record.recency);
        let attention = current_input
            .and_then(|record| record.matter_id.clone())
            .or_else(|| store.attention_matter(&request.conversation_id));

        let mut selected = Vec::new();
        let mut reasons = Vec::new();
        push_unique(
            &mut selected,
            &mut reasons,
            current_input.cloned(),
            "current-input",
        );

        // Exact references before fuzzy hints.
        if let Some(input) = current_input {
            for reference in &input.explicit_refs {
                match authorized_record(store, request, reference, &grants, &all) {
                    Ok(Some(record)) => {
                        push_unique(&mut selected, &mut reasons, Some(record), "exact-ref");
                    }
                    Ok(None) => {}
                    Err(err) if err.code == ContinuityFailureCode::ScopeDenied => {}
                    Err(err) => return Err(err),
                }
            }
        }

        for record in granted_parent_records(request, &grants, &all)? {
            push_unique(&mut selected, &mut reasons, Some(record), "parent-grant");
        }

        for record in all.iter().filter(|record| {
            record.class == InformationClass::Agreement
                && record_in_scope(request, record, &attention)
                && candidate_scope_authorized(request, record)
        }) {
            if latest_agreement(&all, record) {
                push_unique(
                    &mut selected,
                    &mut reasons,
                    Some(record.clone()),
                    "current-agreement",
                );
            }
        }

        for record in all.iter().filter(|record| {
            record.class == InformationClass::Responsibility
                && record_in_scope(request, record, &attention)
                && candidate_scope_authorized(request, record)
        }) {
            push_unique(
                &mut selected,
                &mut reasons,
                Some(record.clone()),
                "responsibility",
            );
        }

        let input_entities = current_input
            .map(|record| record.entities.as_slice())
            .unwrap_or(&[]);
        let mut lexical = all
            .iter()
            .filter(|record| {
                !record.is_current_input
                    && record_in_scope(request, record, &attention)
                    && candidate_scope_authorized(request, record)
                    && !input_entities.is_empty()
                    && record
                        .entities
                        .iter()
                        .any(|entity| input_entities.contains(entity))
            })
            .cloned()
            .collect::<Vec<_>>();
        lexical.sort_by_key(|record| std::cmp::Reverse(record.recency));
        for record in lexical {
            if selected.len() >= CONTINUITY_MAX_ORIENTATION_ITEMS {
                break;
            }
            if record.class == InformationClass::Agreement && !latest_agreement(&all, &record) {
                continue;
            }
            push_unique(&mut selected, &mut reasons, Some(record), "lexical-hint");
        }

        let mut recent = all
            .iter()
            .filter(|record| {
                record.conversation_id == request.conversation_id
                    && record_in_scope(request, record, &attention)
                    && candidate_scope_authorized(request, record)
                    && !record.is_current_input
            })
            .cloned()
            .collect::<Vec<_>>();
        recent.sort_by_key(|record| std::cmp::Reverse(record.recency));
        for record in recent {
            if selected.len() >= CONTINUITY_MAX_ORIENTATION_ITEMS {
                break;
            }
            if record.class == InformationClass::Agreement && !latest_agreement(&all, &record) {
                continue;
            }
            push_unique(&mut selected, &mut reasons, Some(record), "recency");
        }

        selected = apply_budget(selected, current_input);
        if selected.len() > CONTINUITY_MAX_REFS {
            selected.truncate(CONTINUITY_MAX_REFS);
        }
        for record in &selected {
            recheck_record(request, record, &grants)?;
        }

        let input_identity = current_input.map(|record| source_identity_key(&record.source));
        let snapshot = AssemblySnapshot {
            invocation_id: invocation_id(request, input_identity.as_deref(), false),
            conversation_id: request.conversation_id.clone(),
            request: request.clone(),
            records: selected.clone(),
            input_refs: current_input
                .map(|record| vec![record.source.clone()])
                .unwrap_or_default(),
            retrieved_reads: Vec::new(),
            replay_key: replay_key(
                request,
                input_identity.as_deref(),
                current_input.map(|record| record.source.source_revision),
            ),
            observed_revision: basis.revision,
            designation_epoch: basis.designation_epoch,
        };
        self.workspace.remember_assembly(snapshot.clone());
        let mut manifest = manifest_from(&snapshot, store, request)?;
        manifest.selection_reason_codes = stable_reasons(&selected, &reasons);
        if store.prompt_cache_expired(&request.conversation_id)
            && manifest.context_transition == ContinuityContextTransition::Continue
        {
            manifest
                .selection_reason_codes
                .push("prompt-cache-expired".into());
        }
        Ok(manifest)
    }
}

fn listed_grants(
    store: &FrozenContextStore,
    request: &ContinuityContextCompositionRequest,
) -> Result<Vec<ContinuityParentContextGrant>, ContinuityFailure> {
    let mut grants = Vec::new();
    let mut after = request.after.clone();
    loop {
        let page = store.list_parent_grants(
            &request.conversation_id,
            &request.recipient_membership_id,
            after.as_deref(),
            PENDING_OBLIGATION_PAGE_SIZE as usize,
        )?;
        let page_len = page.len();
        after = page.last().map(|grant| grant.grant_id.clone());
        grants.extend(page);
        if page_len < PENDING_OBLIGATION_PAGE_SIZE as usize {
            return Ok(grants);
        }
    }
}

fn exact_grant_for_source<'a>(
    request: &ContinuityContextCompositionRequest,
    source_conversation_id: &str,
    requested: &ContinuitySourceRef,
    grants: &'a [ContinuityParentContextGrant],
) -> Option<&'a ContinuityParentContextGrant> {
    let basis = ContinuityParentGrantBasis {
        recipient_conversation_id: request.conversation_id.clone(),
        recipient_membership_id: request.recipient_membership_id.clone(),
        revocation_generation: request.revocation_generation,
    };
    grants.iter().find(|grant| {
        grant.source_conversation_id == source_conversation_id
            && admit_parent_context_grant(grant, requested, &basis).is_ok()
    })
}

fn authorized_record(
    store: &FrozenContextStore,
    request: &ContinuityContextCompositionRequest,
    reference: &ContinuitySourceRef,
    grants: &[licoup_conversation::continuity::ContinuityParentContextGrant],
    all: &[ContextRecord],
) -> Result<Option<ContextRecord>, ContinuityFailure> {
    scope_filter_before_read(store, request, reference, grants)?;
    if let Some(record) = all
        .iter()
        .find(|record| source_reference_matches(record, reference))
    {
        return Ok(Some(project_record(record, reference)));
    }
    Ok(None)
}

fn scope_filter_before_read(
    store: &FrozenContextStore,
    request: &ContinuityContextCompositionRequest,
    reference: &ContinuitySourceRef,
    grants: &[licoup_conversation::continuity::ContinuityParentContextGrant],
) -> Result<(), ContinuityFailure> {
    let Some(record) = store
        .records()
        .into_iter()
        .find(|item| source_reference_matches(item, reference))
    else {
        return Err(continuity_failure(
            ContinuityFailureCode::SourceUnavailable,
            ContinuityFailureStage::ContinuityAdmission,
        ));
    };
    let projected = project_record(&record, reference);
    if projected.source.validity == ContinuitySourceValidity::Revoked {
        return Err(continuity_failure(
            ContinuityFailureCode::SourceRevoked,
            ContinuityFailureStage::ContinuityAdmission,
        ));
    }
    if record.conversation_id == request.conversation_id {
        admit_source_ref(&request.authorized_scopes, &projected.source)?;
    }
    if !conversation_readable(request, &projected, grants) {
        return Err(continuity_failure(
            ContinuityFailureCode::ScopeDenied,
            ContinuityFailureStage::ContinuityAdmission,
        ));
    }
    Ok(())
}

fn conversation_readable(
    request: &ContinuityContextCompositionRequest,
    record: &ContextRecord,
    grants: &[licoup_conversation::continuity::ContinuityParentContextGrant],
) -> bool {
    record.conversation_id == request.conversation_id
        || exact_grant_for_source(request, &record.conversation_id, &record.source, grants)
            .is_some()
}

fn granted_parent_records(
    request: &ContinuityContextCompositionRequest,
    grants: &[licoup_conversation::continuity::ContinuityParentContextGrant],
    all: &[ContextRecord],
) -> Result<Vec<ContextRecord>, ContinuityFailure> {
    let mut out = Vec::new();
    let basis = ContinuityParentGrantBasis {
        recipient_conversation_id: request.conversation_id.clone(),
        recipient_membership_id: request.recipient_membership_id.clone(),
        revocation_generation: request.revocation_generation,
    };
    for grant in grants {
        if grant.status != ContinuityParentGrantStatus::Admitted {
            continue;
        }
        for allowed in &grant.source_refs {
            match admit_parent_context_grant(grant, allowed, &basis) {
                Ok(()) => {}
                Err(err)
                    if matches!(
                        err.code,
                        ContinuityFailureCode::StaleRevision | ContinuityFailureCode::ScopeDenied
                    ) =>
                {
                    continue;
                }
                Err(err) => return Err(err),
            }
            if let Some(record) = all.iter().find(|item| {
                item.conversation_id == grant.source_conversation_id
                    && source_identity_key(&item.source) == source_identity_key(allowed)
            }) {
                if record.class == InformationClass::Agreement && !latest_agreement(all, record) {
                    continue;
                }
                out.push(record.clone());
            }
        }
    }
    Ok(out)
}

fn record_in_scope(
    request: &ContinuityContextCompositionRequest,
    record: &ContextRecord,
    attention: &Option<String>,
) -> bool {
    if record.conversation_id != request.conversation_id {
        return false;
    }
    if record.is_current_input {
        return true;
    }
    match (attention.as_deref(), record.matter_id.as_deref()) {
        (_, None) => record.conversation_level,
        (Some(matter), Some(record_matter)) => matter == record_matter,
        (None, Some(_)) => false,
    }
}

fn candidate_scope_authorized(
    request: &ContinuityContextCompositionRequest,
    record: &ContextRecord,
) -> bool {
    // A local record uses the request scope list. An external record is
    // authorized by its admitted parent grant, which carries its own scope.
    record.conversation_id != request.conversation_id
        || admit_source_ref(&request.authorized_scopes, &record.source).is_ok()
}

fn source_identity_key(source: &ContinuitySourceRef) -> String {
    format!("{}:{:?}", source_key(source), source.visibility_scope)
}

fn source_reference_matches(record: &ContextRecord, requested: &ContinuitySourceRef) -> bool {
    let stored = &record.source;
    source_identity_key(stored) == source_identity_key(requested)
        || (requested.owner_kind == ContinuitySourceOwnerKind::Span
            && stored.opaque_id == requested.opaque_id
            && stored.part_id == requested.part_id
            && stored.source_revision == requested.source_revision
            && stored.digest == requested.digest
            && stored.visibility_scope == requested.visibility_scope
            && requested_span_is_contained(record, requested))
}

fn requested_span_is_contained(record: &ContextRecord, requested: &ContinuitySourceRef) -> bool {
    let Some(requested_span) = requested.span.as_ref() else {
        return false;
    };
    let (base_start, base_end) = record
        .source
        .span
        .as_ref()
        .map(|span| (span.start_byte, span.end_byte))
        .unwrap_or((0, record.text_bytes));
    requested_span.start_byte >= base_start
        && requested_span.end_byte <= base_end
        && requested_span.start_byte <= requested_span.end_byte
}

fn project_record(record: &ContextRecord, requested: &ContinuitySourceRef) -> ContextRecord {
    if source_identity_key(&record.source) == source_identity_key(requested) {
        return record.clone();
    }
    let mut projected = record.clone();
    projected.source = requested.clone();
    // The stored record decides provenance: a requested ref cannot project
    // away a revocation.
    if record.source.validity == ContinuitySourceValidity::Revoked {
        projected.source.validity = ContinuitySourceValidity::Revoked;
    }
    if let Some(span) = requested.span.as_ref() {
        projected.text_bytes = span.end_byte.saturating_sub(span.start_byte);
    }
    projected
}

fn latest_agreement(all: &[ContextRecord], record: &ContextRecord) -> bool {
    let Some(agreement) = &record.agreement else {
        return false;
    };
    !all.iter().any(|other| {
        other.agreement.as_ref().is_some_and(|candidate| {
            candidate.id == agreement.id
                && candidate.effective_revision > agreement.effective_revision
        })
    })
}

fn recheck_record(
    request: &ContinuityContextCompositionRequest,
    record: &ContextRecord,
    grants: &[licoup_conversation::continuity::ContinuityParentContextGrant],
) -> Result<(), ContinuityFailure> {
    if record.source.validity == ContinuitySourceValidity::Revoked {
        return Err(continuity_failure(
            ContinuityFailureCode::SourceRevoked,
            ContinuityFailureStage::ContinuityAdmission,
        ));
    }
    if record.conversation_id == request.conversation_id {
        return admit_source_ref(&request.authorized_scopes, &record.source);
    }
    // A parent grant is the external disclosure ACL. Its authorized scope is
    // checked by `exact_grant_for_source` against the current recipient basis.
    if exact_grant_for_source(request, &record.conversation_id, &record.source, grants).is_none() {
        return Err(continuity_failure(
            ContinuityFailureCode::ScopeDenied,
            ContinuityFailureStage::ContinuityAdmission,
        ));
    }
    Ok(())
}

fn apply_budget(
    selected: Vec<ContextRecord>,
    current_input: Option<&ContextRecord>,
) -> Vec<ContextRecord> {
    let within_budget = selected.len() <= CONTINUITY_MAX_ORIENTATION_ITEMS;
    let mut kept = Vec::new();
    if let Some(input) = current_input {
        kept.push(input.clone());
    }
    let mut mandatory = selected
        .iter()
        .filter(|record| {
            record.class == InformationClass::Agreement
                || record.class == InformationClass::Responsibility
                || matches!(
                    record.class,
                    InformationClass::ConversationFact | InformationClass::CallbackFact
                ) && record.conversation_level
        })
        .cloned()
        .collect::<Vec<_>>();
    mandatory.sort_by(|left, right| {
        class_rank(left)
            .cmp(&class_rank(right))
            .then_with(|| right.recency.cmp(&left.recency))
    });
    for record in mandatory {
        if kept.len() >= CONTINUITY_MAX_ORIENTATION_ITEMS {
            break;
        }
        if !kept
            .iter()
            .any(|have| source_identity_key(&have.source) == source_identity_key(&record.source))
        {
            kept.push(record);
        }
    }
    for record in selected {
        if kept.len() >= CONTINUITY_MAX_ORIENTATION_ITEMS {
            break;
        }
        if record.class == InformationClass::WorkingNote && !within_budget {
            continue;
        }
        if !kept
            .iter()
            .any(|have| source_identity_key(&have.source) == source_identity_key(&record.source))
        {
            kept.push(record);
        }
    }
    order_stable_prefix(kept)
}

fn order_stable_prefix(mut selected: Vec<ContextRecord>) -> Vec<ContextRecord> {
    selected.sort_by_key(|record| class_rank(record));
    selected
}

fn class_rank(record: &ContextRecord) -> u8 {
    if record.is_current_input {
        return 0;
    }
    match record.class {
        InformationClass::Agreement => 1,
        InformationClass::Responsibility => 2,
        InformationClass::CallbackFact => 3,
        InformationClass::ConversationFact => 4,
        InformationClass::KnowledgeReference => 5,
        InformationClass::WorkingNote => 6,
    }
}

fn push_unique(
    selected: &mut Vec<ContextRecord>,
    reasons: &mut Vec<(String, String)>,
    record: Option<ContextRecord>,
    reason: &str,
) {
    let Some(record) = record else {
        return;
    };
    let key = source_identity_key(&record.source);
    if selected
        .iter()
        .any(|have| source_identity_key(&have.source) == key)
    {
        return;
    }
    reasons.push((key, reason.into()));
    selected.push(record);
}

fn stable_reasons(selected: &[ContextRecord], reasons: &[(String, String)]) -> Vec<String> {
    let mut out = Vec::new();
    for record in selected {
        let key = source_identity_key(&record.source);
        if let Some((_, reason)) = reasons.iter().find(|(item, _)| item == &key) {
            if !out.contains(reason) {
                out.push(reason.clone());
            }
        }
    }
    out
}

fn manifest_from(
    snapshot: &AssemblySnapshot,
    store: &FrozenContextStore,
    request: &ContinuityContextCompositionRequest,
) -> Result<ContinuityContextManifest, ContinuityFailure> {
    let attention = snapshot
        .records
        .iter()
        .find(|record| record.is_current_input)
        .and_then(|record| record.matter_id.clone())
        .or_else(|| store.attention_matter(&request.conversation_id));
    let previous = store.attention_matter(&request.conversation_id);
    let unrelated = match (attention.as_deref(), previous.as_deref()) {
        (Some(left), Some(right)) => left != right,
        _ => attention.is_some() && previous.is_some(),
    };
    let binding_usable = attention
        .as_deref()
        .map(|matter| store.binding_usable(&request.conversation_id, matter))
        .unwrap_or(true);
    let transition = if store.deliberate_fork(&request.conversation_id) {
        ContinuityContextTransition::Fork
    } else if unrelated {
        ContinuityContextTransition::New
    } else if !binding_usable {
        ContinuityContextTransition::Rehydrate
    } else {
        ContinuityContextTransition::Continue
    };
    let agreement_revisions = snapshot
        .records
        .iter()
        .filter_map(|record| {
            record
                .agreement
                .as_ref()
                .map(|agreement| agreement.effective_revision)
        })
        .collect();
    let token_estimate = snapshot.records.iter().fold(0_u64, |total, record| {
        total.saturating_add(record.text_bytes.div_ceil(4).max(1))
    });
    Ok(ContinuityContextManifest {
        invocation_id: snapshot.invocation_id.clone(),
        sources: snapshot
            .records
            .iter()
            .map(|record| record.source.clone())
            .collect(),
        agreement_revisions,
        acl_generation: store.acl_generation(&request.conversation_id),
        revocation_generation: request.revocation_generation,
        recipient_binding: request.recipient_membership_id.clone(),
        token_estimate,
        selection_reason_codes: Vec::new(),
        context_transition: transition,
    })
}
