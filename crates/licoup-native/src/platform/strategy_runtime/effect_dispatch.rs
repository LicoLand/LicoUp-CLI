//! Production effect delivery and the live runtime Agent profile.
//!
//! Nothing here is a second executor. [`LaneEffectDispatch`] calls the same
//! agent-execution port entry the strategy actor effect already calls, and
//! [`RuntimeRegistryAgentProfiles`] reads the hot-reloadable driver registry
//! instead of a compiled-in vendor table: an effect consumes whatever dynamic
//! instance the profile describes for the agent id it was given.
//!
//! What the lane does NOT give back is stated rather than invented: a lane send
//! settles its turn inside the call and keeps no per-effect record in this
//! process, and the lane's cancel answers describe the request, not the effect's
//! fate. Both are reported as such, so an unresolved effect stays in doubt
//! instead of being written down as a completion.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::platform::work_context_ports::{
    AgentProfileSource, ControlDelivery, ControlDisposition, DeliveryOutcome, EffectControl,
    EffectDelivery, EffectDispatch, EffectHandle, EffectInvocation, EffectUnknownReason,
    RuntimeAgentProfile, effect_input_text,
};

/// Keep the lane's stderr bound identical to the strategy actor effect's.
const LANE_MAX_STDERR_BYTES: usize = 512 * 1024;

/// The live runtime Agent registry: the packaged driver inventory plus its
/// hot-reloadable readiness projection.
pub(crate) struct RuntimeRegistryAgentProfiles;

impl AgentProfileSource for RuntimeRegistryAgentProfiles {
    fn profile(&self, agent_id: &str) -> Option<RuntimeAgentProfile> {
        let agent_id = agent_id.trim();
        if agent_id.is_empty() {
            return None;
        }
        let profile = crate::platform::runtime_adapters::runtime_driver_profile(agent_id)?;
        let driver_id = crate::platform::runtime_adapters::adapter_for_agent_public(agent_id)?
            .driver_id()
            .to_owned();
        Some(RuntimeAgentProfile {
            agent_id: agent_id.to_owned(),
            driver_id,
            runtime_protocol: profile.protocol,
            lane_family: profile
                .capability_matrix
                .as_ref()
                .and_then(|matrix| matrix.get("laneFamily"))
                .and_then(Value::as_str)
                .map(str::to_owned),
            readiness: profile.readiness,
            blocker: profile.blocker,
            declarations: declared_flags(profile.capability_matrix.as_ref()),
        })
    }
}

/// Boolean declarations are read from the profile's own capability matrix, so a
/// vendor that adds or drops a flag changes the answer without a code change.
fn declared_flags(matrix: Option<&Value>) -> BTreeMap<String, bool> {
    matrix
        .and_then(Value::as_object)
        .map(|object| {
            object
                .iter()
                .filter_map(|(key, value)| value.as_bool().map(|flag| (key.clone(), flag)))
                .collect()
        })
        .unwrap_or_default()
}

/// The existing conversation lane, reached the way the strategy actor effect
/// already reaches it.
///
/// It carries no injected surface: the lane entry it calls is this crate root's
/// own composition, and the Agent inventory port declares no dispatch member,
/// so a port field here would be a value nothing reads.
#[derive(Default)]
pub(crate) struct LaneEffectDispatch;

impl EffectDispatch for LaneEffectDispatch {
    fn deliver(&self, invocation: &EffectInvocation) -> EffectDelivery {
        let mut params = json!({
            "agent": invocation.agent_id,
            "message": effect_input_text(&invocation.input),
            "sessionId": invocation.turn.native_session_id,
            "timeoutMs": 0,
            "maxStderrBytes": LANE_MAX_STDERR_BYTES,
        });
        if let Value::Object(ref mut object) = params {
            for key in ["model", "reasoningEffort", "cwd", "workingDirectory"] {
                if let Some(value) = invocation
                    .input
                    .get(key)
                    .and_then(Value::as_str)
                    .filter(|value| !value.trim().is_empty())
                {
                    object.insert(key.to_owned(), Value::String(value.to_owned()));
                }
            }
        }
        let response = match crate::agent_port::send(&params) {
            Ok(value) => value,
            Err(_) => {
                return EffectDelivery::unconfirmed(
                    "lane_dispatch_failed",
                    EffectUnknownReason::AdapterUnreachable,
                );
            }
        };
        classify_send(&response)
    }

