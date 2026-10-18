use super::super::protocol_selector::{
    AuthenticationEvidence, AuthenticationStatus, CapabilityEvidence, CapabilityEvidenceUpdate,
    CapabilityRequirement, CapabilitySnapshot, EvidenceError, PinnedProtocol, ProtocolKind,
    ProtocolPolicy, SelectionError, TargetProtocolRequest, project_authentication_evidence,
    reduce_capability_evidence, select_pinned_protocol,
};
use crate::platform::conversation_lane::{
    AdapterDispatchOutcome, AdapterOperationError, DispatchBounds, DispatchDisposition,
    GovernedConversationAdapter, GovernedConversationRequest, GovernedCoordinatorRequest,
    ResumeCapabilityContext, SemanticEvent, SemanticEventKind, cancel_pinned_attempt,
    cleanup_pinned_attempt, coordinate_governed_attempt, dispatch_pinned_attempt,
    resume_pinned_attempt,
};
use std::collections::BTreeSet;

fn requirements() -> CapabilityRequirement {
    CapabilityRequirement {
        streaming: true,
        semantic_completion: true,
        exact_resume: true,
        cancellation: true,
        cleanup: true,
    }
}

fn target(adapter_id: &str, protocols: Vec<ProtocolKind>, attempt: &str) -> TargetProtocolRequest {
    TargetProtocolRequest {
        attempt_id: attempt.to_owned(),
        adapter_id: adapter_id.to_owned(),
        configured_protocols: protocols,
        session_binding: format!("opaque-{attempt}"),
        required: requirements(),
    }
}

fn evidence(
    adapter_id: &str,
    protocol: ProtocolKind,
    authentication: AuthenticationEvidence,
) -> CapabilityEvidence {
    let mut state = CapabilityEvidence::unverified(
        adapter_id.to_owned(),
        format!("driver-{adapter_id}"),
        protocol,
        format!("sha256:executable-{adapter_id}"),
    );
    for update in [
        CapabilityEvidenceUpdate::Installed(true),
        CapabilityEvidenceUpdate::Executable(true),
        CapabilityEvidenceUpdate::Authentication(authentication),
        CapabilityEvidenceUpdate::ProtocolCapable(true),
        CapabilityEvidenceUpdate::SendProbeSucceeded(true),
        CapabilityEvidenceUpdate::Operations(requirements()),
    ] {
        state = reduce_capability_evidence(&state, update);
    }
    state
}

fn policy(native_allowlist: &[&str]) -> ProtocolPolicy {
    ProtocolPolicy {
        allow_acp: true,
        native_allowlist: native_allowlist
            .iter()
            .map(|value| (*value).to_owned())
            .collect::<BTreeSet<_>>(),
    }
}

fn snapshot(_fixture_label: &str, evidence: Vec<CapabilityEvidence>) -> CapabilitySnapshot {
    CapabilitySnapshot::mint(evidence).expect("synthetic reduced evidence should mint a revision")
}

#[test]
fn configured_verified_acp_is_preferred_and_authentication_unsupported_is_skipped() {
    let target = target(
        "fixture-acp-agent",
        vec![ProtocolKind::Native, ProtocolKind::Acp],
        "attempt-1",
    );
    let capabilities = snapshot(
        "caps-r1",
        vec![
            evidence(
                "fixture-acp-agent",
                ProtocolKind::Native,
                AuthenticationEvidence::Supported(true),
            ),
            evidence(
                "fixture-acp-agent",
                ProtocolKind::Acp,
                AuthenticationEvidence::Unsupported,
            ),
        ],
    );

    let expected_revision = capabilities.revision().to_owned();
    let pin = select_pinned_protocol(&target, &capabilities, &policy(&["fixture-acp-agent"]))
        .expect("configured ready ACP should be selected");

    assert_eq!(pin.protocol, ProtocolKind::Acp);
    assert_eq!(pin.adapter_id, "fixture-acp-agent");
    assert_eq!(pin.driver_id, "driver-fixture-acp-agent");
    assert_eq!(
        pin.executable_binding,
        "sha256:executable-fixture-acp-agent"
    );
    assert_eq!(pin.attempt_id, "attempt-1");
    assert_eq!(pin.session_binding, "opaque-attempt-1");
    assert_eq!(pin.capability_revision, expected_revision);
}

