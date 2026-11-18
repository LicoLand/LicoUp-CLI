//! The call, capability and extension-catalog contract.
//!
//! These are the rules both interfaces have to apply the same way: which names
//! are namespaced, which contract versions fit together, what happens to an
//! unknown optional attribute versus an unsupported required capability, which
//! fields free-form input may never write, and what survives of a failure end to
//! end. Each case is written so that passing it means the rule holds, not that
//! the code compiles.

use licoup_application::{
    AUTHORITY_FIELDS, ActorClaim, ApplicationCommand, ApplicationFailure, AuthorityHandle,
    AuthoritySource, CapabilityDescriptor, ContractCompatibility, ContractRange, DeclaredAttribute,
    DiscoveredCapabilities, EffectCertainty, InvocationScope, LifecycleSupport,
    MAX_NAMESPACED_NAME_BYTES, MAX_PRESENTATION_ARGS, NaturalOutput, Operation, OperationReference,
    OperationState, PresentationArgs, QuotaShape, ReceiptKind, RecoveryAction, Requirement,
    SubagentCommand, ToolInvocation, ToolReceipt, is_authority_field, is_namespaced, is_semver,
};
use serde_json::{Value, json};
use std::collections::BTreeSet;

fn descriptor() -> CapabilityDescriptor {
    CapabilityDescriptor {
        plugin_id: "vendor.example.render".into(),
        implementation_version: "1.4.0".into(),
        supported_contract_range: ContractRange {
            major: 1,
            minimum_minor: 3,
        },
        capabilities: vec!["vendor.example/render".into()],
        required_grants: vec!["licoup.permission.conversation-read".into()],
        quota_shape: QuotaShape {
            dimensions: vec!["licoup.quota.tokens".into()],
        },
        lifecycle: LifecycleSupport {
            activation: Default::default(),
            operations: vec!["extension.initialize".into()],
        },
        attributes: vec![
            DeclaredAttribute::optional("vendor.example/theme").with_value(json!({"mode": "dark"})),
            DeclaredAttribute::required("vendor.example/render"),
        ],
    }
}

fn invocation(scope: InvocationScope) -> ToolInvocation {
    ToolInvocation::new(
        "request:one",
        Operation::SubagentsList,
        scope,
        AuthorityHandle::from_verified_source(
            ActorClaim::membership("codex", "conversation:one", "membership:codex"),
            AuthoritySource::VerifiedTransport,
        ),
    )
}

// ---------------------------------------------------------------------------
// Namespaced identity
// ---------------------------------------------------------------------------

#[test]
fn namespaced_names_admit_vendors_and_reject_bare_words() {
    for name in [
        "vendor.example/render",
        "vendor.example/render.width",
        "licoup.quota.tokens",
        "licoup.permission.conversation-read",
        "vendor.example/Render",
        "vendor.example.Render",
        "vendor.example.render_width",
        "vendor.example-Render",
    ] {
        assert!(is_namespaced(name), "{name} is a namespaced name");
    }

    for name in [
        "",
        "render",
        "Vendor.example/render",
        "vendor..example/render",
        "vendor.example/",
        "vendor.example/ /render",
        "vendor.example/render ",
    ] {
        assert!(
            !is_namespaced(name),
            "{name} is not namespaced, so admitting it would let one writer claim a bare name"
        );
    }

    // The bound is the one a host applies too, so nothing admitted here can be
    // refused later for its length alone.
    assert!(is_namespaced(&format!("vendor.{}/render", "x".repeat(20))));
    assert!(!is_namespaced(&format!(
        "vendor.{}/render",
        "x".repeat(150)
    )));
    assert!(format!("vendor.{}/render", "x".repeat(150)).len() > MAX_NAMESPACED_NAME_BYTES);
}

#[test]
fn versions_are_published_versions_only() {
    for version in ["1.4.0", "0.0.1", "12.3.4-rc.1"] {
        assert!(is_semver(version), "{version} is a published version");
    }
    for version in ["", "1.4", "1.4.0.1", "v1.4.0", "1.4.0-", "latest", "1.x.0"] {
        assert!(!is_semver(version), "{version} is not a published version");
    }
}

// ---------------------------------------------------------------------------
// Contract version negotiation
// ---------------------------------------------------------------------------

