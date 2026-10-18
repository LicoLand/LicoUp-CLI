//! Canonical client-owned Conversation records.

use serde::{Deserialize, Serialize};

use crate::store::NativeSessionReference;

pub use crate::state_machine::TurnState;
pub use crate::state_machine::conversation_dispatch::State as DispatchState;
pub use crate::state_machine::conversation_dispatch_delivery::State as DispatchDeliveryState;
pub use crate::state_machine::conversation_membership::State as MembershipStatus;
pub use crate::state_machine::conversation_subagent_claim::State as SubagentDispatchClaimState;

pub const CONVERSATION_SCHEMA_VERSION: &str = "lico.conversation.v1";
pub const DEFAULT_LOCAL_AGENT_GROUP_ID: &str = "lico-group-default";
pub const DEFAULT_LOCAL_AGENT_GROUP_TITLE: &str = "Local";
pub const MAX_PROFILE_CAPABILITIES: usize = 32;
pub const MAX_PROFILE_SKILLS: usize = 32;
pub const MAX_PROFILE_FIELD_BYTES: usize = 128;
pub const MAX_PROFILE_INSTRUCTIONS_BYTES: usize = 8192;
pub const MAX_PROFILE_CONTEXT_WINDOW_TOKENS: u64 = 8_000_000;
pub const LICOUP_GUIDE_SKILL_ID: &str = "licoup-guide";
const DIRECT_ASSISTANT_GUIDANCE: &str = "Respond directly to the user's request. Use tools only when the current request requires them. Do not start, resume, or invent unrelated work. Finish the requested response and return it.";

pub(crate) fn direct_assistant_guidance() -> &'static str {
    DIRECT_ASSISTANT_GUIDANCE
}