#[test]
fn canonical_reduction_mints_advances_and_validates_content_bound_revisions() {
    let ready = evidence(
        "fixture-agent",
        ProtocolKind::Acp,
        AuthenticationEvidence::Supported(true),
    );
    let initial = CapabilitySnapshot::mint(vec![ready.clone()]).unwrap();
    let reminted = CapabilitySnapshot::mint(vec![ready]).unwrap();
    assert_eq!(initial.revision(), reminted.revision());
    let probe_failed =
        reduce_capability_evidence(&ready, CapabilityEvidenceUpdate::SendProbeSucceeded(false));
    let advanced = initial.advance(vec![probe_failed]).unwrap();

    assert_ne!(initial.revision(), advanced.revision());
    assert!(advanced.revision().starts_with("sha256:"));
    let persisted = advanced.persisted();
    assert_eq!(
        CapabilitySnapshot::restore(persisted.clone()).unwrap(),
        advanced
    );

    let mut forged = persisted;
    forged.revision = "sha256:forged-caller-revision".to_owned();
    assert_eq!(
        CapabilitySnapshot::restore(forged),
        Err(EvidenceError::CapabilityRevisionMismatch)
    );

    let mut evidence_forgery = advanced.persisted();
    evidence_forgery.evidence = initial.persisted().evidence;
    assert_eq!(
        CapabilitySnapshot::restore(evidence_forgery),
        Err(EvidenceError::CapabilityRevisionMismatch)
    );
}

#[test]
fn authentication_evidence_does_not_block_an_available_execution_path() {
    let request = target("fixture-agent", vec![ProtocolKind::Acp], "attempt-auth");
    let capabilities = snapshot(
        "caps-auth",
        vec![evidence(
            "fixture-agent",
            ProtocolKind::Acp,
            AuthenticationEvidence::Unsupported,
        )],
    );

    assert!(select_pinned_protocol(&request, &capabilities, &policy(&[])).is_ok());

    let unauthenticated = snapshot(
        "caps-unauthenticated",
        vec![evidence(
            "fixture-agent",
            ProtocolKind::Acp,
            AuthenticationEvidence::Supported(false),
        )],
    );
    assert!(select_pinned_protocol(&request, &unauthenticated, &policy(&[])).is_ok());
}

#[test]
fn canonical_auth_receipts_project_authenticated_unauthenticated_and_skipped_without_inference() {
    assert_eq!(
        project_authentication_evidence(true, AuthenticationStatus::Authenticated),
        Ok(AuthenticationEvidence::Supported(true))
    );
    assert_eq!(
        project_authentication_evidence(true, AuthenticationStatus::Unauthenticated),
        Ok(AuthenticationEvidence::Supported(false))
    );
    assert_eq!(
        project_authentication_evidence(false, AuthenticationStatus::Skipped),
        Ok(AuthenticationEvidence::Unsupported)
    );
    assert_eq!(
        project_authentication_evidence(false, AuthenticationStatus::Authenticated),
        Err(EvidenceError::InvalidAuthenticationProjection)
    );
    assert_eq!(
        project_authentication_evidence(true, AuthenticationStatus::Skipped),
        Err(EvidenceError::InvalidAuthenticationProjection)
    );
}

#[test]
fn native_is_selected_only_when_configured_ready_and_explicitly_allowlisted() {
    let request = target(
        "fixture-native-agent",
        vec![ProtocolKind::Acp, ProtocolKind::Native],
        "attempt-2",
    );
    let capabilities = snapshot(
        "caps-r2",
        vec![
            reduce_capability_evidence(
                &evidence(
                    "fixture-native-agent",
                    ProtocolKind::Acp,
                    AuthenticationEvidence::Supported(true),
                ),
                CapabilityEvidenceUpdate::ProtocolCapable(false),
            ),
            evidence(
                "fixture-native-agent",
                ProtocolKind::Native,
                AuthenticationEvidence::Supported(true),
            ),
        ],
    );

    assert_eq!(
        select_pinned_protocol(&request, &capabilities, &policy(&[])),
        Err(SelectionError::NoAvailableProtocol)
    );
    let selected =
        select_pinned_protocol(&request, &capabilities, &policy(&["fixture-native-agent"]))
            .expect("allowlisted ready native adapter should be selected");
    assert_eq!(selected.protocol, ProtocolKind::Native);

    let both_ready = snapshot(
        "caps-r3",
        vec![
            evidence(
                "fixture-native-agent",
                ProtocolKind::Acp,
                AuthenticationEvidence::Supported(true),
            ),
            evidence(
                "fixture-native-agent",
                ProtocolKind::Native,
                AuthenticationEvidence::Supported(true),
            ),
        ],
    );
    let mut native_only_policy = policy(&["fixture-native-agent"]);
    native_only_policy.allow_acp = false;
    assert_eq!(
        select_pinned_protocol(&request, &both_ready, &native_only_policy)
            .unwrap()
            .protocol,
        ProtocolKind::Native
    );
}

