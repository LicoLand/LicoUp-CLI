//! Production cognition: one admitted Assistant turn through PersistentTurn.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use licoup_conversation::continuity::{
    ContinuityContextManifest, ContinuityFailure, ContinuityParentContextGrant,
    ContinuityParentGrantBasis, ContinuitySourceOwnerKind, ContinuitySourceRef,
    ContinuitySourceValidity, admit_utf8_span, find_admitted_parent_grant, source_is_revoked_now,
};
use licoup_conversation::{
    Conversation, ConversationStore, ImageAttachmentReference, MembershipStatus, PrincipalKind,
    ProfileIntent,
};
use serde_json::{Value, json};

use crate::domain::client_conversation::{LICOUP_GUIDE_SKILL_SOURCE, dispatch_attachments_param};
use crate::platform::runtime_adapters::{
    GeneratedInstructionDelivery, RuntimeAdapterError, compose_generated_instruction_delivery,
};

use super::interpret::{abstain_proposal, proposal_from_turn_output, unavailable_abstain};
use super::runtime::{
    CognitionInvoker, CognitionReply, CognitionRequest, proposal_has_business_effect,
};

pub(crate) type CompleteAdmittedTurn =
    dyn Fn(&Value) -> std::result::Result<Value, RuntimeAdapterError> + Send + Sync;

pub struct PersistentTurnCognition {
    store: ConversationStore,
    complete_turn: Arc<CompleteAdmittedTurn>,
    invocations: AtomicU64,
}

impl PersistentTurnCognition {
    pub fn new(store: ConversationStore, complete_turn: Arc<CompleteAdmittedTurn>) -> Self {
        Self {
            store,
            complete_turn,
            invocations: AtomicU64::new(0),
        }
    }
}

impl CognitionInvoker for PersistentTurnCognition {
    fn invoke(&self, request: &CognitionRequest) -> Result<CognitionReply, ContinuityFailure> {
        self.invocations.fetch_add(1, Ordering::SeqCst);
        if request.recipient_membership_id.trim().is_empty() {
            return Ok(unavailable_reply(&request.assembly));
        }
        let conversation = self.store.get(&request.conversation_id).map_err(|_| {
            super::types::continuity_failure(
                licoup_conversation::continuity::ContinuityFailureCode::SourceUnavailable,
                licoup_conversation::continuity::ContinuityFailureStage::ContinuityAdmission,
            )
        })?;
        if !recipient_is_current(&conversation, &request.recipient_membership_id) {
            return Ok(unavailable_reply(&request.assembly));
        }
        if membership_agent_id(&conversation, &request.recipient_membership_id).is_none() {
            return Ok(unavailable_reply(&request.assembly));
        }
        let user_text = request
            .review
            .as_ref()
            .map(|review| review.brief.clone())
            .filter(|text| !text.trim().is_empty())
            .unwrap_or_else(|| {
                self.store
                    .posted_event_text(&request.conversation_id, &request.event_id)
                    .unwrap_or_default()
            });
        let continuity_kind = request
            .review
            .as_ref()
            .map(|review| review.continuity_kind)
            .unwrap_or(match request.intent {
                super::runtime::CognitionIntent::WakeReevaluation => "wake-review",
                super::runtime::CognitionIntent::ChildWork => "child-work",
                super::runtime::CognitionIntent::UserPosted => "user-posted",
            });
        let params = compose_admitted_turn_params(&AdmittedTurnRequest {
            store: &self.store,
            conversation_id: &request.conversation_id,
            membership_id: &request.recipient_membership_id,
            text: &user_text,
            causation_id: (!request.event_id.trim().is_empty())
                .then_some(request.event_id.as_str()),
            dispatch_id: None,
            continuity_kind,
            goal_id: request
                .review
                .as_ref()
                .map(|review| review.goal_id.as_str()),
            goal_revision: request.review.as_ref().map(|review| review.goal_revision),
            review_policy: request
                .review
                .as_ref()
                .map(|review| review.review_policy.as_str()),
            parent_conversation_id: request
                .review
                .as_ref()
                .map(|review| review.parent_conversation_id.as_str()),
            assembly: Some(&request.assembly),
            orientation: Some(&request.orientation),
            include_licoup_guide: true,
        })?;
        match (self.complete_turn)(&params) {
            Ok(value) => {
                let output = turn_output(&value);
                let proposal = proposal_from_turn_output(&request.assembly, &output)?;
                let abstained = !proposal_has_business_effect(&proposal);
                Ok(CognitionReply {
                    proposal,
                    unavailable: false,
                    abstained,
                })
            }
            Err(_) => Ok(CognitionReply {
                proposal: abstain_proposal(&request.assembly),
                unavailable: true,
                abstained: true,
            }),
        }
    }