    fn read_back(&self, _handle: &EffectHandle) -> Option<EffectDelivery> {
        // The lane settles a send inside the call and keeps no per-effect record
        // this process can read back. Reporting one would be a fabricated fact;
        // `None` keeps the effect in doubt instead.
        None
    }

    fn control(
        &self,
        handle: &EffectHandle,
        control: EffectControl,
        instruction: Option<&str>,
    ) -> ControlDelivery {
        let operation = match control {
            EffectControl::Steer => "steer",
            EffectControl::Cancel => "cancel",
        };
        let mut params = json!({
            "agent": handle.agent_id,
            "sessionId": handle.turn.native_session_id,
            "turnId": handle.turn.native_turn_id,
        });
        if let Some(instruction) = instruction {
            params["text"] = Value::String(instruction.to_owned());
        }
        let response = match crate::agent_port::dispatch(operation, &params) {
            Ok(value) => value,
            Err(_) => {
                // The lane call failed. That alone cannot establish whether the
                // request reached the adapter before the failure: the lane
                // collapses its own errors into one dispatch failure, and a
                // failure is not evidence of a pre-dispatch refusal. It stays
                // unconfirmed, never "not delivered".
                return ControlDelivery {
                    disposition: ControlDisposition::Unconfirmed,
                    status: "lane_dispatch_failed".to_owned(),
                };
            }
        };
        control_delivery(&response)
    }
}

/// A lane control answer, read for the fact it actually states.
///
/// The tokens are the ones `conversation_lane::cancel_turn` / `steer_turn`
/// publish. Each fact is read only from the exact shape the lane uses for it:
/// an accepted request carries `ok: true`; the negative answers carry
/// `ok: false`. Any other combination — an unknown token, an empty status, or a
/// success flag that contradicts the status — is `Unconfirmed`: that answer
/// cannot establish whether the request reached the adapter, so nothing may be
/// claimed about delivery either way. Only a surface that reports
/// `NotDelivered` has established the request never left the process.
fn control_delivery(response: &Value) -> ControlDelivery {
    let status = response
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let ok = response.get("ok").and_then(Value::as_bool);
    let disposition = match (ok, status.as_str()) {
        (Some(true), "cancel_requested" | "accepted") => ControlDisposition::Accepted,
        (Some(false), "not_active" | "no_active_turn") => ControlDisposition::NoActiveTurn,
        (Some(false), "not_found" | "session_unavailable") => {
            ControlDisposition::SessionUnavailable
        }
        (Some(false), "unsupported") => ControlDisposition::Unsupported,
        _ => ControlDisposition::Unconfirmed,
    };
    ControlDelivery {
        disposition,
        status,
    }
}