#[test]
fn minor_negotiation_refuses_only_what_it_cannot_serve() {
    let extension = ContractRange {
        major: 1,
        minimum_minor: 3,
    };

    assert_eq!(
        extension.negotiate(ContractRange {
            major: 1,
            minimum_minor: 3
        }),
        ContractCompatibility::Compatible
    );
    // A host with a *newer* minor is compatible: additive minor fields are
    // ignored rather than fatal.
    assert_eq!(
        extension.negotiate(ContractRange {
            major: 1,
            minimum_minor: 9
        }),
        ContractCompatibility::Compatible
    );

    let older = extension.negotiate(ContractRange {
        major: 1,
        minimum_minor: 2,
    });
    assert_eq!(older, ContractCompatibility::RequiresNewerMinor);
    let other_major = extension.negotiate(ContractRange {
        major: 2,
        minimum_minor: 3,
    });
    assert_eq!(other_major, ContractCompatibility::MajorMismatch);
    assert!(!other_major.is_compatible());

    // Refusals are local and name the plugin, so the caller can install a
    // version that fits instead of being told its request was malformed.
    assert!(
        ContractCompatibility::Compatible
            .refusal("vendor.example/render")
            .is_none()
    );
    for refusal in [older, other_major] {
        let failure = refusal.refusal("vendor.example/render").expect("a refusal");
        assert_eq!(failure.stage, "extension/negotiate");
        assert_eq!(failure.field.as_deref(), Some("supportedContractRange"));
        assert_eq!(failure.recovery, RecoveryAction::InstallOrRetryRuntime);
        assert!(!failure.retryable);
        assert_eq!(
            failure.presentation_args.get("pluginId"),
            Some("vendor.example/render")
        );
    }
}

#[test]
fn a_negotiated_range_carries_no_identity_and_no_authority() {
    // A payload may carry whatever it likes next to a range; none of it becomes
    // part of the negotiated value, so a version exchange cannot grant identity
    // or permission.
    let hostile = json!({
        "major": 1,
        "minimumMinor": 0,
        "principal": "mallory",
        "effectId": "effect:forged",
        "authorized": true,
        "stateRoot": "/somewhere/else"
    });
    let range: ContractRange = serde_json::from_value(hostile).expect("a range");
    assert_eq!(
        range,
        ContractRange {
            major: 1,
            minimum_minor: 0
        }
    );
    assert_eq!(
        serde_json::to_value(range).expect("wire"),
        json!({"major": 1, "minimumMinor": 0})
    );
}

#[test]
fn the_descriptor_publishes_the_contract_fields_and_no_authority_ones() {
    let descriptor = descriptor();
    descriptor
        .validate()
        .expect("the descriptor is well formed");

    let wire = serde_json::to_value(&descriptor).expect("wire");
    let keys: BTreeSet<&str> = wire
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        BTreeSet::from([
            "pluginId",
            "implementationVersion",
            "supportedContractRange",
            "capabilities",
            "requiredGrants",
            "quotaShape",
            "lifecycle",
            "attributes",
        ]),
        "a descriptor records exactly what the contract names, so it has nowhere \
         to carry an authority field"
    );
    for field in AUTHORITY_FIELDS {
        assert!(
            !wire.as_object().expect("object").contains_key(field),
            "{field} is not a descriptor field"
        );
    }
}

// ---------------------------------------------------------------------------
// Discovered versus required capabilities
// ---------------------------------------------------------------------------

#[test]
fn an_unknown_optional_attribute_is_preserved_and_never_bound() {
    let discovered = DiscoveredCapabilities::new(["vendor.example/render"]);
    let adopted = discovered
        .admit(&[
            DeclaredAttribute::optional("vendor.example/render").with_value(json!("yes")),
            DeclaredAttribute::optional("vendor.unknown/decorate").with_value(json!("kept")),
        ])
        .expect("unknown optional attributes are not fatal");

    assert_eq!(
        adopted.bound_value("vendor.example/render"),
        Some(&json!("yes"))
    );
    assert!(
        adopted.bound_value("vendor.unknown/decorate").is_none(),
        "the host must not be able to rule on an attribute it does not understand"
    );
    assert!(adopted.was_preserved("vendor.unknown/decorate"));
    assert_eq!(
        adopted.preserved().get("vendor.unknown/decorate"),
        Some(&json!("kept")),
        "preserved attributes are kept verbatim for the caller"
    );
}

