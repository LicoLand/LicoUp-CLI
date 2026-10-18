//! A18 at `component-integration`: capability discovery through the native
//! catalog, namespaced extensions, and the error chain.
//!
//! The catalog under test is the production one, and the carrier is a real
//! implementation of the production port. The extension identities are made up
//! on purpose: a vendor this repository has never heard of must be discoverable
//! without regenerating anything.

use licoup_application::{
    ContractRange, EffectCertainty, OperationState, ReceiptKind, RecoveryAction,
};
use licoup_extension_contracts::profile::DeclaredMethods;
use licoup_native::platform::extension_host::{
    CatalogProfileStatus, ExtensionHost, InvocationOutcome,
};
use serde_json::json;

use crate::support::{
    ControlledCarrier, DispatchReply, activate, host as build_host, stage_request,
    stage_request_with_attributes,
};

#[test]
fn namespaced_extensions_are_discovered_through_the_native_catalog() {
    let carrier = ControlledCarrier::new("alpha");
    let host = build_host(carrier);
    let receipt = activate(&host, "acme.tools/ext", &["acme.tools/render"]).expect("activate");

    let snapshot = host.catalog();
    assert_eq!(snapshot.epoch().get(), 1);
    assert!(snapshot.serves("acme.tools/render"));
    assert!(snapshot.discovered().supports("acme.tools/render"));
    assert_eq!(
        snapshot
            .route("acme.tools/render")
            .map(|entry| entry.instance_id()),
        Some(receipt.instance_id.as_str())
    );

    let admitted = host
        .begin("acme.tools/render", &json!({"prompt": "draw"}))
        .expect("admit");
    assert_eq!(admitted.binding.generation(), receipt.generation);
    assert_eq!(admitted.binding.registry_epoch(), receipt.registry_epoch);
    assert_eq!(admitted.binding.instance_id(), receipt.instance_id);
    assert_eq!(admitted.binding.profile(), Some("agent-execution"));

    // The same catalog value is what every surface would read; the document is
    // its serializable projection and carries the same epoch.
    let document = snapshot.document();
    assert_eq!(document.epoch, snapshot.epoch());
    assert_eq!(document.extensions.len(), 1);
    assert_eq!(
        document.extensions[0].capabilities,
        vec!["acme.tools/render".to_owned()]
    );
}

#[test]
fn an_unknown_vendor_and_an_unpublished_profile_need_no_code_change() {
    let carrier = ControlledCarrier::new("beta");
    let host = build_host(carrier);
    let staged = host
        .stage(stage_request(
            "never.seen-before/widget",
            "0.0.1",
            &["never.seen-before/do"],
            &[
                ("future-profile", &["never.seen-before/do"]),
                ("declarative-ui", &[]),
            ],
        ))
        .expect("stage");
    let receipt = host
        .activate(host.prepare(staged).expect("prepare"))
        .expect("activate");

    let snapshot = host.catalog();
    let entry = snapshot.entry(&receipt.instance_id).expect("entry");
    assert!(entry.profiles.iter().any(|profile| {
        profile.id == "future-profile" && profile.status == CatalogProfileStatus::Unpublished
    }));
    assert!(entry.profiles.iter().any(|profile| {
        profile.id == "declarative-ui" && profile.status == CatalogProfileStatus::Available
    }));
    // The declaration is preserved; the capability the extension serves under a
    // published profile is discoverable.
    assert!(snapshot.serves("never.seen-before/do"));
}

#[test]
fn a_missing_capability_refuses_only_that_call() {
    let carrier = ControlledCarrier::new("gamma");
    let host = build_host(carrier);
    activate(&host, "acme.tools/ext", &["acme.tools/render"]).expect("activate");

    let failure = host
        .begin("acme.tools/absent", &json!({}))
        .expect_err("unserved capability is refused");
    assert_eq!(failure.code, "extension_capability_unavailable");
    assert_eq!(&*failure.component, ExtensionHost::component());
    assert_eq!(failure.stage, "extension/admit");
    assert_eq!(failure.field.as_deref(), Some("acme.tools/absent"));
    assert_eq!(failure.recovery, RecoveryAction::InstallOrRetryRuntime);
    assert_eq!(
        failure.presentation_args.get("reason"),
        Some("not-installed")
    );
    assert_eq!(failure.effect, EffectCertainty::NotAttempted);

    // The rest of the catalog is untouched by that refusal.
    let admitted = host
        .begin("acme.tools/render", &json!({}))
        .expect("the served capability still admits");
    assert_eq!(admitted.outcome, InvocationOutcome::Admitted);
}