    fn invocation_count(&self) -> u64 {
        self.invocations.load(Ordering::SeqCst)
    }
}

fn recipient_is_current(conversation: &Conversation, membership_id: &str) -> bool {
    conversation.memberships.iter().any(|membership| {
        membership.id == membership_id
            && membership.status == MembershipStatus::Active
            && membership.principal.kind == PrincipalKind::Agent
    })
}

pub(crate) struct AdmittedTurnRequest<'a> {
    pub store: &'a ConversationStore,
    pub conversation_id: &'a str,
    pub membership_id: &'a str,
    pub text: &'a str,
    pub causation_id: Option<&'a str>,
    pub dispatch_id: Option<&'a str>,
    pub continuity_kind: &'a str,
    pub goal_id: Option<&'a str>,
    pub goal_revision: Option<i64>,
    pub review_policy: Option<&'a str>,
    pub parent_conversation_id: Option<&'a str>,
    pub assembly: Option<&'a super::types::AssemblySnapshot>,
    pub orientation: Option<&'a ContinuityContextManifest>,
    pub include_licoup_guide: bool,
}

pub(crate) fn compose_admitted_turn_params(
    request: &AdmittedTurnRequest<'_>,
) -> Result<Value, ContinuityFailure> {
    let conversation = request.store.get(request.conversation_id).map_err(|_| {
        super::types::continuity_failure(
            licoup_conversation::continuity::ContinuityFailureCode::SourceUnavailable,
            licoup_conversation::continuity::ContinuityFailureStage::ContinuityAdmission,
        )
    })?;
    let Some(agent_id) = membership_agent_id(&conversation, request.membership_id) else {
        return Err(super::types::continuity_failure(
            licoup_conversation::continuity::ContinuityFailureCode::SourceUnavailable,
            licoup_conversation::continuity::ContinuityFailureStage::ContinuityAdmission,
        ));
    };
    let profile = request
        .store
        .membership_profile(request.membership_id)
        .ok()
        .flatten();
    let native_role = profile.as_ref().and_then(|intent| {
        crate::domain::native_roles::role_from_skill_refs(&intent.skill_references)
    });
    let mut guidance = match (request.assembly, request.orientation) {
        (Some(assembly), Some(orientation)) => compose_continuity_guidance(
            request.store,
            assembly,
            orientation,
            request.include_licoup_guide,
            native_role
                .as_ref()
                .map(|role| role.instructions.as_str())
                .filter(|text| !text.trim().is_empty()),
        ),
        _ => String::new(),
    };
    if let Some(instructions) = profile
        .as_ref()
        .and_then(|intent| intent.preferred_instructions.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if !guidance.is_empty() {
            guidance.push_str("\n\n");
        }
        guidance.push_str(instructions);
    }
    let delivery = compose_generated_instruction_delivery(
        &agent_id,
        request.text,
        (!guidance.is_empty()).then_some(guidance.as_str()),
    )
    .unwrap_or(GeneratedInstructionDelivery {
        text: request.text.to_owned(),
        field: None,
        guidance: None,
    });
    let mut params = json!({
        "agentId": agent_id,
        "agent": agent_id,
        "text": delivery.text,
        "streamEvents": true,
        "conversationId": request.conversation_id,
        "membershipId": request.membership_id,
        "continuityKind": request.continuity_kind,
    });
    if let Some(dispatch_id) = request.dispatch_id.filter(|value| !value.trim().is_empty()) {
        params["dispatchId"] = json!(dispatch_id);
    }
    if let Some(causation) = request
        .causation_id
        .filter(|value| !value.trim().is_empty())
    {
        params["causationId"] = json!(causation);
    }
    if let Some(attachments) = admitted_image_attachments(request) {
        params["attachments"] = attachments;
    }
    if let Some(goal_id) = request.goal_id.filter(|value| !value.trim().is_empty()) {
        params["goalId"] = json!(goal_id);
    }
    if let Some(revision) = request.goal_revision {
        params["goalRevision"] = json!(revision);
    }
    if let Some(policy) = request
        .review_policy
        .filter(|value| !value.trim().is_empty())
    {
        params["reviewPolicy"] = json!(policy);
    }
    if let Some(parent) = request
        .parent_conversation_id
        .filter(|value| !value.trim().is_empty())
    {
        params["parentConversationId"] = json!(parent);
    }
    if let (Some(field), Some(guidance)) = (delivery.field, delivery.guidance) {
        params[field] = json!(guidance);
    }
    if let Some(role) = native_role.as_ref() {
        params["runtimeAgent"] = json!(role.slug);
    }
    apply_admitted_runtime_fields(
        &mut params,
        request.store,
        request.conversation_id,
        request.membership_id,
        profile.as_ref(),
        None,
    );
    Ok(params)
}