/// A lane send answer, read for what it actually says.
fn classify_send(response: &Value) -> EffectDelivery {
    let status = response
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    if response.get("ok").and_then(Value::as_bool) == Some(true) {
        // The lane settles the turn inside the call, so an accepted send carries
        // the attempt's materialised result. Its content is the adapter's own
        // report and is preserved verbatim.
        let output = match response.get("output").and_then(Value::as_str) {
            Some(raw) => {
                serde_json::from_str::<Value>(raw).unwrap_or_else(|_| Value::String(raw.to_owned()))
            }
            None => response.clone(),
        };
        return EffectDelivery {
            outcome: DeliveryOutcome::Completed { output },
            status,
        };
    }
    // A non-ok answer describes the call, not the effect's fate: whether the
    // external effect happened is not something this answer can decide.
    EffectDelivery::unconfirmed(
        if status.is_empty() {
            "lane_unconfirmed".to_owned()
        } else {
            status
        },
        EffectUnknownReason::NoRecordedResult,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ok_send_is_read_as_the_attempt_s_materialised_result() {
        let parsed = classify_send(&json!({
            "ok": true,
            "status": "completed",
            "output": "{\"ok\":true,\"answer\":\"fixture\"}",
        }));
        assert_eq!(
            parsed.outcome,
            DeliveryOutcome::Completed {
                output: json!({"ok": true, "answer": "fixture"})
            }
        );
        assert_eq!(parsed.status, "completed");

        // Non-JSON output is preserved as it stands rather than being dropped.
        let raw = classify_send(&json!({"ok": true, "output": "plain text"}));
        assert_eq!(
            raw.outcome,
            DeliveryOutcome::Completed {
                output: Value::String("plain text".into())
            }
        );

        // An ok answer with no output field still carries the adapter's report.
        let bare = classify_send(&json!({"ok": true, "status": "completed"}));
        assert!(matches!(bare.outcome, DeliveryOutcome::Completed { .. }));
    }

    #[test]
    fn a_non_ok_send_is_never_read_as_a_terminal_fact() {
        let failed = classify_send(&json!({
            "ok": false,
            "status": "not_active",
            "error": {"code": "codex_turn_not_active"},
        }));
        assert_eq!(
            failed.outcome,
            DeliveryOutcome::Unconfirmed {
                reason: EffectUnknownReason::NoRecordedResult
            },
            "an error answer describes the call, not whether the effect happened"
        );
        assert_eq!(failed.status, "not_active");

        let bare = classify_send(&json!({"ok": false}));
        assert_eq!(
            bare.outcome,
            DeliveryOutcome::Unconfirmed {
                reason: EffectUnknownReason::NoRecordedResult
            }
        );
        assert_eq!(bare.status, "lane_unconfirmed");
    }

    #[test]
    fn declarations_come_from_the_profile_flags_only() {
        let flags = declared_flags(Some(&json!({
            "laneFamily": "rpc",
            "cancel": false,
            "openNew": true,
            "nested": {"cancel": true},
            "vendor.example/render": true,
            "vendor.example/hint": "text",
        })));
        assert_eq!(flags.len(), 3);
        assert_eq!(flags.get("cancel"), Some(&false));
        assert_eq!(flags.get("openNew"), Some(&true));
        // A namespaced boolean is carried verbatim; it is never read as one of
        // the operations this bridge asks about. Non-boolean attributes are not
        // declarations at all.
        assert_eq!(flags.get("vendor.example/render"), Some(&true));
        assert!(!flags.contains_key("nested"));
        assert!(!flags.contains_key("vendor.example/hint"));
        assert!(declared_flags(None).is_empty());
    }

    #[test]
    fn lane_control_answers_map_to_the_fact_they_state() {
        // The exact answers `cancel_turn` publishes.
        assert_eq!(
            control_delivery(&json!({"ok": true, "status": "cancel_requested"})).disposition,
            ControlDisposition::Accepted
        );
        assert_eq!(
            control_delivery(&json!({"ok": false, "status": "not_active"})).disposition,
            ControlDisposition::NoActiveTurn
        );
        assert_eq!(
            control_delivery(&json!({"ok": false, "status": "not_found"})).disposition,
            ControlDisposition::SessionUnavailable
        );
        assert_eq!(
            control_delivery(&json!({"ok": false, "status": "unsupported"})).disposition,
            ControlDisposition::Unsupported
        );
        // The exact answers `steer_turn` publishes.
        assert_eq!(
            control_delivery(&json!({"ok": true, "status": "accepted"})).disposition,
            ControlDisposition::Accepted
        );
        assert_eq!(
            control_delivery(&json!({"ok": false, "status": "no_active_turn"})).disposition,
            ControlDisposition::NoActiveTurn
        );
        assert_eq!(
            control_delivery(&json!({"ok": false, "status": "session_unavailable"})).disposition,
            ControlDisposition::SessionUnavailable
        );
        // A lane-level failure answer (`unavailable`) is a transport-level
        // answer: it cannot establish whether the request reached the adapter.
        assert_eq!(
            control_delivery(&json!({"ok": false, "status": "unavailable"})).disposition,
            ControlDisposition::Unconfirmed
        );
        assert_eq!(
            control_delivery(&json!({"ok": false, "status": "blocked"})).disposition,
            ControlDisposition::Unconfirmed
        );
        // Empty, unknown, or self-contradicting answers are unconfirmed, never
        // delivered and never "not reached".
        assert_eq!(
            control_delivery(&json!({"ok": false, "status": ""})).disposition,
            ControlDisposition::Unconfirmed
        );
        assert_eq!(
            control_delivery(&json!({"ok": true, "status": ""})).disposition,
            ControlDisposition::Unconfirmed
        );
        assert_eq!(
            control_delivery(&json!({"ok": true, "status": "not_active"})).disposition,
            ControlDisposition::Unconfirmed
        );
        assert_eq!(
            control_delivery(&json!({"ok": false, "status": "cancel_requested"})).disposition,
            ControlDisposition::Unconfirmed
        );
        assert_eq!(
            control_delivery(&json!({"status": "cancel_requested"})).disposition,
            ControlDisposition::Unconfirmed
        );
        // A bare object with no answer at all is unconfirmed too.
        assert_eq!(
            control_delivery(&json!({})).disposition,
            ControlDisposition::Unconfirmed
        );
    }

    /// The real lane, reached with the exact shapes its control entry points
    /// reject before any process is touched. Each returns `Err`, which this
    /// surface may only report as unconfirmed: an error is not proof the request
    /// never reached the adapter.
    #[test]
    fn a_failed_lane_call_is_unconfirmed_not_a_pre_dispatch_refusal() {
        use licoup_agent_runtime::work_context::NativeWorkContextKey;

        use crate::platform::work_context_ports::{EffectState, EffectTurn};

        let handle = EffectHandle {
            effect_id: "effect:unit".into(),
            attempt_token: "effect:unit-attempt-1".into(),
            agent_id: "not-a-runtime-adapter".into(),
            driver_id: "fixture-driver".into(),
            runtime_protocol: "fixture-protocol".into(),
            session: NativeWorkContextKey {
                conversation_id: "conversation:fixture".into(),
                membership_id: "membership:fixture".into(),
                matter_id: "matter:fixture".into(),
                generation: 1,
            },
            turn: EffectTurn {
                host_handle: "turn:fixture".into(),
                native_session_id: "native-session:fixture".into(),
                native_turn_id: "native-turn:fixture".into(),
            },
            state: EffectState::InFlight,
        };
        let dispatch = LaneEffectDispatch;
        // The adapter id does not resolve: the lane call fails.
        let unresolved = dispatch.control(&handle, EffectControl::Cancel, None);
        assert_eq!(unresolved.disposition, ControlDisposition::Unconfirmed);
        assert_eq!(unresolved.status, "lane_dispatch_failed");
        let unresolved = dispatch.control(&handle, EffectControl::Steer, Some("guidance"));
        assert_eq!(unresolved.disposition, ControlDisposition::Unconfirmed);

        // A packaged adapter with no exact native session fails too, and still
        // only proves the call failed.
        let sessionless = EffectHandle {
            agent_id: "codex".into(),
            turn: EffectTurn {
                host_handle: "turn:fixture".into(),
                native_session_id: String::new(),
                native_turn_id: String::new(),
            },
            ..handle
        };
        let failed = dispatch.control(&sessionless, EffectControl::Cancel, None);
        assert_eq!(failed.disposition, ControlDisposition::Unconfirmed);
        assert_eq!(failed.status, "lane_dispatch_failed");
    }

    #[test]
    fn the_profile_source_reads_the_live_registry_or_nothing() {
        let profiles = RuntimeRegistryAgentProfiles;
        let codex = profiles.profile("codex").expect("codex is packaged");
        assert_eq!(codex.driver_id, "codex-app-server");
        assert_eq!(codex.declares("cancel"), Some(true));
        // The namespaced/alternative ids of the same instance answer as one.
        assert_eq!(
            profiles.profile(" pi ").map(|pi| pi.driver_id),
            Some("pi-rpc".to_owned())
        );
        assert_eq!(profiles.profile("not-a-runtime-adapter"), None);
        assert_eq!(profiles.profile("   "), None);
    }
}