#[test]
fn a_required_capability_is_refused_locally_and_only_that_call() {
    let discovered = DiscoveredCapabilities::new(["vendor.example/render"]);
    let failure = discovered
        .admit(&[DeclaredAttribute::required("vendor.missing/analyse")])
        .expect_err("an unsupported required capability is refused");
    assert_eq!(failure.code, "capability_required_unsupported");
    assert_eq!(failure.stage, "capability/admit");
    assert_eq!(failure.field.as_deref(), Some("vendor.missing/analyse"));
    assert_eq!(failure.effect, EffectCertainty::NotAttempted);
    assert_eq!(failure.recovery, RecoveryAction::InstallOrRetryRuntime);

    // The refusal is local: the catalog still serves everything else, and the
    // next call — which needs only what exists — is admitted.
    assert!(discovered.supports("vendor.example/render"));
    assert!(
        discovered
            .admit(&[
                DeclaredAttribute::required("vendor.example/render"),
                DeclaredAttribute::optional("vendor.unknown/decorate"),
            ])
            .is_ok()
    );

    // A descriptor that requires what the host cannot serve is refused as an
    // extension, and its neighbours are untouched.
    let mut requiring = descriptor();
    requiring.attributes = vec![DeclaredAttribute::required("vendor.missing/analyse")];
    let failure = requiring
        .admit(&discovered)
        .expect_err("the extension cannot run as it stands");
    assert_eq!(failure.code, "capability_required_unsupported");
    assert!(descriptor().admit(&discovered).is_ok());
}

#[test]
fn the_extension_attributes_are_admitted_without_reading_its_grants() {
    let discovered = DiscoveredCapabilities::new(["vendor.example/render"]);
    let mut ungranted = descriptor();
    ungranted.required_grants = vec!["licoup.permission.everything".into()];
    ungranted
        .validate()
        .expect("a grant is a request, not a shape error");
    ungranted
        .admit(&discovered)
        .expect("whether a grant is granted is the host's decision, not this data");
}

// ---------------------------------------------------------------------------
// Authority fields are not writable through free-form attributes
// ---------------------------------------------------------------------------

#[test]
fn free_form_attributes_cannot_write_an_authority_field() {
    let discovered = DiscoveredCapabilities::new(["principal", "vendor.example/render"]);

    for name in [
        "principal",
        "effectId",
        "authorized",
        "stateRoot",
        "effect_id",
        "state_root",
        "PRINCIPAL",
    ] {
        assert!(
            is_authority_field(name),
            "{name} names an authority field in some spelling"
        );
        let failure = discovered
            .admit(&[DeclaredAttribute::optional(name).with_value(json!("forged"))])
            .expect_err("an authority field is refused rather than preserved");
        assert_eq!(failure.code, "capability_authority_field_reserved");
    }

    // Not even a host that "discovered" such a capability can bind one.
    for name in AUTHORITY_FIELDS {
        assert!(!is_namespaced(name));
    }

    // A descriptor that declares one is refused outright.
    let mut hostile = descriptor();
    hostile.attributes = vec![DeclaredAttribute::optional("authorized").with_value(json!(true))];
    assert_eq!(
        hostile.validate().expect_err("refused").code,
        "extension_descriptor_invalid"
    );

    // An unnamespaced attribute is refused too: it would be a second, competing
    // namespace next to the core's own field names.
    assert_eq!(
        discovered
            .admit(&[DeclaredAttribute::optional("render")])
            .expect_err("refused")
            .code,
        "capability_attribute_invalid"
    );
    // And the same name twice is ambiguous, so it is refused rather than
    // resolved by position.
    assert_eq!(
        discovered
            .admit(&[
                DeclaredAttribute::optional("vendor.example/render"),
                DeclaredAttribute::required("vendor.example/render"),
            ])
            .expect_err("refused")
            .code,
        "capability_attribute_duplicate"
    );
}