#[test]
fn configured_executable_protocol_is_selected_despite_incomplete_send_evidence() {
    let request = target(
        "fixture-agent",
        vec![ProtocolKind::Native],
        "attempt-configured-only",
    );
    let capabilities = snapshot(
        "caps-configured-only",
        vec![
            evidence(
                "fixture-agent",
                ProtocolKind::Acp,
                AuthenticationEvidence::Supported(true),
            ),
            reduce_capability_evidence(
                &evidence(
                    "fixture-agent",
                    ProtocolKind::Native,
                    AuthenticationEvidence::Supported(true),
                ),
                CapabilityEvidenceUpdate::SendProbeSucceeded(false),
            ),
        ],
    );

    let selected =
        select_pinned_protocol(&request, &capabilities, &policy(&["fixture-agent"])).unwrap();
    assert_eq!(selected.protocol, ProtocolKind::Native);
}

#[test]
fn pins_reject_private_paths_and_unbounded_bindings() {
    let mut request = target("fixture-agent", vec![ProtocolKind::Acp], "attempt-binding");
    request.session_binding = ["private-user", "native-session"].join("/");
    assert_eq!(
        select_pinned_protocol(
            &request,
            &snapshot(
                "caps-binding",
                vec![evidence(
                    "fixture-agent",
                    ProtocolKind::Acp,
                    AuthenticationEvidence::Supported(true),
                )],
            ),
            &policy(&[]),
        ),
        Err(SelectionError::InvalidOpaqueBinding)
    );

    let mut invalid_executable = CapabilityEvidence::unverified(
        "fixture-agent".to_owned(),
        "driver-fixture-agent".to_owned(),
        ProtocolKind::Acp,
        ["private-user", "agent-binary"].join("/"),
    );
    for update in [
        CapabilityEvidenceUpdate::Installed(true),
        CapabilityEvidenceUpdate::Executable(true),
        CapabilityEvidenceUpdate::Authentication(AuthenticationEvidence::Supported(true)),
        CapabilityEvidenceUpdate::ProtocolCapable(true),
        CapabilityEvidenceUpdate::SendProbeSucceeded(true),
        CapabilityEvidenceUpdate::Operations(requirements()),
    ] {
        invalid_executable = reduce_capability_evidence(&invalid_executable, update);
    }
    let request = target("fixture-agent", vec![ProtocolKind::Acp], "attempt-binding");
    assert_eq!(
        select_pinned_protocol(
            &request,
            &snapshot("caps-binding", vec![invalid_executable]),
            &policy(&[]),
        ),
        Err(SelectionError::InvalidOpaqueBinding)
    );
}

#[test]
fn only_deterministic_execution_path_requirements_block_selection() {
    let request = target("fixture-agent", vec![ProtocolKind::Acp], "attempt-3");
    let baseline = evidence(
        "fixture-agent",
        ProtocolKind::Acp,
        AuthenticationEvidence::Supported(true),
    );
    let blocked = [
        CapabilityEvidenceUpdate::Installed(false),
        CapabilityEvidenceUpdate::Executable(false),
        CapabilityEvidenceUpdate::ProtocolCapable(false),
        CapabilityEvidenceUpdate::Operations(CapabilityRequirement {
            exact_resume: false,
            ..requirements()
        }),
    ]
    .map(|update| reduce_capability_evidence(&baseline, update));

    for (index, unavailable) in blocked.into_iter().enumerate() {
        assert_eq!(
            select_pinned_protocol(
                &request,
                &snapshot(&format!("blocked-{index}"), vec![unavailable]),
                &policy(&[]),
            ),
            Err(SelectionError::NoAvailableProtocol),
            "execution-path fact {index} must be independently required"
        );
    }

    for informational_update in [
        CapabilityEvidenceUpdate::Authentication(AuthenticationEvidence::Supported(false)),
        CapabilityEvidenceUpdate::SendProbeSucceeded(false),
    ] {
        let evidence = reduce_capability_evidence(&baseline, informational_update);
        assert!(
            select_pinned_protocol(
                &request,
                &snapshot("informational", vec![evidence]),
                &policy(&[]),
            )
            .is_ok()
        );
    }

    assert_eq!(
        select_pinned_protocol(&request, &snapshot("absent", vec![]), &policy(&[])),
        Err(SelectionError::NoAvailableProtocol)
    );
}