#[test]
fn unknown_optional_attributes_are_preserved_and_never_acted_on() {
    let carrier = ControlledCarrier::new("delta");
    let host = build_host(carrier);
    let staged = host
        .stage(stage_request_with_attributes(
            "acme.attr/ext",
            &["acme.attr/run"],
            &[("acme.future/flag", false)],
        ))
        .expect("stage");
    assert_eq!(staged.staged_epoch().get(), 0);
    assert_eq!(staged.staged_capabilities(), ["acme.attr/run".to_owned()]);
    assert!(
        staged
            .adopted_attributes()
            .preserved()
            .contains_key("acme.future/flag")
    );
    assert!(staged.adopted_attributes().bound().is_empty());

    let receipt = host
        .activate(host.prepare(staged).expect("prepare"))
        .expect("activate");
    let entry = host
        .catalog()
        .entry(&receipt.instance_id)
        .cloned()
        .expect("entry");
    assert!(entry.preserved_attributes.contains_key("acme.future/flag"));
    assert!(!entry.bound_attributes.contains_key("acme.future/flag"));
    assert!(
        entry
            .attributes
            .iter()
            .any(|attribute| attribute.name == "acme.future/flag")
    );

    // The preserved attribute survives the surface projection verbatim.
    let encoded = serde_json::to_value(host.catalog().document()).expect("document");
    assert!(encoded.to_string().contains("acme.future/flag"));
}

#[test]
fn required_unknown_attributes_and_authority_fields_are_refused() {
    let carrier = ControlledCarrier::new("epsilon");
    let host = build_host(carrier);

    let failure = host
        .stage(stage_request_with_attributes(
            "acme.attr/ext",
            &["acme.attr/run"],
            &[("acme.future/must", true)],
        ))
        .expect_err("a required unknown attribute refuses this extension");
    assert_eq!(failure.code, "capability_required_unsupported");
    assert_eq!(&*failure.component, "extension_catalog");
    assert_eq!(failure.recovery, RecoveryAction::InstallOrRetryRuntime);

    let failure = host
        .stage(stage_request_with_attributes(
            "acme.attr/ext",
            &["acme.attr/run"],
            &[("principal", false)],
        ))
        .expect_err("an authority field cannot be claimed");
    // The descriptor boundary refuses the declaration structurally, and the
    // shared admission rule refuses the borrowed authority name by itself.
    assert_eq!(failure.code, "extension_descriptor_invalid");
    let borrowed = licoup_application::DiscoveredCapabilities::new(Vec::<String>::new())
        .admit(&[licoup_application::DeclaredAttribute::optional("principal")])
        .expect_err("the shared rule refuses the name without the descriptor check");
    assert_eq!(borrowed.code, "capability_authority_field_reserved");

    let failure = host
        .stage(stage_request_with_attributes(
            "acme.attr/ext",
            &["acme.attr/run"],
            &[("notnamespaced", false)],
        ))
        .expect_err("a bare word is not an extension attribute");
    // A bare word is refused structurally rather than as an extension
    // attribute: it never reaches the admission rule.
    assert_eq!(failure.code, "extension_descriptor_invalid");
    let bare = licoup_application::DiscoveredCapabilities::new(Vec::<String>::new())
        .admit(&[licoup_application::DeclaredAttribute::optional("bareword")])
        .expect_err("the shared rule refuses a non-namespaced attribute");
    assert_eq!(bare.code, "capability_attribute_invalid");
}

#[test]
fn a_contract_major_mismatch_refuses_only_that_extension() {
    let carrier = ControlledCarrier::new("zeta");
    let host = build_host(carrier);
    activate(&host, "acme.one/ext", &["acme.one/run"]).expect("activate");

    let mut request = stage_request(
        "acme.two/ext",
        "1.0.0",
        &["acme.two/run"],
        &[("agent-execution", &["acme.two/run"])],
    );
    request.descriptor.supported_contract_range = ContractRange {
        major: 2,
        minimum_minor: 0,
    };
    let failure = host
        .stage(request)
        .expect_err("major mismatch refuses the extension");
    assert_eq!(failure.code, "extension_contract_major_mismatch");
    assert_eq!(failure.recovery, RecoveryAction::InstallOrRetryRuntime);

    // The first extension still answers; the refusal was local.
    assert!(host.catalog().serves("acme.one/run"));
}

#[test]
fn a_profile_in_a_different_major_is_kept_and_serves_nothing() {
    let carrier = ControlledCarrier::new("eta");
    let host = build_host(carrier);
    let mut request = stage_request(
        "acme.mixed/ext",
        "1.0.0",
        &["acme.mixed/run"],
        &[("agent-execution", &["acme.mixed/run"])],
    );
    request.profiles = vec![
        licoup_extension_contracts::profile::ProfileDeclaration::new("agent-execution", 2)
            .with_capabilities(["acme.mixed/run"]),
    ];
    let staged = host.stage(request).expect("stage");
    let receipt = host
        .activate(host.prepare(staged).expect("prepare"))
        .expect("activate");
    let entry = host
        .catalog()
        .entry(&receipt.instance_id)
        .cloned()
        .expect("entry");
    assert!(entry.profiles.iter().any(|profile| {
        profile.id == "agent-execution" && profile.status == CatalogProfileStatus::MajorMismatch
    }));
    // A profile the host cannot use contributes no capability.
    assert!(!host.catalog().serves("acme.mixed/run"));
    assert!(host.begin("acme.mixed/run", &json!({})).is_err());
}