#[test]
fn tool_arguments_cannot_supply_the_authority_an_invocation_runs_as() {
    let arguments = json!({
        "family": "subagent",
        "command": "list",
        "principal": "mallory",
        "effectId": "effect:forged",
        "authorized": true,
        "stateRoot": "/elsewhere",
        "authority": {"kind": "local-admin", "ownerMembershipId": "membership:mallory"}
    });
    let decoded = ApplicationCommand::decode(&arguments).expect("the arguments decode");
    assert_eq!(
        decoded,
        ApplicationCommand::Subagent(SubagentCommand::List),
        "every authority field in the arguments is ignored, not applied"
    );

    let invocation = invocation(InvocationScope::installation());
    assert_eq!(
        invocation.authority.source(),
        AuthoritySource::VerifiedTransport
    );
    assert_eq!(
        invocation.authority.claim(),
        &ActorClaim::membership("codex", "conversation:one", "membership:codex"),
        "the authority is the one the trusted source proved, not the one the input names"
    );
}

// ---------------------------------------------------------------------------
// The failure chain
// ---------------------------------------------------------------------------

#[test]
fn a_failure_keeps_its_identity_and_its_public_arguments() {
    let failure =
        ApplicationFailure::permanent("capability_required_unsupported", "capability/admit")
            .with_component("extension_catalog")
            .with_field("vendor.missing/analyse")
            .with_presentation_arg("capability", "vendor.missing/analyse")
            .with_effect(EffectCertainty::Uncertain);

    let wire = serde_json::to_value(&failure).expect("wire");
    let restored: ApplicationFailure = serde_json::from_value(wire).expect("it round trips");
    assert_eq!(restored, failure);
    assert_eq!(restored.code, "capability_required_unsupported");
    assert_eq!(restored.stage, "capability/admit");
    assert_eq!(restored.component.as_ref(), "extension_catalog");
    assert!(restored.retryable);
    assert_eq!(restored.effect, EffectCertainty::Uncertain);
    assert_eq!(restored.recovery, RecoveryAction::ReconcileBeforeRetry);
    assert!(restored.requires_reconciliation());
    assert!(!restored.safe_to_retry());
    assert_eq!(
        restored.presentation_args.get("capability"),
        Some("vendor.missing/analyse")
    );
}

#[test]
fn technical_text_and_paths_are_dropped_rather_than_carried() {
    // A producer that leaked its technical cause keeps its failure: the extra
    // fields are not part of the contract and do not survive, and the arguments
    // that are not public are dropped.
    //
    // The home path below is assembled from parts rather than written out, so
    // this source file carries no machine path of its own. The assertions about
    // it are exactly as strong: if the redaction stopped working, the path would
    // reach `wire` and both checks would fail.
    let home = ["", "Users", "private-owner", "secrets"].join("/");
    let leaked_file = format!("{home}/plugin.wasm");
    let leaked_cause = format!("no such file or directory: {leaked_file}");
    let leaked_raw = format!("{{\"plugin\":\"{leaked_file}\"}}");
    let leaked = json!({
        "code": "capability_required_unsupported",
        "stage": "capability/admit",
        "component": "extension_catalog",
        "retryable": false,
        "effect": "not-attempted",
        "recovery": "install-or-retry-runtime",
        "field": "vendor.missing/analyse",
        "message": leaked_cause,
        "path": home,
        "stack": "at extension::host::load (host.rs:412)",
        "presentationArgs": {
            "capability": "vendor.missing/analyse",
            "path": leaked_file,
            "message": "no such file or directory",
            "detail": "at host::load",
            "stderr": "panic at host.rs:412",
            "raw": leaked_raw
        }
    });
    let failure: ApplicationFailure = serde_json::from_value(leaked).expect("the failure survives");

    assert_eq!(failure.code, "capability_required_unsupported");
    assert_eq!(failure.stage, "capability/admit");
    assert_eq!(failure.component.as_ref(), "extension_catalog");
    assert_eq!(failure.recovery, RecoveryAction::InstallOrRetryRuntime);
    assert_eq!(failure.presentation_args.len(), 1);
    assert_eq!(
        failure.presentation_args.get("capability"),
        Some("vendor.missing/analyse")
    );

    let wire = serde_json::to_value(&failure).expect("wire");
    let body = wire.as_object().expect("object");
    for leaked in ["message", "path", "stack", "detail", "stderr", "raw"] {
        assert!(
            !body.contains_key(leaked),
            "a failure may not carry {leaked}: there is no slot for technical text or a path"
        );
    }
    let rendered = wire.to_string();
    assert!(
        !rendered.contains(home.as_str()) && !rendered.contains(leaked_file.as_str()),
        "not even inside an argument: {wire}"
    );

    // The bounds are the client bridge's own, so both interfaces bound the same
    // way instead of each inventing a limit.
    let mut args = PresentationArgs::new();
    assert!(!args.insert("path", leaked_file.as_str()));
    assert!(!args.insert("message", "boom"));
    assert!(!args.insert("Path", "short"));
    assert!(!args.insert("k".repeat(33), "short"));
    assert!(!args.insert("field", "line\nbreak"));
    assert!(!args.insert("field", "y".repeat(97)));
    assert!(args.insert("agentLabel", "Fixture Agent"));
    assert!(args.insert("runtimeLabel", "Fixture Runtime"));
    assert!(args.insert("sequence", "7"));
    assert!(args.insert("resultKind", "terminal"));
    assert!(
        !args.insert("title", "one too many"),
        "four entries at most, as the client bridge publishes"
    );
    assert_eq!(args.len(), MAX_PRESENTATION_ARGS);
    assert!(PresentationArgs::new().is_empty());
}