struct ScriptedAdapter {
    adapter_id: String,
    driver_id: String,
    protocol: ProtocolKind,
    executable_binding: String,
    capability_revision: String,
    session_binding: String,
    outcome: Option<Result<AdapterDispatchOutcome, AdapterOperationError>>,
    dispatches: usize,
    cancellations: usize,
    cleanups: usize,
}

impl ScriptedAdapter {
    fn for_pin(
        pin: &PinnedProtocol,
        outcome: Result<AdapterDispatchOutcome, AdapterOperationError>,
    ) -> Self {
        Self {
            adapter_id: pin.adapter_id.clone(),
            driver_id: pin.driver_id.clone(),
            protocol: pin.protocol,
            executable_binding: pin.executable_binding.clone(),
            capability_revision: pin.capability_revision.clone(),
            session_binding: pin.session_binding.clone(),
            outcome: Some(outcome),
            dispatches: 0,
            cancellations: 0,
            cleanups: 0,
        }
    }
}

impl GovernedConversationAdapter for ScriptedAdapter {
    fn adapter_id(&self) -> &str {
        &self.adapter_id
    }

    fn protocol(&self) -> ProtocolKind {
        self.protocol
    }

    fn driver_id(&self) -> &str {
        &self.driver_id
    }

    fn executable_binding(&self) -> &str {
        &self.executable_binding
    }

    fn capability_revision(&self) -> &str {
        &self.capability_revision
    }

    fn session_binding(&self) -> &str {
        &self.session_binding
    }

    fn dispatch(
        &mut self,
        _request: &GovernedConversationRequest,
    ) -> Result<AdapterDispatchOutcome, AdapterOperationError> {
        self.dispatches += 1;
        self.outcome
            .take()
            .expect("a pinned attempt may invoke its adapter at most once")
    }

    fn cancel(&mut self) -> Result<(), AdapterOperationError> {
        self.cancellations += 1;
        Ok(())
    }

    fn cleanup(&mut self) -> Result<(), AdapterOperationError> {
        self.cleanups += 1;
        Ok(())
    }
}

fn request(pin: PinnedProtocol) -> GovernedConversationRequest {
    GovernedConversationRequest {
        pin,
        input_artifact_handle: "artifact-in-01".to_owned(),
        input_digest: "sha256:input-01".to_owned(),
    }
}

fn completed_events() -> Vec<SemanticEvent> {
    vec![
        SemanticEvent {
            sequence: 1,
            kind: SemanticEventKind::Started,
        },
        SemanticEvent {
            sequence: 2,
            kind: SemanticEventKind::Progress {
                artifact_handle: "artifact-progress-01".to_owned(),
                digest: "sha256:progress-01".to_owned(),
            },
        },
        SemanticEvent {
            sequence: 3,
            kind: SemanticEventKind::Completed {
                artifact_handle: "artifact-out-01".to_owned(),
                digest: "sha256:output-01".to_owned(),
            },
        },
    ]
}

fn fixture_pin(adapter: &str, protocol: ProtocolKind, attempt: &str) -> PinnedProtocol {
    let native_allowlist = if protocol == ProtocolKind::Native {
        vec![adapter]
    } else {
        Vec::new()
    };
    select_pinned_protocol(
        &target(adapter, vec![protocol], attempt),
        &snapshot(
            "caps-fixture-r1",
            vec![evidence(
                adapter,
                protocol,
                AuthenticationEvidence::Unsupported,
            )],
        ),
        &policy(&native_allowlist),
    )
    .unwrap()
}