#[test]
fn the_live_handshake_decides_the_profile_not_the_manifest_claim() {
    let carrier = ControlledCarrier::new("handshake");
    // The manifest claims the full minimal agent, but the runtime answers with
    // a method set that is missing `agent.event`.
    carrier.with_methods(DeclaredMethods::new([
        "extension.initialize",
        "extension.ready",
        "extension.shutdown",
        "agent.describe",
        "agent.execute",
    ]));
    let host = build_host(carrier.clone());
    let staged = host
        .stage(stage_request(
            "acme.live/ext",
            "1.0.0",
            &["acme.live/run"],
            &[("agent-execution", &["acme.live/run"])],
        ))
        .expect("stage");
    // The claim validated against the published method catalog...
    assert_eq!(staged.profiles()[0].status, CatalogProfileStatus::Available);
    let receipt = host
        .activate(host.prepare(staged).expect("prepare"))
        .expect("activate");
    // ...and the live decision refuses the profile for the missing method.
    let entry = host
        .catalog()
        .entry(&receipt.instance_id)
        .cloned()
        .expect("entry");
    let profile = entry
        .profiles
        .iter()
        .find(|profile| profile.id == "agent-execution")
        .expect("profile");
    assert_eq!(profile.status, CatalogProfileStatus::MissingRequired);
    assert_eq!(profile.missing, vec!["agent.event".to_owned()]);
    assert!(!host.catalog().serves("acme.live/run"));
    assert_eq!(carrier.dispatches(), 0);

    // A runtime that accepts a different profile than the one declared is
    // decided by its own answer too.
    let carrier = ControlledCarrier::new("handshake-other");
    carrier.accepted_profiles(vec!["declarative-ui".to_owned()]);
    let host = build_host(carrier.clone());
    let staged = host
        .stage(stage_request(
            "acme.live/ext",
            "1.0.0",
            &["acme.live/run"],
            &[("agent-execution", &["acme.live/run"])],
        ))
        .expect("stage");
    let receipt = host
        .activate(host.prepare(staged).expect("prepare"))
        .expect("activate");
    let entry = host
        .catalog()
        .entry(&receipt.instance_id)
        .cloned()
        .expect("entry");
    assert!(entry.profiles.iter().any(|profile| {
        profile.id == "agent-execution" && profile.status == CatalogProfileStatus::MissingRequired
    }));
    assert!(!host.catalog().serves("acme.live/run"));
    assert_eq!(carrier.dispatches(), 0);
}

#[test]
fn plain_text_is_carried_verbatim_and_never_forced_into_a_schema() {
    let carrier = ControlledCarrier::new("prose");
    carrier.with_dispatch(DispatchReply::Natural("{\"broken\": ".to_owned()));
    let host = build_host(carrier);
    let receipt = activate(&host, "acme.prose/ext", &["acme.prose/chat"]).expect("activate");

    let admitted = host.begin("acme.prose/chat", &json!(null)).expect("admit");
    match admitted.outcome {
        InvocationOutcome::Natural(output) => {
            assert_eq!(output.text(), "{\"broken\": ");
        }
        other => panic!("expected natural output, got {other:?}"),
    }
    // The call settled as a completed look at natural output; nothing reparsed.
    let report = host.instance_report(&receipt.instance_id).expect("report");
    assert_eq!(report.in_flight, 0);
    assert_eq!(report.unknown, 0);
}

#[test]
fn an_uncertain_effect_reaches_the_shared_reconciliation_vocabulary() {
    let carrier = ControlledCarrier::new("omega");
    carrier.with_dispatch(DispatchReply::Fault(
        licoup_native::platform::extension_host::FaultClass::Unresponsive,
    ));
    let host = build_host(carrier);
    activate(&host, "acme.faulty/ext", &["acme.faulty/run"]).expect("activate");

    let failure = host
        .begin("acme.faulty/run", &json!({"prompt": "work"}))
        .expect_err("an unresponsive plugin is a fault");
    assert_eq!(failure.code, "extension_carrier_unresponsive");
    assert_eq!(&*failure.component, "extension_host");
    assert_eq!(failure.effect, EffectCertainty::Uncertain);
    assert!(failure.requires_reconciliation());
    assert!(!failure.safe_to_retry());

    // The shared machine receipt reports exactly this state; the host does not
    // invent a second vocabulary for it.
    assert_eq!(
        ReceiptKind::Unknown.state(),
        OperationState::ReconciliationRequired
    );
    assert!(ReceiptKind::Unknown.state().is_terminal());
}

#[test]
fn the_failure_chain_survives_the_interface_projection() {
    let carrier = ControlledCarrier::new("theta");
    let host = build_host(carrier);
    let failure = host
        .begin("acme.absent/none", &json!({}))
        .expect_err("nothing serves this capability");
    let encoded = serde_json::to_value(&failure).expect("encode");
    let decoded: licoup_application::ApplicationFailure =
        serde_json::from_value(encoded).expect("decode");
    assert_eq!(decoded, failure);
    assert_eq!(decoded.stage, "extension/admit");
    assert_eq!(
        decoded.presentation_args.get("capability"),
        Some("acme.absent/none")
    );
    assert_eq!(
        decoded.presentation_args.get("reason"),
        Some("not-installed")
    );
}