#[test]
fn only_a_payload_that_is_not_json_is_a_format_error() {
    // Malformed JSON is the one cause with the `provide_valid_json` recovery.
    let malformed = ApplicationCommand::decode_text("{\"family\": \"subagent\"")
        .expect_err("malformed JSON is refused");
    assert_eq!(malformed.code, "invalid_json");
    assert_eq!(malformed.stage, "schema/decode");
    assert_eq!(malformed.recovery, RecoveryAction::ProvideValidJson);
    assert_eq!(malformed.field.as_deref(), Some("command"));

    // A well-formed payload of the wrong shape names its field and asks the
    // caller to correct the request, not to rewrite the JSON.
    let wrong_shape =
        ApplicationCommand::decode_text("{\"family\": \"subagent\"}").expect_err("refused");
    assert_eq!(wrong_shape.code, "invalid_request");
    assert_eq!(wrong_shape.stage, "schema/validate");
    assert_eq!(wrong_shape.recovery, RecoveryAction::CorrectRequest);

    // A capability that is not installed, a refused claim and a failed port all
    // keep their own code and recovery: none of them is flattened into bad
    // format, and none asks the caller to fix the encoding.
    let refused = DiscoveredCapabilities::default()
        .admit(&[DeclaredAttribute::required("vendor.missing/analyse")])
        .expect_err("refused");
    assert_ne!(refused.code, wrong_shape.code);
    assert_ne!(refused.stage, wrong_shape.stage);
    assert_ne!(refused.recovery, RecoveryAction::ProvideValidJson);
}

// ---------------------------------------------------------------------------
// The invocation envelope
// ---------------------------------------------------------------------------

#[test]
fn an_invocation_validates_its_own_shape_before_any_port_runs() {
    let valid = invocation(
        InvocationScope::conversation("conversation:one").with_membership("membership:codex"),
    )
    .with_expected_revision(7)
    .with_idempotency("idem-1");
    valid.validate().expect("a complete invocation");

    let no_request_id = ToolInvocation::new(
        "  ",
        Operation::SubagentsList,
        InvocationScope::installation(),
        valid.authority.clone(),
    );
    assert_eq!(
        no_request_id
            .validate()
            .expect_err("refused")
            .field
            .as_deref(),
        Some("request_id")
    );

    let zero_revision = valid.clone().with_expected_revision(0);
    assert_eq!(
        zero_revision
            .validate()
            .expect_err("refused")
            .field
            .as_deref(),
        Some("expected_revision")
    );

    let bad_scope = invocation(InvocationScope::conversation("   "));
    assert_eq!(
        bad_scope.validate().expect_err("refused").field.as_deref(),
        Some("conversation_id")
    );

    let bad_claim = ToolInvocation::new(
        "request:one",
        Operation::SubagentsList,
        InvocationScope::installation(),
        AuthorityHandle::from_verified_source(
            ActorClaim::membership("Not A Provider", "conversation:one", "membership:codex"),
            AuthoritySource::AdmittedAdapter,
        ),
    );
    assert_eq!(
        bad_claim.validate().expect_err("refused").code,
        "actor_provider_invalid"
    );
    assert!(InvocationScope::installation().is_installation_wide());
    assert!(!InvocationScope::conversation("conversation:one").is_installation_wide());
}