#[test]
fn synthetic_kimi_acp_and_claude_native_are_data_only_and_dispatch_through_real_lane() {
    let fixtures = [
        ("kimi-code", ProtocolKind::Acp, "kimi-attempt"),
        ("claude-code", ProtocolKind::Native, "claude-attempt"),
    ];

    for (adapter_id, protocol, attempt_id) in fixtures {
        let pin = fixture_pin(adapter_id, protocol, attempt_id);
        let outcome = AdapterDispatchOutcome {
            session_binding: pin.session_binding.clone(),
            events: completed_events(),
            disposition: DispatchDisposition::Completed,
        };
        let mut adapter = ScriptedAdapter::for_pin(&pin, Ok(outcome));
        let result = dispatch_pinned_attempt(
            &request(pin.clone()),
            &mut adapter,
            DispatchBounds { max_events: 8 },
        )
        .unwrap();

        assert_eq!(result.pin, pin);
        assert_eq!(result.events, completed_events());
        assert_eq!(result.disposition, DispatchDisposition::Completed);
        assert_eq!(adapter.dispatches, 1);
    }

    // A second assignment changes selection and emitted dispatch metadata with
    // data only. No source routing branch is required.
    let alternate = fixture_pin("alternate-agent", ProtocolKind::Acp, "alternate-attempt");
    let mut adapter = ScriptedAdapter::for_pin(
        &alternate,
        Ok(AdapterDispatchOutcome {
            session_binding: alternate.session_binding.clone(),
            events: completed_events(),
            disposition: DispatchDisposition::Completed,
        }),
    );
    let result = dispatch_pinned_attempt(
        &request(alternate.clone()),
        &mut adapter,
        DispatchBounds { max_events: 8 },
    )
    .unwrap();
    assert_eq!(result.pin.adapter_id, "alternate-agent");
    assert_eq!(result.pin.protocol, ProtocolKind::Acp);
}

#[test]
fn resume_rejects_any_attempt_protocol_adapter_session_or_capability_revision_mismatch() {
    let pin = fixture_pin("fixture-agent", ProtocolKind::Acp, "attempt-resume");
    let variants = [
        PinnedProtocol {
            attempt_id: "different-attempt".to_owned(),
            ..pin.clone()
        },
        PinnedProtocol {
            adapter_id: "different-adapter".to_owned(),
            ..pin.clone()
        },
        PinnedProtocol {
            driver_id: "different-driver".to_owned(),
            ..pin.clone()
        },
        PinnedProtocol {
            protocol: ProtocolKind::Native,
            ..pin.clone()
        },
        PinnedProtocol {
            session_binding: "different-session".to_owned(),
            ..pin.clone()
        },
        PinnedProtocol {
            capability_revision: "different-revision".to_owned(),
            ..pin.clone()
        },
        PinnedProtocol {
            executable_binding: "sha256:different-executable".to_owned(),
            ..pin.clone()
        },
    ];

    for mismatched in variants {
        let mut adapter = ScriptedAdapter::for_pin(
            &pin,
            Ok(AdapterDispatchOutcome {
                session_binding: pin.session_binding.clone(),
                events: completed_events(),
                disposition: DispatchDisposition::Completed,
            }),
        );
        let error = dispatch_pinned_attempt(
            &request(mismatched),
            &mut adapter,
            DispatchBounds { max_events: 8 },
        )
        .unwrap_err();
        assert_eq!(error, AdapterOperationError::PinnedBindingMismatch);
        assert_eq!(adapter.dispatches, 0);
    }
}