pub(crate) fn apply_admitted_runtime_fields(
    params: &mut Value,
    store: &ConversationStore,
    conversation_id: &str,
    membership_id: &str,
    profile: Option<&ProfileIntent>,
    attachments: Option<Value>,
) {
    if let Some(model) = profile
        .and_then(|intent| intent.preferred_model.as_deref())
        .filter(|value| !value.trim().is_empty())
    {
        params["model"] = json!(model);
    }
    if let Some(reasoning) = profile
        .and_then(|intent| intent.preferred_reasoning_effort.as_deref())
        .filter(|value| !value.trim().is_empty())
    {
        params["reasoningEffort"] = json!(reasoning);
    }
    if let Some(permission_mode) = profile
        .and_then(|intent| intent.preferred_permission_mode.as_deref())
        .filter(|value| !value.trim().is_empty())
    {
        params["permissionMode"] = json!(permission_mode);
    }
    if let Some(context_window) = profile.and_then(|intent| intent.preferred_context_window) {
        params["contextWindow"] = json!(context_window);
    }
    if let Ok(Some(binding)) = store.private_runtime_binding(conversation_id, membership_id) {
        if !binding.runtime_session_id.trim().is_empty() {
            params["sessionId"] = json!(binding.runtime_session_id);
        }
        if let Some(path) = binding
            .runtime_conversation_path
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            params["sourcePath"] = json!(path);
        }
        if let Some(cwd) = binding
            .working_directory
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            params["workingDirectory"] = json!(cwd);
        }
    }
    if params
        .get("workingDirectory")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_none()
    {
        if let Some(environment) = profile
            .and_then(|intent| intent.preferred_environment.as_deref())
            .filter(|value| !value.trim().is_empty())
        {
            params["workingDirectory"] = json!(environment);
        }
    }
    if let Some(capabilities) = profile
        .map(|intent| &intent.required_capabilities)
        .filter(|values| !values.is_empty())
    {
        params["requiredCapabilities"] = json!(capabilities);
    }
    if let Some(attachments) = attachments {
        params["attachments"] = attachments;
    }
}

fn admitted_image_attachments(request: &AdmittedTurnRequest<'_>) -> Option<Value> {
    let mut references = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    if let Some(assembly) = request.assembly {
        for record in &assembly.records {
            if record.source.owner_kind != ContinuitySourceOwnerKind::Event
                && record.source.owner_kind != ContinuitySourceOwnerKind::Part
            {
                continue;
            }
            append_admitted_source_attachments(
                request,
                &record.conversation_id,
                &record.source,
                &mut references,
                &mut seen,
            );
        }
    }
    if let Some(causation) = request
        .causation_id
        .filter(|value| !value.trim().is_empty())
    {
        if let Some(source) =
            event_source_for_conversation(request, request.conversation_id, causation)
        {
            append_admitted_source_attachments(
                request,
                request.conversation_id,
                &source,
                &mut references,
                &mut seen,
            );
        }
        if let Some(parent) = request
            .parent_conversation_id
            .filter(|value| !value.trim().is_empty())
        {
            if let Some(source) = event_source_for_conversation(request, parent, causation) {
                append_admitted_source_attachments(
                    request,
                    parent,
                    &source,
                    &mut references,
                    &mut seen,
                );
            }
        }
    }
    if references.is_empty() {
        return None;
    }
    Some(dispatch_attachments_param(&references))
}

fn event_source_for_conversation(
    request: &AdmittedTurnRequest<'_>,
    conversation_id: &str,
    event_id: &str,
) -> Option<ContinuitySourceRef> {
    if let Some(assembly) = request.assembly {
        if let Some(record) = assembly.records.iter().find(|record| {
            record.conversation_id == conversation_id
                && record.source.opaque_id == event_id
                && record.source.owner_kind == ContinuitySourceOwnerKind::Event
        }) {
            return Some(record.source.clone());
        }
    }
    let event = request
        .store
        .event(conversation_id, event_id)
        .ok()
        .flatten()?;
    Some(ContinuitySourceRef {
        owner_kind: ContinuitySourceOwnerKind::Event,
        opaque_id: event_id.to_owned(),
        part_id: None,
        span: None,
        source_revision: event.sequence,
        digest: format!("event:{event_id}"),
        visibility_scope: licoup_conversation::continuity::ContinuityVisibilityScope::Conversation,
        validity: ContinuitySourceValidity::Current,
    })
}