#[test]
fn a_stale_revision_is_refused_against_the_revision_it_named() {
    let fenced = invocation(InvocationScope::installation()).with_expected_revision(7);
    fenced
        .fence(7)
        .expect("the caller saw the current revision");
    invocation(InvocationScope::installation())
        .fence(9)
        .expect("an invocation that named no revision is not fenced");

    let failure = fenced.fence(9).expect_err("the state moved on");
    assert_eq!(failure.code, "stale_revision");
    assert_eq!(failure.stage, "invocation/fence");
    assert_eq!(failure.effect, EffectCertainty::NotAttempted);
    assert_eq!(failure.recovery, RecoveryAction::RetryOrReviewRequest);
    assert!(!failure.requires_reconciliation());
    assert_eq!(failure.presentation_args.get("expectedRevision"), Some("7"));
    assert_eq!(failure.presentation_args.get("currentRevision"), Some("9"));
}

#[test]
fn a_replay_key_returns_the_reference_it_already_produced() {
    let mut replaying = invocation(InvocationScope::installation()).with_idempotency("idem-1");
    replaying.operation = Operation::SubagentDelegate;
    let answered = OperationReference::new(
        Operation::SubagentDelegate,
        "dispatch:one",
        OperationState::Accepted,
    )
    .with_idempotency_key("idem-1");
    let other = OperationReference::new(
        Operation::SubagentDelegate,
        "dispatch:two",
        OperationState::Accepted,
    )
    .with_idempotency_key("idem-2");

    assert!(
        replaying.replays(&answered),
        "the same key is the same operation, so it must not start a second effect"
    );
    assert!(!replaying.replays(&other));
    let mut unrelated = answered.clone();
    unrelated.operation = Operation::ConversationList.as_str().into();
    assert!(
        !replaying.replays(&unrelated),
        "keys are not global identities"
    );
    unrelated = answered.clone().with_conversation("conversation:other");
    assert!(!replaying.replays(&unrelated));
    unrelated = answered.clone().with_membership("membership:other");
    assert!(!replaying.replays(&unrelated));
    let mut scoped = replaying.clone();
    scoped.scope =
        InvocationScope::conversation("conversation:one").with_membership("membership:one");
    let scoped_answer = answered
        .clone()
        .with_conversation("conversation:one")
        .with_membership("membership:one");
    assert!(scoped.replays(&scoped_answer));
    assert!(!replaying.replays(&scoped_answer));
    assert!(
        !invocation(InvocationScope::installation()).replays(&answered),
        "an invocation without a key has nothing to replay"
    );
    assert_eq!(
        replaying.idempotency.as_ref().map(|key| key.key()),
        Some("idem-1")
    );
}

// ---------------------------------------------------------------------------
// Receipts, and what is not a receipt
// ---------------------------------------------------------------------------