pub const PERSISTENT_TRANSPORT_REQUIRED: &str = "persistent_conversation_transport_required";

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PrincipalKind {
    Human,
    Agent,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum MembershipAccess {
    Owner,
    Member,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Principal {
    pub id: String,
    pub kind: PrincipalKind,
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    pub created_at_unix_ms: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Conversation {
    pub id: String,
    pub title: String,
    pub archived: bool,
    pub pinned: bool,
    pub is_group: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strategy_revision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assistant_membership_id: Option<String>,
    pub revision: i64,
    pub created_at_unix_ms: i64,
    pub updated_at_unix_ms: i64,
    pub memberships: Vec<Membership>,
    pub event_count: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationSummary {
    pub id: String,
    pub title: String,
    pub archived: bool,
    pub pinned: bool,
    pub is_group: bool,
    pub revision: i64,
    pub updated_at_unix_ms: i64,
    pub membership_count: i64,
    pub event_count: i64,
    /// Active Agent members' agent ids in join order; the client's group
    /// mosaic avatar reads the first few cells from it.
    #[serde(default)]
    pub member_agent_ids: Vec<String>,
    /// Whether the conversation has an active human owner membership — the
    /// local user's cell in the mosaic.
    #[serde(default)]
    pub has_local_owner: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationClearReport {
    pub conversation_id: String,
    pub archived_child_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assistant_membership_id: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationArchiveReport {
    pub conversation_id: String,
    pub archived_child_ids: Vec<String>,
    #[serde(default)]
    pub archived_native_sessions: Vec<NativeSessionReference>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub successor_conversation_id: Option<String>,
    /// True when the reserved default local group was reset in place instead
    /// of archived: the same Conversation stays active and pinned with one
    /// `conversation-reset` notice, and no successor is created.
    #[serde(default)]
    pub reset_in_place: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Membership {
    pub id: String,
    pub conversation_id: String,
    pub principal: Principal,
    pub access: MembershipAccess,
    pub status: MembershipStatus,
    pub joined_at_unix_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left_at_unix_ms: Option<i64>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProfileResponsibility {
    Assistant,
    #[default]
    Member,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileIntent {
    pub revision: i64,
    #[serde(default)]
    pub required_capabilities: Vec<String>,
    #[serde(default)]
    pub preferred_capabilities: Vec<String>,
    #[serde(default)]
    pub skill_references: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_reasoning_effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_environment: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_permission_mode: Option<String>,
    /// Free-form persona guidance delivered with the turn through the
    /// adapter's generated-instruction channel; never stored as Event text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_instructions: Option<String>,
    /// Requested context-window size in tokens, admitted into dispatch
    /// params; consumption is adapter-dependent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_context_window: Option<u64>,
    #[serde(default)]
    pub responsibility: ProfileResponsibility,
    pub updated_at_unix_ms: i64,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileIntentUpdate {
    #[serde(default)]
    pub required_capabilities: Vec<String>,
    #[serde(default)]
    pub preferred_capabilities: Vec<String>,
    #[serde(default)]
    pub skill_references: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_reasoning_effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_environment: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_permission_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_instructions: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_context_window: Option<u64>,
}

impl ProfileIntentUpdate {
    pub fn validate(&self) -> Result<(), String> {
        for (field, values, maximum) in [
            (
                "required_capabilities",
                &self.required_capabilities,
                MAX_PROFILE_CAPABILITIES,
            ),
            (
                "preferred_capabilities",
                &self.preferred_capabilities,
                MAX_PROFILE_CAPABILITIES,
            ),
            (
                "skill_references",
                &self.skill_references,
                MAX_PROFILE_SKILLS,
            ),
        ] {
            if values.len() > maximum {
                return Err(format!("profile_{field}_limit"));
            }
            if values
                .iter()
                .any(|value| value.is_empty() || value.len() > MAX_PROFILE_FIELD_BYTES)
            {
                return Err(format!("profile_{field}_invalid"));
            }
        }
        for (field, value) in [
            ("preferred_model", self.preferred_model.as_deref()),
            (
                "preferred_reasoning_effort",
                self.preferred_reasoning_effort.as_deref(),
            ),
            (
                "preferred_environment",
                self.preferred_environment.as_deref(),
            ),
            (
                "preferred_permission_mode",
                self.preferred_permission_mode.as_deref(),
            ),
        ] {
            if value.is_some_and(|value| value.is_empty() || value.len() > MAX_PROFILE_FIELD_BYTES)
            {
                return Err(format!("profile_{field}_invalid"));
            }
        }
        if self
            .preferred_instructions
            .as_deref()
            .is_some_and(|value| value.is_empty() || value.len() > MAX_PROFILE_INSTRUCTIONS_BYTES)
        {
            return Err("profile_preferred_instructions_invalid".to_owned());
        }
        if self
            .preferred_context_window
            .is_some_and(|value| value == 0 || value > MAX_PROFILE_CONTEXT_WINDOW_TOKENS)
        {
            return Err("profile_preferred_context_window_invalid".to_owned());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MembershipProfileSnapshot {
    pub conversation_id: String,
    pub membership_id: String,
    pub agent_id: String,
    pub intent_revision: i64,
    pub responsibility: ProfileResponsibility,
    #[serde(default)]
    pub required_capabilities: Vec<String>,
    #[serde(default)]
    pub preferred_capabilities: Vec<String>,
    #[serde(default)]
    pub skill_references: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_reasoning_effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_environment: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_permission_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub readiness: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price_input_usd_per_million_tokens: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price_output_usd_per_million_tokens: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intelligence_score: Option<i64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub task_tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reliability_class: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_class: Option<u8>,
    #[serde(default)]
    pub authority: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum EventKind {
    Message,
    MembershipChanged,
    Availability,
    /// One plain notice that a fresh conversation continues from an archived
    /// predecessor. Carries no member churn; the archived Conversation keeps
    /// the full prior history.
    ConversationReset,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum EventPartKind {
    Text,
    Reasoning,
    ToolCall,
    ToolResult,
    Artifact,
    Diagnostic,
    Metadata,
    Image,
}

/// Attachment metadata carried by one image Event Part. The part content is
/// this value's JSON encoding, exactly like Metadata and Diagnostic parts
/// carry structured JSON in their content; the byte size is recorded from the
/// local file at admission time, never from a caller declaration.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageAttachment {
    pub path: String,
    pub name: String,
    pub media_type: String,
    pub byte_size: u64,
}

impl ImageAttachment {
    /// Canonical content encoding for one image Event Part.
    pub fn part_content(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_owned())
    }

    /// Decode the attachment metadata from one image part's content. Returns
    /// None for malformed content so readers tolerate a part they cannot
    /// interpret instead of failing the whole event read.
    pub fn from_part_content(content: &str) -> Option<Self> {
        serde_json::from_str(content).ok()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationEvent {
    pub id: String,
    pub conversation_id: String,
    pub sequence: i64,
    pub author_membership_id: Option<String>,
    pub kind: EventKind,
    pub causation_id: Option<String>,
    pub correlation_id: Option<String>,
    pub created_at_unix_ms: i64,
    pub finalized: bool,
    pub parts: Vec<EventPart>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EventPart {
    pub id: String,
    pub event_id: String,
    pub ordinal: i64,
    pub kind: EventPartKind,
    pub content: String,
    pub created_at_unix_ms: i64,
}

impl EventPart {
    /// The decoded attachment metadata when this part is an image part with a
    /// well-formed content encoding; None for every other part.
    pub fn image_attachment(&self) -> Option<ImageAttachment> {
        (self.kind == EventPartKind::Image)
            .then(|| ImageAttachment::from_part_content(&self.content))
            .flatten()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EventPage {
    pub events: Vec<ConversationEvent>,
    pub next_cursor: Option<String>,
    pub total_count: i64,
    pub has_earlier: bool,
    pub next_before_sequence: Option<i64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceLink {
    pub id: String,
    pub conversation_id: String,
    pub source_kind: String,
    pub native_identity: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeBinding {
    pub id: String,
    pub conversation_id: String,
    pub membership_id: String,
    pub lane: String,
    pub availability: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub safe_reason: Option<String>,
}

/// Private adapter binding read only by the native runtime. It deliberately
/// has no serialization implementation and is absent from public projection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PrivateRuntimeBinding {
    pub conversation_id: String,
    pub membership_id: String,
    pub runtime_session_id: String,
    pub runtime_conversation_path: Option<String>,
    pub working_directory: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConversationDispatch {
    pub id: String,
    pub conversation_id: String,
    pub membership_id: String,
    pub operation: String,
    pub state: DispatchState,
    pub session_mode: DispatchSessionMode,
    pub runtime_conversation_path: Option<String>,
    pub error_code: Option<String>,
    pub created_at_unix_ms: i64,
    pub updated_at_unix_ms: i64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DispatchSessionMode {
    New,
    Resume,
}

/// Server-owned lineage claim for one Subagent edge. This record is durable
/// dispatch authority; native identities and prompts are deliberately absent.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubagentDispatchClaim {
    pub id: String,
    pub conversation_id: String,
    pub caller_membership_id: String,
    pub target_membership_id: String,
    pub parent_dispatch_id: Option<String>,
    pub depth: u8,
    pub state: SubagentDispatchClaimState,
    pub created_at_unix_ms: i64,
    pub updated_at_unix_ms: i64,
}

/// Privacy-safe Subagent MCP edge status. Identifiers and prompts stay out.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SubagentMeshEdge {
    pub inbound_delegate: bool,
    pub inbound_continue: bool,
    pub inbound_cancel: bool,
    pub delegate_outcome: Option<String>,
    pub continue_outcome: Option<String>,
    pub cancel_outcome: Option<String>,
    pub claim_state: Option<String>,
    pub dispatch_state: Option<String>,
}

/// Delivery category for subagent dispatch wakes and callback notices.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DispatchDeliveryKind {
    Observation,
    Terminal,
}

impl DispatchDeliveryKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Observation => "observation",
            Self::Terminal => "terminal",
        }
    }

    pub fn from_wire(s: &str) -> Option<Self> {
        match s {
            "observation" => Some(Self::Observation),
            "terminal" => Some(Self::Terminal),
            _ => None,
        }
    }
}

/// Settlement lifecycle of a dispatch delivery.
impl DispatchDeliveryState {
    pub fn from_wire(s: &str) -> Option<Self> {
        Self::from_name(s)
    }
}

/// Durable delivery record for a subagent notification or wake.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatchDeliveryRecord {
    pub claim_id: String,
    pub kind: DispatchDeliveryKind,
    pub conversation_id: String,
    pub recipient_membership_id: String,
    pub state: DispatchDeliveryState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<String>,
    pub attempt_count: u32,
    pub created_at_unix_ms: i64,
    pub updated_at_unix_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivered_at_unix_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admitted_turn_id: Option<String>,
}

/// Coalesced wake combining multiple pending notifications for one recipient
/// membership without merging away original events.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CoalescedDispatchWake {
    pub conversation_id: String,
    pub recipient_membership_id: String,
    pub claim_ids: Vec<String>,
    pub has_terminal: bool,
    pub has_observation: bool,
    pub deliveries: Vec<DispatchDeliveryRecord>,
}

/// Classification of waiting conditions tracked by the conversation store.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum WaitSourceKind {
    SubagentClaim,
    DirectTurn,
    ToolExecution,
}

impl WaitSourceKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SubagentClaim => "subagent-claim",
            Self::DirectTurn => "direct-turn",
            Self::ToolExecution => "tool-execution",
        }
    }
}

/// Record of an active or settled wait source in the conversation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WaitSourceRecord {
    pub wait_source_id: String,
    pub kind: WaitSourceKind,
    pub conversation_id: String,
    pub waiting_membership_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_membership_id: Option<String>,
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at_unix_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at_unix_ms: Option<i64>,
    pub is_terminal: bool,
}

impl DispatchSessionMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::New => "new",
            Self::Resume => "resume",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectTurn {
    pub id: String,
    pub conversation_id: String,
    pub source_event_id: String,
    pub membership_id: String,
    pub state: TurnState,
    pub ordinal: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image_part(content: String) -> EventPart {
        EventPart {
            id: "part:1".to_owned(),
            event_id: "event:1".to_owned(),
            ordinal: 1,
            kind: EventPartKind::Image,
            content,
            created_at_unix_ms: 1,
        }
    }

    #[test]
    fn image_part_content_round_trips_attachment_metadata() {
        let attachment = ImageAttachment {
            path: "fixtures/mockup.png".to_owned(),
            name: "mockup.png".to_owned(),
            media_type: "image/png".to_owned(),
            byte_size: 12,
        };
        let content = attachment.part_content();
        assert_eq!(
            ImageAttachment::from_part_content(&content).as_ref(),
            Some(&attachment)
        );
        let part = image_part(content);
        assert_eq!(part.image_attachment().as_ref(), Some(&attachment));
    }

    #[test]
    fn image_part_wire_kind_is_kebab_case_image() {
        assert_eq!(
            serde_json::to_value(EventPartKind::Image).unwrap(),
            serde_json::json!("image")
        );
        assert_eq!(
            serde_json::from_value::<EventPartKind>(serde_json::json!("image")).unwrap(),
            EventPartKind::Image
        );
    }

    #[test]
    fn image_part_attachment_decodes_only_for_well_formed_image_parts() {
        let mut text_part = image_part(
            ImageAttachment {
                path: "fixtures/a.png".to_owned(),
                name: "a.png".to_owned(),
                media_type: "image/png".to_owned(),
                byte_size: 1,
            }
            .part_content(),
        );
        text_part.kind = EventPartKind::Text;
        assert_eq!(text_part.image_attachment(), None);

        let malformed = image_part("not json".to_owned());
        assert_eq!(malformed.image_attachment(), None);

        let missing_fields = image_part(r#"{"path":"fixtures/a.png"}"#.to_owned());
        assert_eq!(missing_fields.image_attachment(), None);
    }

    fn assert_generated_state_wire<T>(state: T, wire_name: &str)
    where
        T: Copy + PartialEq + std::fmt::Debug + serde::Serialize + serde::de::DeserializeOwned,
    {
        let encoded = serde_json::to_string(&state).unwrap();
        assert_eq!(encoded, format!("\"{wire_name}\""));
        assert_eq!(serde_json::from_str::<T>(&encoded).unwrap(), state);
    }

    #[test]
    fn generated_lifecycle_states_keep_persisted_wire_names() {
        let turn = TurnState::WaitingForHuman;
        assert_eq!(turn.as_str(), "waiting-for-human");
        assert_eq!(TurnState::from_name("waiting-for-human"), Some(turn));
        assert_generated_state_wire(turn, "waiting-for-human");

        let send = crate::state_machine::SendState::Delivered;
        assert_eq!(send.as_str(), "delivered");
        assert_eq!(
            crate::state_machine::SendState::from_name("delivered"),
            Some(send)
        );
        assert_generated_state_wire(send, "delivered");

        let membership = MembershipStatus::Left;
        assert_eq!(membership.as_str(), "left");
        assert_eq!(MembershipStatus::from_name("left"), Some(membership));
        assert_generated_state_wire(membership, "left");

        let dispatch = DispatchState::CancelRequested;
        assert_eq!(dispatch.as_str(), "cancel-requested");
        assert_eq!(DispatchState::from_name("cancel-requested"), Some(dispatch));
        assert_generated_state_wire(dispatch, "cancel-requested");

        let claim = SubagentDispatchClaimState::ReconciliationRequired;
        assert_eq!(claim.as_str(), "reconciliation-required");
        assert_eq!(
            SubagentDispatchClaimState::from_name("reconciliation-required"),
            Some(claim)
        );
        assert_generated_state_wire(claim, "reconciliation-required");

        let delivery = DispatchDeliveryState::Delivering;
        assert_eq!(delivery.as_str(), "delivering");
        assert_eq!(
            DispatchDeliveryState::from_name("delivering"),
            Some(delivery)
        );
        assert_generated_state_wire(delivery, "delivering");
    }
}