fn append_admitted_source_attachments(
    request: &AdmittedTurnRequest<'_>,
    conversation_id: &str,
    source: &ContinuitySourceRef,
    references: &mut Vec<ImageAttachmentReference>,
    seen: &mut std::collections::BTreeSet<String>,
) {
    if !source_attachments_are_admitted(request, conversation_id, source) {
        return;
    }
    if let Some(part_id) = source.part_id.as_deref().filter(|value| !value.is_empty()) {
        let Ok(Some(reference)) =
            request
                .store
                .image_attachment_for_part(conversation_id, &source.opaque_id, part_id)
        else {
            return;
        };
        if seen.insert(reference.part_id.clone()) {
            references.push(reference);
        }
        return;
    }
    if source.owner_kind != ContinuitySourceOwnerKind::Event {
        return;
    }
    let Ok(found) = request
        .store
        .image_attachments_for_event(conversation_id, &source.opaque_id)
    else {
        return;
    };
    for reference in found {
        if seen.insert(reference.part_id.clone()) {
            references.push(reference);
        }
    }
}

fn source_attachments_are_admitted(
    request: &AdmittedTurnRequest<'_>,
    conversation_id: &str,
    source: &ContinuitySourceRef,
) -> bool {
    if source.validity == ContinuitySourceValidity::Revoked {
        return false;
    }
    if source_is_revoked_now(request.store, conversation_id, source).unwrap_or(true) {
        return false;
    }
    if matches!(
        source.owner_kind,
        ContinuitySourceOwnerKind::Event
            | ContinuitySourceOwnerKind::Part
            | ContinuitySourceOwnerKind::Span
    ) {
        let Some(observed) = request
            .store
            .event_sequence(conversation_id, &source.opaque_id)
            .ok()
            .flatten()
        else {
            return false;
        };
        if observed != source.source_revision {
            return false;
        }
    }
    if conversation_id == request.conversation_id {
        return true;
    }
    let Some(assembly) = request.assembly else {
        return false;
    };
    let Some(record) = assembly.records.iter().find(|record| {
        record.conversation_id == conversation_id
            && super::types::source_key(&record.source) == super::types::source_key(source)
    }) else {
        return false;
    };
    if !disclosable_selected_record(assembly, record) {
        return false;
    }
    exact_admitted_parent_grant(request.store, assembly, conversation_id, &record.source).is_some()
}

fn exact_admitted_parent_grant(
    store: &ConversationStore,
    assembly: &super::types::AssemblySnapshot,
    source_conversation_id: &str,
    requested: &ContinuitySourceRef,
) -> Option<ContinuityParentContextGrant> {
    let basis = ContinuityParentGrantBasis {
        recipient_conversation_id: assembly.conversation_id.clone(),
        recipient_membership_id: assembly.request.recipient_membership_id.clone(),
        revocation_generation: assembly.request.revocation_generation,
    };
    find_admitted_parent_grant(
        store,
        &assembly.conversation_id,
        &assembly.request.recipient_membership_id,
        source_conversation_id,
        requested,
        &basis,
    )
    .ok()
    .flatten()
}

fn membership_agent_id(conversation: &Conversation, membership_id: &str) -> Option<String> {
    conversation
        .memberships
        .iter()
        .find(|membership| membership.id == membership_id)
        .and_then(|membership| membership.principal.agent_id.clone())
        .filter(|agent_id| !agent_id.trim().is_empty())
}

fn turn_output(value: &Value) -> String {
    value
        .get("output")
        .and_then(Value::as_str)
        .or_else(|| value.get("text").and_then(Value::as_str))
        .or_else(|| value.get("message").and_then(Value::as_str))
        .map(str::to_owned)
        .unwrap_or_else(|| value.to_string())
}

fn unavailable_reply(assembly: &super::types::AssemblySnapshot) -> CognitionReply {
    CognitionReply {
        proposal: unavailable_abstain(assembly),
        unavailable: true,
        abstained: true,
    }
}