#[test]
fn persisted_resume_uses_the_original_pin_after_policy_and_current_snapshot_change() {
    let request_target = target(
        "fixture-agent",
        vec![ProtocolKind::Acp, ProtocolKind::Native],
        "attempt-persisted-resume",
    );
    let original_snapshot = snapshot(
        "caps-original",
        vec![
            evidence(
                "fixture-agent",
                ProtocolKind::Acp,
                AuthenticationEvidence::Supported(true),
            ),
            evidence(
                "fixture-agent",
                ProtocolKind::Native,
                AuthenticationEvidence::Supported(true),
            ),
        ],
    );
    let original_pin = select_pinned_protocol(
        &request_target,
        &original_snapshot,
        &policy(&["fixture-agent"]),
    )
    .unwrap();
    assert_eq!(original_pin.protocol, ProtocolKind::Acp);
    let persisted = serde_json::to_vec(&original_pin).unwrap();
    let restored_pin: PinnedProtocol = serde_json::from_slice(&persisted).unwrap();

    let changed_snapshot = original_snapshot
        .advance(vec![evidence(
            "fixture-agent",
            ProtocolKind::Native,
            AuthenticationEvidence::Supported(true),
        )])
        .unwrap();
    let mut changed_policy = policy(&["fixture-agent"]);
    changed_policy.allow_acp = false;
    let context = ResumeCapabilityContext {
        pinned_snapshot: &original_snapshot,
        current_snapshot: &changed_snapshot,
        current_policy: &changed_policy,
    };
    let mut original_adapter = ScriptedAdapter::for_pin(
        &original_pin,
        Ok(AdapterDispatchOutcome {
            session_binding: original_pin.session_binding.clone(),
            events: completed_events(),
            disposition: DispatchDisposition::Completed,
        }),
    );
    let resumed = resume_pinned_attempt(
        &request(restored_pin),
        &context,
        &mut original_adapter,
        DispatchBounds { max_events: 8 },
    )
    .unwrap();
    assert_eq!(resumed.pin, original_pin);
    assert_eq!(resumed.disposition, DispatchDisposition::Completed);
    assert_eq!(original_adapter.dispatches, 1);

    let invalid_context = ResumeCapabilityContext {
        pinned_snapshot: &changed_snapshot,
        current_snapshot: &changed_snapshot,
        current_policy: &changed_policy,
    };
    let mut rejected_adapter = ScriptedAdapter::for_pin(
        &original_pin,
        Ok(AdapterDispatchOutcome {
            session_binding: original_pin.session_binding.clone(),
            events: completed_events(),
            disposition: DispatchDisposition::Completed,
        }),
    );
    assert_eq!(
        resume_pinned_attempt(
            &request(original_pin),
            &invalid_context,
            &mut rejected_adapter,
            DispatchBounds { max_events: 8 },
        ),
        Err(AdapterOperationError::PinnedBindingMismatch)
    );
    assert_eq!(rejected_adapter.dispatches, 0);
}

#[test]
fn semantic_events_are_ordered_bounded_and_end_in_exactly_one_terminal_event() {
    let pin = fixture_pin("fixture-agent", ProtocolKind::Acp, "attempt-events");
    let cases = [
        vec![
            SemanticEvent {
                sequence: 2,
                kind: SemanticEventKind::Started,
            },
            SemanticEvent {
                sequence: 1,
                kind: SemanticEventKind::Completed {
                    artifact_handle: "artifact".to_owned(),
                    digest: "digest".to_owned(),
                },
            },
        ],
        vec![SemanticEvent {
            sequence: 1,
            kind: SemanticEventKind::Progress {
                artifact_handle: "artifact".to_owned(),
                digest: "digest".to_owned(),
            },
        }],
        vec![
            SemanticEvent {
                sequence: 1,
                kind: SemanticEventKind::Completed {
                    artifact_handle: "artifact-1".to_owned(),
                    digest: "digest-1".to_owned(),
                },
            },
            SemanticEvent {
                sequence: 2,
                kind: SemanticEventKind::Completed {
                    artifact_handle: "artifact-2".to_owned(),
                    digest: "digest-2".to_owned(),
                },
            },
        ],
    ];

    for events in cases {
        let mut adapter = ScriptedAdapter::for_pin(
            &pin,
            Ok(AdapterDispatchOutcome {
                session_binding: pin.session_binding.clone(),
                events,
                disposition: DispatchDisposition::Completed,
            }),
        );
        assert_eq!(
            dispatch_pinned_attempt(
                &request(pin.clone()),
                &mut adapter,
                DispatchBounds { max_events: 8 },
            ),
            Err(AdapterOperationError::InvalidSemanticEvents)
        );
    }

    let mut too_many = ScriptedAdapter::for_pin(
        &pin,
        Ok(AdapterDispatchOutcome {
            session_binding: pin.session_binding.clone(),
            events: completed_events(),
            disposition: DispatchDisposition::Completed,
        }),
    );
    assert_eq!(
        dispatch_pinned_attempt(
            &request(pin),
            &mut too_many,
            DispatchBounds { max_events: 2 },
        ),
        Err(AdapterOperationError::EventLimitExceeded)
    );
}