#[test]
fn a_receipt_reports_what_the_tool_observed() {
    let reference = OperationReference::new(
        Operation::SubagentDelegate,
        "dispatch:one",
        OperationState::Processing,
    )
    .with_conversation("conversation:one")
    .with_idempotency_key("idem-1");

    let admitted = ToolReceipt::admitted("request:one", reference.clone());
    assert_eq!(admitted.kind.as_str(), "admitted");
    assert_eq!(admitted.state(), OperationState::Accepted);
    assert!(admitted.state().is_live());
    assert!(admitted.failure().is_none());
    assert_eq!(admitted.operation, "subagent.delegate");
    assert_eq!(admitted.request_id, "request:one");

    let completed = ToolReceipt::completed("request:two", reference.clone(), json!({"ok": true}));
    assert_eq!(completed.kind, ReceiptKind::Completed);
    assert_eq!(completed.state(), OperationState::Completed);
    assert_eq!(completed.payload, json!({"ok": true}));

    let read = ToolReceipt::read("request:three", Operation::ConversationList, json!([]));
    assert_eq!(read.operation, "conversation.list");
    assert!(!read.state().is_live(), "a read has no identity to follow");

    // An unknown effect is not a completion, and it never reports a blind retry
    // as the next step.
    let unknown = ToolReceipt::unknown(
        "request:four",
        reference,
        "extension_effect_unknown",
        "extension/execute",
    );
    assert_eq!(unknown.kind, ReceiptKind::Unknown);
    assert_eq!(unknown.state(), OperationState::ReconciliationRequired);
    let failure = unknown
        .failure()
        .expect("an unknown effect carries its cause");
    assert_eq!(failure.effect, EffectCertainty::Uncertain);
    assert_eq!(failure.recovery, RecoveryAction::ReconcileBeforeRetry);
    assert!(failure.requires_reconciliation());
    assert!(!failure.safe_to_retry());

    // Kind and reference state are one fact, so a receipt cannot report two.
    for receipt in [admitted, completed, read, unknown] {
        assert_eq!(receipt.state(), receipt.kind.state());
        let wire = serde_json::to_value(&receipt).expect("wire");
        let restored: ToolReceipt = serde_json::from_value(wire).expect("round trips");
        assert_eq!(restored, receipt);
    }
}

#[test]
fn natural_output_is_carried_verbatim_and_never_parsed() {
    // A reply that looks like a broken envelope is still a reply: the fault
    // belongs to the machine interface that produced the envelope, not to the
    // text an agent wrote.
    let body = "here is the JSON you asked about: {\"tool_receipt\": ";
    let reply = NaturalOutput::new(body);
    assert_eq!(reply.text(), body);
    assert!(!reply.is_empty());
    assert_eq!(
        serde_json::to_value(&reply).expect("wire"),
        json!({"text": body})
    );

    // The same bytes as tool arguments are refused, because that is a machine
    // interface with a schema.
    let refused = ApplicationCommand::decode_text(body).expect_err("not a command");
    assert_eq!(refused.code, "invalid_json");

    // Nothing about a natural reply is required: no envelope, no fields, and a
    // reply in another language or an empty one is still valid output.
    for text in ["", "完毕", "{\"kind\": \"unknown\"}", "\n"] {
        assert_eq!(NaturalOutput::new(text).text(), text);
    }
}

#[test]
fn an_extension_that_cannot_fit_is_refused_without_a_format_error() {
    let published = descriptor();
    assert_eq!(
        published.compatible_with(ContractRange {
            major: 1,
            minimum_minor: 3
        }),
        ContractCompatibility::Compatible
    );
    let failure = published
        .compatible_with(ContractRange {
            major: 2,
            minimum_minor: 0,
        })
        .refusal("vendor.example/render")
        .expect("a refusal");
    assert_eq!(failure.code, "extension_contract_major_mismatch");
    assert_ne!(failure.code, "invalid_json");
    assert_ne!(failure.recovery, RecoveryAction::ProvideValidJson);

    let mut unnamed = descriptor();
    unnamed.plugin_id = "render".into();
    assert_eq!(
        unnamed.validate().expect_err("refused").field.as_deref(),
        Some("pluginId")
    );
    assert!(matches!(
        published.attributes[0].requirement,
        Requirement::Optional
    ));
    assert!(matches!(
        published.attributes[1].requirement,
        Requirement::Required
    ));

    let wire: Value = serde_json::to_value(published.lifecycle).expect("wire");
    assert_eq!(wire["activation"], json!("explicit"));
}

#[test]
fn recovery_cannot_turn_reconciliation_into_a_blind_retry() {
    let uncertain = ApplicationFailure::uncertain("effect_unknown", "extension/execute")
        .with_recovery(RecoveryAction::RetryAfterRecovery);
    assert_eq!(uncertain.recovery, RecoveryAction::ReconcileBeforeRetry);
    assert!(!uncertain.safe_to_retry());

    let reconcile = ApplicationFailure::retryable("effect_pending", "extension/observe")
        .with_recovery(RecoveryAction::ReconcileBeforeRetry);
    assert!(!reconcile.safe_to_retry());

    let failure = licoup_application::FailureNormalization {
        retryable: false,
        uncertain_effect: true,
    }
    .into_failure("effect_unknown", "extension/execute");
    assert!(
        failure.retryable,
        "retry is possible only after reconciliation"
    );
    assert!(!failure.safe_to_retry());
}