pub(crate) fn compose_continuity_guidance(
    store: &ConversationStore,
    assembly: &super::types::AssemblySnapshot,
    orientation: &ContinuityContextManifest,
    include_licoup_guide: bool,
    native_role_instructions: Option<&str>,
) -> String {
    let mut guidance = String::new();
    if include_licoup_guide {
        guidance.push_str(LICOUP_GUIDE_SKILL_SOURCE);
    }
    if !orientation.selection_reason_codes.is_empty() {
        let mut reasons = String::from("Selection reasons:");
        for code in &orientation.selection_reason_codes {
            reasons.push('\n');
            reasons.push_str(code);
        }
        push_block(&mut guidance, &reasons);
    }
    let contents = authorized_source_contents(store, assembly);
    if !contents.is_empty() {
        push_block(&mut guidance, &contents);
    }
    if let Some(role) = native_role_instructions.filter(|text| !text.trim().is_empty()) {
        push_block(&mut guidance, role);
    }
    guidance
}

fn push_block(guidance: &mut String, block: &str) {
    if block.trim().is_empty() {
        return;
    }
    if !guidance.is_empty() {
        guidance.push_str("\n\n");
    }
    guidance.push_str(block);
}

pub(crate) fn collect_admitted_granted_facts(
    store: &ConversationStore,
    assembly: &super::types::AssemblySnapshot,
) -> Vec<(String, String)> {
    let mut facts = Vec::new();
    for record in &assembly.records {
        if record.conversation_id == assembly.conversation_id {
            continue;
        }
        if !disclosable_selected_record(assembly, record) {
            continue;
        }
        let Some(text) = resolve_authorized_text(store, assembly, record) else {
            continue;
        };
        if text.trim().is_empty() {
            continue;
        }
        facts.push((record.source.opaque_id.clone(), text));
    }
    facts
}

fn authorized_source_contents(
    store: &ConversationStore,
    assembly: &super::types::AssemblySnapshot,
) -> String {
    let mut blocks = Vec::new();
    for record in &assembly.records {
        if !disclosable_selected_record(assembly, record) {
            continue;
        }
        let Some(text) = resolve_authorized_text(store, assembly, record) else {
            continue;
        };
        if text.is_empty() {
            continue;
        }
        let Ok(source_json) = serde_json::to_string(&record.source) else {
            continue;
        };
        blocks.push(format!("source-ref {source_json}\n{text}"));
    }
    if blocks.is_empty() {
        return String::new();
    }
    format!(
        "Authorized selected sources (current, grant-checked):\n\n{}",
        blocks.join("\n\n")
    )
}

fn disclosable_selected_record(
    assembly: &super::types::AssemblySnapshot,
    record: &super::types::ContextRecord,
) -> bool {
    if record.is_current_input || record.class == super::types::InformationClass::Agreement {
        return true;
    }
    if record.conversation_id != assembly.conversation_id {
        return true;
    }
    assembly
        .retrieved_reads
        .iter()
        .any(|source| super::types::source_key(source) == super::types::source_key(&record.source))
}

fn resolve_authorized_text(
    store: &ConversationStore,
    assembly: &super::types::AssemblySnapshot,
    record: &super::types::ContextRecord,
) -> Option<String> {
    if record.source.validity == ContinuitySourceValidity::Revoked {
        return None;
    }
    if source_is_revoked_now(store, &record.conversation_id, &record.source).unwrap_or(true) {
        return None;
    }
    if record.conversation_id != assembly.conversation_id {
        exact_admitted_parent_grant(store, assembly, &record.conversation_id, &record.source)?;
    }
    if matches!(
        record.source.owner_kind,
        ContinuitySourceOwnerKind::Event
            | ContinuitySourceOwnerKind::Part
            | ContinuitySourceOwnerKind::Span
    ) {
        let observed = store
            .event_sequence(&record.conversation_id, &record.source.opaque_id)
            .ok()
            .flatten()?;
        if observed != record.source.source_revision {
            return None;
        }
    }
    let source = record
        .agreement
        .as_ref()
        .map(|agreement| &agreement.statement_ref)
        .unwrap_or(&record.source);
    let text = if let Some(part_id) = source
        .part_id
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        store
            .posted_event_part_text(&record.conversation_id, &source.opaque_id, part_id)
            .ok()?
    } else {
        store
            .posted_event_text(&record.conversation_id, &source.opaque_id)
            .ok()?
    };
    apply_source_span(&text, source.span.as_ref())
}

fn apply_source_span(
    text: &str,
    span: Option<&licoup_conversation::continuity::ContinuityUtf8ByteSpan>,
) -> Option<String> {
    let Some(span) = span else {
        return Some(text.to_owned());
    };
    admit_utf8_span(text, span.start_byte, span.end_byte).ok()?;
    let start = usize::try_from(span.start_byte).ok()?;
    let end = usize::try_from(span.end_byte).ok()?;
    Some(text.get(start..end)?.to_owned())
}