#[test]
fn cancellation_and_cleanup_use_the_exact_pin_and_unknown_never_retries_or_falls_back() {
    let pin = fixture_pin("fixture-agent", ProtocolKind::Acp, "attempt-controls");
    let mut control_adapter =
        ScriptedAdapter::for_pin(&pin, Err(AdapterOperationError::UnknownOutcome));
    cancel_pinned_attempt(&pin, &mut control_adapter).unwrap();
    cleanup_pinned_attempt(&pin, &mut control_adapter).unwrap();
    assert_eq!(control_adapter.cancellations, 1);
    assert_eq!(control_adapter.cleanups, 1);

    let mut mismatched_control =
        ScriptedAdapter::for_pin(&pin, Err(AdapterOperationError::UnknownOutcome));
    mismatched_control.session_binding = "wrong-session".to_owned();
    assert_eq!(
        cancel_pinned_attempt(&pin, &mut mismatched_control),
        Err(AdapterOperationError::PinnedBindingMismatch)
    );
    assert_eq!(
        cleanup_pinned_attempt(&pin, &mut mismatched_control),
        Err(AdapterOperationError::PinnedBindingMismatch)
    );
    assert_eq!(mismatched_control.cancellations, 0);
    assert_eq!(mismatched_control.cleanups, 0);

    let result = dispatch_pinned_attempt(
        &request(pin.clone()),
        &mut control_adapter,
        DispatchBounds { max_events: 8 },
    );
    assert_eq!(result, Err(AdapterOperationError::UnknownOutcome));
    assert_eq!(control_adapter.dispatches, 1);
    assert_eq!(control_adapter.cancellations, 1);
    assert_eq!(control_adapter.cleanups, 1);

    // A different ready adapter exists but is never passed to or selected by
    // the pinned attempt after an unknown external outcome.
    let fallback_pin = fixture_pin("fallback-agent", ProtocolKind::Native, "fallback-attempt");
    let fallback = ScriptedAdapter::for_pin(
        &fallback_pin,
        Ok(AdapterDispatchOutcome {
            session_binding: fallback_pin.session_binding.clone(),
            events: completed_events(),
            disposition: DispatchDisposition::Completed,
        }),
    );
    assert_eq!(fallback.dispatches, 0);
}

#[test]
fn real_coordinator_terminalizes_unknown_once_and_never_wakes_the_ready_secondary() {
    let request_target = target(
        "fixture-agent",
        vec![ProtocolKind::Native, ProtocolKind::Acp],
        "attempt-coordinator",
    );
    let capabilities = snapshot(
        "caps-coordinator",
        vec![
            evidence(
                "fixture-agent",
                ProtocolKind::Acp,
                AuthenticationEvidence::Supported(true),
            ),
            evidence(
                "fixture-agent",
                ProtocolKind::Native,
                AuthenticationEvidence::Supported(true),
            ),
        ],
    );
    let primary_pin =
        select_pinned_protocol(&request_target, &capabilities, &policy(&["fixture-agent"]))
            .unwrap();
    assert_eq!(primary_pin.protocol, ProtocolKind::Acp);
    let secondary_pin = PinnedProtocol {
        protocol: ProtocolKind::Native,
        ..primary_pin.clone()
    };
    let mut primary =
        ScriptedAdapter::for_pin(&primary_pin, Err(AdapterOperationError::UnknownOutcome));
    let mut secondary = ScriptedAdapter::for_pin(
        &secondary_pin,
        Ok(AdapterDispatchOutcome {
            session_binding: secondary_pin.session_binding.clone(),
            events: completed_events(),
            disposition: DispatchDisposition::Completed,
        }),
    );
    let outcome = {
        let mut adapters: [&mut dyn GovernedConversationAdapter; 2] =
            [&mut primary, &mut secondary];
        coordinate_governed_attempt(
            &GovernedCoordinatorRequest {
                target: request_target,
                input_artifact_handle: "artifact-in-coordinator".to_owned(),
                input_digest: "sha256:input-coordinator".to_owned(),
            },
            &capabilities,
            &policy(&["fixture-agent"]),
            &mut adapters,
            DispatchBounds { max_events: 8 },
        )
        .unwrap()
    };

    assert_eq!(outcome.pin, primary_pin);
    assert_eq!(outcome.disposition, DispatchDisposition::Unknown);
    assert!(outcome.events.is_empty());
    assert_eq!(primary.dispatches, 1);
    assert_eq!(secondary.dispatches, 0);
}