#[test]
fn decoding_an_uncertain_failure_keeps_the_cause_but_requires_reconciliation() {
    let wire = json!({
        "code": "effect_unknown", "stage": "extension/execute",
        "retryable": false, "effect": "uncertain", "recovery": "retry-after-recovery"
    });
    let failure: ApplicationFailure = serde_json::from_value(wire).unwrap();
    assert_eq!(failure.code, "effect_unknown");
    assert_eq!(failure.recovery, RecoveryAction::ReconcileBeforeRetry);
    assert!(failure.retryable);
    assert!(!failure.safe_to_retry());
}

#[test]
fn malformed_presentation_arguments_do_not_erase_valid_public_arguments() {
    let args: PresentationArgs = serde_json::from_value(json!({
        "agentLabel": "Fixture Agent", "bad": {"nested": true}, "sequence": 7
    }))
    .unwrap();
    assert_eq!(args.get("agentLabel"), Some("Fixture Agent"));
    assert_eq!(args.len(), 1);
    let mut full = PresentationArgs::new();
    for key in ["agentLabel", "runtimeLabel", "sequence", "resultKind"] {
        assert!(full.insert(key, "before"));
    }
    assert!(full.insert("agentLabel", "after"));
    assert_eq!(full.get("agentLabel"), Some("after"));
    assert_eq!(full.len(), MAX_PRESENTATION_ARGS);
    for invalid in [
        json!(["private", "text"]),
        json!(7),
        json!(null),
        json!(true),
    ] {
        let wire = json!({
            "code": "effect_unknown", "stage": "extension/execute",
            "retryable": true, "effect": "uncertain", "recovery": "reconcile-before-retry",
            "presentationArgs": invalid
        });
        // Real interfaces decode bytes, not an already parsed Value. Consume
        // the entire bad argument value so the real failure still decodes.
        let failure: ApplicationFailure = serde_json::from_str(&wire.to_string()).unwrap();
        assert_eq!(failure.code, "effect_unknown");
        assert!(failure.presentation_args.is_empty());
    }
}

#[test]
fn invalid_attribute_names_do_not_echo_raw_input_into_the_failure_chain() {
    let private_path = ["", "Users", "fixture-owner", "private"].join("/");
    for name in [private_path.as_str(), "raw\ntechnical\ntext"] {
        let failure = DiscoveredCapabilities::default()
            .admit(&[DeclaredAttribute::optional(name)])
            .unwrap_err();
        assert_eq!(failure.code, "capability_attribute_invalid");
        assert_eq!(failure.field.as_deref(), Some("attributes"));
        assert!(failure.presentation_args.is_empty());
        assert!(!serde_json::to_string(&failure).unwrap().contains(name));
    }
}

#[test]
fn contradictory_machine_receipts_are_refused_without_touching_natural_output() {
    let receipt = ToolReceipt::unknown(
        "request:one",
        OperationReference::new(
            Operation::SubagentDelegate,
            "dispatch:one",
            OperationState::Processing,
        ),
        "effect_unknown",
        "extension/execute",
    );
    let valid = serde_json::to_value(&receipt).unwrap();
    let mut variants = Vec::new();
    let mut wire = valid.clone();
    wire["reference"]["state"] = json!("completed");
    variants.push(wire);
    let mut wire = valid.clone();
    wire["operation"] = json!("conversation.list");
    variants.push(wire);
    let mut wire = valid.clone();
    wire["failure"] = Value::Null;
    variants.push(wire);
    let mut wire = valid.clone();
    wire["failure"]["effect"] = json!("not-attempted");
    variants.push(wire);
    let mut wire = valid.clone();
    wire["kind"] = json!("completed");
    wire["reference"]["state"] = json!("completed");
    variants.push(wire);
    for wire in variants {
        let text = wire.to_string();
        assert!(serde_json::from_value::<ToolReceipt>(wire).is_err());
        assert_eq!(NaturalOutput::new(&text).text(), text);
    }
}