#[test]
fn governed_values_are_privacy_minimal_opaque_handles_and_digests() {
    let pin = fixture_pin("fixture-agent", ProtocolKind::Acp, "attempt-private");
    let value = serde_json::to_string(&request(pin)).unwrap();
    let private_path_canary = ["private-user", "path-canary"].join("/");
    for forbidden in [
        "raw prompt canary",
        "raw provider output canary",
        "native-session-id-canary",
        "credential-canary",
        private_path_canary.as_str(),
    ] {
        assert!(!value.contains(forbidden));
    }
    assert!(value.contains("artifact-in-01"));
    assert!(value.contains("sha256:input-01"));
}

#[test]
fn sensitive_bindings_and_adapter_events_are_rejected_before_projection() {
    let pin = fixture_pin("fixture-agent", ProtocolKind::Acp, "attempt-canary");
    let valid_outcome = || AdapterDispatchOutcome {
        session_binding: pin.session_binding.clone(),
        events: completed_events(),
        disposition: DispatchDisposition::Completed,
    };

    let mut bad_input_adapter = ScriptedAdapter::for_pin(&pin, Ok(valid_outcome()));
    let mut bad_input = request(pin.clone());
    bad_input.input_artifact_handle = ["private-user", "prompt-canary"].join("/");
    let error = dispatch_pinned_attempt(
        &bad_input,
        &mut bad_input_adapter,
        DispatchBounds { max_events: 8 },
    )
    .unwrap_err();
    assert_eq!(error, AdapterOperationError::SensitiveEvidenceRejected);
    assert_eq!(bad_input_adapter.dispatches, 0);
    assert!(!format!("{error:?}").contains("prompt-canary"));

    let mut bad_event_adapter = ScriptedAdapter::for_pin(
        &pin,
        Ok(AdapterDispatchOutcome {
            session_binding: pin.session_binding.clone(),
            events: vec![
                SemanticEvent {
                    sequence: 1,
                    kind: SemanticEventKind::Started,
                },
                SemanticEvent {
                    sequence: 2,
                    kind: SemanticEventKind::Completed {
                        artifact_handle: "raw-provider-output-canary".to_owned(),
                        digest: "credential-canary".to_owned(),
                    },
                },
            ],
            disposition: DispatchDisposition::Completed,
        }),
    );
    let error = dispatch_pinned_attempt(
        &request(pin.clone()),
        &mut bad_event_adapter,
        DispatchBounds { max_events: 8 },
    )
    .unwrap_err();
    assert_eq!(error, AdapterOperationError::SensitiveEvidenceRejected);
    assert_eq!(bad_event_adapter.dispatches, 1);
    let redacted = format!("{error:?}");
    assert!(!redacted.contains("raw-provider-output-canary"));
    assert!(!redacted.contains("credential-canary"));

    let mut bad_binding_adapter = ScriptedAdapter::for_pin(
        &pin,
        Ok(AdapterDispatchOutcome {
            session_binding: "native-session-id-canary".to_owned(),
            events: completed_events(),
            disposition: DispatchDisposition::Completed,
        }),
    );
    let error = dispatch_pinned_attempt(
        &request(pin.clone()),
        &mut bad_binding_adapter,
        DispatchBounds { max_events: 8 },
    )
    .unwrap_err();
    assert_eq!(error, AdapterOperationError::SensitiveEvidenceRejected);
    assert_eq!(bad_binding_adapter.dispatches, 1);
    assert!(!format!("{error:?}").contains("native-session-id-canary"));

    let mut valid_adapter = ScriptedAdapter::for_pin(&pin, Ok(valid_outcome()));
    let accepted = dispatch_pinned_attempt(
        &request(pin),
        &mut valid_adapter,
        DispatchBounds { max_events: 8 },
    )
    .unwrap();
    assert_eq!(accepted.events, completed_events());
}
