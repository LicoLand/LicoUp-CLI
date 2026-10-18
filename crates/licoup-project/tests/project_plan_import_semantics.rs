//! Canonical plan documents: round-trip, source correspondence and refusal.
//!
//! Every case here is synthetic. The sources are documents the test itself
//! wrote, the identities name no real project, and no case reads a real
//! document: the boundary under test converts a caller's deliberate
//! declaration, it is not a parser for anyone's original format.
//!
//! Fixtures are built as JSON values rather than as raw text so that a case
//! needing a field the canonical model does not define can carry it without
//! escaping a document by hand.

use licoup_project::{
    ArtifactReference, IMPORT_STAGE, ImportDiagnostic, MAX_PLAN_ACCEPTANCE, MAX_PLAN_WORK_ITEMS,
    PLAN_DOCUMENT_SCHEMA, PlanDocument, ProjectFailure, RoleId, RoleReference, RoleScope, SourceId,
    SourceKind, SourceLocator, WorkItemId,
};
use serde_json::{Value, json};

/// One declared work item that admits cleanly.
fn work_item(id: &str, anchor: &str) -> Value {
    json!({
        "workItemId": id,
        "outcome": format!("Deliver {id}."),
        "acceptance": ["The declared outcome holds."],
        "inputs": [],
        "roles": [{"roleId": "role:maintainer", "scope": "work-item"}],
        "sourceAnchor": anchor,
    })
}

/// One canonical document over the given work items.
fn document(work_items: Vec<Value>) -> Value {
    json!({
        "schema": PLAN_DOCUMENT_SCHEMA,
        "projectId": "project:alpha",
        "planId": "plan:alpha",
        "source": {
            "sourceId": "source:roadmap",
            "sourceKind": "markdown",
            "locator": "docs/roadmap.md",
        },
        "workItems": work_items,
    })
}

/// One canonical document that admits cleanly, for the cases that need a base.
fn admits() -> PlanDocument {
    PlanDocument::from_value(document(vec![
        work_item("work:read", "heading:Read"),
        work_item("work:write", "heading:Write"),
    ]))
    .expect("the synthetic document is canonical")
}

fn codes(diagnostics: &[ImportDiagnostic]) -> Vec<&'static str> {
    diagnostics.iter().map(|failure| failure.code).collect()
}

fn paths(diagnostics: &[ImportDiagnostic]) -> Vec<&str> {
    diagnostics
        .iter()
        .map(|failure| failure.path.as_str())
        .collect()
}

#[test]
fn a_canonical_document_round_trips_through_the_actual_types() {
    let plan = admits();
    assert_eq!(plan.schema, PLAN_DOCUMENT_SCHEMA);
    assert_eq!(plan.project_id.as_str(), "project:alpha");
    assert_eq!(plan.plan_id.as_str(), "plan:alpha");
    assert_eq!(plan.work_items.len(), 2);

    // The wire form is produced by the same types that parsed it, so a value
    // cannot mean one thing on the way in and another on the way out.
    let value = serde_json::to_value(&plan).expect("the document serializes");
    let reparsed = PlanDocument::from_value(value).expect("the value is canonical");
    assert_eq!(reparsed, plan);

    let text = serde_json::to_string(&plan).expect("the document serializes");
    let reparsed = PlanDocument::from_json(&text).expect("the text is canonical");
    assert_eq!(reparsed, plan);
}

#[test]
fn admission_reports_the_source_correspondence_of_every_work_item() {
    let admission = admits().admit().expect("the document admits");
    assert_eq!(admission.work_item_count, 2);
    assert_eq!(admission.mapping.len(), 2);
    assert_eq!(admission.mapping[0].source_id.as_str(), "source:roadmap");
    assert_eq!(admission.mapping[0].source_anchor, "heading:Read");
    assert_eq!(admission.mapping[1].source_anchor, "heading:Write");

    // Attribution is addressable per work item, not per document.
    let read = WorkItemId::declare("work:read").expect("a bounded identity");
    let mapping = admission
        .anchor_of(&read)
        .expect("the work item is anchored");
    assert_eq!(mapping.source_anchor, "heading:Read");
    assert_eq!(
        admission.anchor_of(&WorkItemId::declare("work:absent").unwrap()),
        None
    );
}

#[test]
fn a_document_that_asserts_progress_is_refused_by_name_and_path() {
    // The source had a status column. The conversion must decide what the plan
    // should declare instead; an import can never mark anything executed.
    for (field, value) in [
        ("status", json!("done")),
        ("accepted", json!(true)),
        ("progress", json!(100)),
        ("completedAt", json!("2026-01-01")),
    ] {
        let mut item = work_item("work:read", "heading:Read");
        item[field] = value;
        let diagnostics =
            PlanDocument::from_value(document(vec![item])).expect_err("progress is not admitted");
        assert_eq!(
            codes(&diagnostics),
            vec!["project_plan_progress_not_admitted"]
        );
        assert_eq!(paths(&diagnostics), vec![format!("workItems[0].{field}")]);
        assert!(
            diagnostics[0]
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("established by the owner")),
            "{:?}",
            diagnostics[0]
        );
    }
}

#[test]
fn a_document_that_asserts_progress_at_its_own_level_is_refused_too() {
    let mut plan = document(vec![work_item("work:read", "heading:Read")]);
    plan["status"] = json!("in-progress");
    let diagnostics = PlanDocument::from_value(plan).expect_err("progress is not admitted");
    assert_eq!(
        codes(&diagnostics),
        vec!["project_plan_progress_not_admitted"]
    );
    assert_eq!(paths(&diagnostics), vec!["status"]);
}

#[test]
fn the_admitted_wire_form_has_no_field_that_could_carry_a_runtime_fact() {
    let admission = admits().admit().expect("the document admits");
    let value = serde_json::to_value(&admission).expect("the admission serializes");
    let mut keys = value
        .as_object()
        .expect("the admission is an object")
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    keys.sort();
    assert_eq!(
        keys,
        vec!["document", "inputCount", "mapping", "workItemCount"]
    );
    let item = value["document"]["workItems"][0]
        .as_object()
        .expect("a work item is an object");
    let mut fields = item.keys().cloned().collect::<Vec<_>>();
    fields.sort();
    assert_eq!(
        fields,
        vec![
            "acceptance",
            "inputs",
            "outcome",
            "roles",
            "sourceAnchor",
            "workItemId"
        ]
    );
}

#[test]
fn an_unknown_field_is_reported_with_its_path() {
    let mut item = work_item("work:read", "heading:Read");
    item["priority"] = json!(3);
    let diagnostics =
        PlanDocument::from_value(document(vec![item])).expect_err("an unknown field is ambiguous");
    assert_eq!(codes(&diagnostics), vec!["project_plan_unsupported_field"]);
    assert_eq!(paths(&diagnostics), vec!["workItems[0].priority"]);
}

#[test]
fn a_foreign_or_absent_schema_is_reported_rather_than_read_best_effort() {
    let mut plan = document(vec![work_item("work:read", "heading:Read")]);
    plan["schema"] = json!("some-other.plan/v9");
    let diagnostics = PlanDocument::from_value(plan).expect_err("a foreign schema is refused");
    assert_eq!(codes(&diagnostics), vec!["project_plan_schema_unsupported"]);
    assert_eq!(diagnostics[0].detail.as_deref(), Some("some-other.plan/v9"));

    let mut plan = document(vec![work_item("work:read", "heading:Read")]);
    plan.as_object_mut().expect("an object").remove("schema");
    let diagnostics = PlanDocument::from_value(plan).expect_err("an absent schema is refused");
    assert_eq!(codes(&diagnostics), vec!["project_plan_schema_required"]);
}

#[test]
fn an_empty_declaration_is_refused_instead_of_read_as_a_deletion() {
    let diagnostics = PlanDocument::from_value(document(Vec::new())).expect_err("empty is refused");
    assert_eq!(
        codes(&diagnostics),
        vec!["project_plan_work_items_required"]
    );
    assert_eq!(
        diagnostics[0].detail.as_deref(),
        Some("an empty declaration is not a deletion")
    );

    let mut plan = document(vec![work_item("work:read", "heading:Read")]);
    plan.as_object_mut().expect("an object").remove("workItems");
    let diagnostics = PlanDocument::from_value(plan).expect_err("absent is refused");
    assert_eq!(
        codes(&diagnostics),
        vec!["project_plan_work_items_required"]
    );
}

#[test]
fn a_duplicate_work_item_identity_is_refused_with_the_second_position() {
    let plan = PlanDocument::from_value(document(vec![
        work_item("work:read", "heading:Read"),
        work_item("work:read", "heading:Read again"),
    ]))
    .expect("the shape is canonical");
    let diagnostics = plan.admit().expect_err("a duplicate is refused");
    assert_eq!(
        codes(&diagnostics),
        vec!["project_plan_work_item_duplicate"]
    );
    assert_eq!(paths(&diagnostics), vec!["workItems[1].workItemId"]);
}

#[test]
fn an_input_that_names_a_work_item_the_plan_does_not_declare_is_refused() {
    let mut item = work_item("work:write", "heading:Write");
    item["inputs"] = json!([
        {"kind": "local", "producerWorkItemId": "work:absent", "path": "build/out.json"}
    ]);
    let plan = PlanDocument::from_value(document(vec![item])).expect("the shape is canonical");
    let diagnostics = plan.admit().expect_err("an unknown producer is refused");
    assert_eq!(
        codes(&diagnostics),
        vec!["project_plan_input_unknown_work_item"]
    );
    assert_eq!(paths(&diagnostics), vec!["workItems[0].inputs[0]"]);
    assert_eq!(
        diagnostics[0].detail.as_deref(),
        Some("project:alpha/work:absent")
    );
}

#[test]
fn an_input_cycle_is_refused_with_the_path_that_closes_it() {
    let mut first = work_item("work:first", "heading:First");
    first["inputs"] = json!([
        {"kind": "local", "producerWorkItemId": "work:second", "path": "build/second.json"}
    ]);
    let mut second = work_item("work:second", "heading:Second");
    second["inputs"] = json!([
        {"kind": "local", "producerWorkItemId": "work:first", "path": "build/first.json"}
    ]);
    let plan = PlanDocument::from_value(document(vec![first, second])).expect("canonical");
    let diagnostics = plan.admit().expect_err("a cycle is refused");
    assert_eq!(codes(&diagnostics), vec!["project_plan_dependency_cycle"]);
    assert!(
        diagnostics[0]
            .detail
            .as_deref()
            .is_some_and(|detail| detail.contains("->")),
        "{:?}",
        diagnostics[0]
    );
}

#[test]
fn a_cross_project_input_is_left_to_the_store_and_an_in_plan_one_is_resolved() {
    // A cross-project reference names another project: admission resolves only
    // the plan's own identities and leaves the registration question to the
    // apply path, which asks the same store the dependency owner asks.
    let mut consumer = work_item("work:consume", "heading:Consume");
    consumer["inputs"] = json!([
        {"kind": "cross-project", "projectId": "project:beta", "workItemId": "work:produce"},
        {"kind": "cross-project", "projectId": "project:alpha", "workItemId": "work:produce"}
    ]);
    let plan = PlanDocument::from_value(document(vec![
        consumer,
        work_item("work:produce", "heading:Produce"),
    ]))
    .expect("the shape is canonical");
    let admission = plan
        .admit()
        .expect("a registered-looking cross-project input resolves later");
    assert_eq!(admission.input_count, 2);

    // The same project named explicitly is still this plan's own identity, so it
    // must be declared here.
    let mut consumer = work_item("work:consume", "heading:Consume");
    consumer["inputs"] = json!([
        {"kind": "cross-project", "projectId": "project:beta", "workItemId": "work:produce"},
        {"kind": "cross-project", "projectId": "project:alpha", "workItemId": "work:absent"}
    ]);
    let plan = PlanDocument::from_value(document(vec![
        consumer,
        work_item("work:produce", "heading:Produce"),
    ]))
    .expect("the shape is canonical");
    let diagnostics = plan.admit().expect_err("the in-plan reference is resolved");
    assert_eq!(
        codes(&diagnostics),
        vec!["project_plan_input_unknown_work_item"]
    );
    assert_eq!(paths(&diagnostics), vec!["workItems[0].inputs[1]"]);
}

#[test]
fn a_repeated_input_or_role_in_one_work_item_is_refused() {
    let mut consumer = work_item("work:consume", "heading:Consume");
    consumer["inputs"] = json!([
        {"kind": "cross-project", "projectId": "project:beta", "workItemId": "work:produce"},
        {"kind": "cross-project", "projectId": "project:beta", "workItemId": "work:produce"}
    ]);
    consumer["roles"] = json!([
        {"roleId": "role:maintainer", "scope": "work-item"},
        {"roleId": "role:maintainer", "scope": "work-item"}
    ]);
    let plan = PlanDocument::from_value(document(vec![consumer])).expect("canonical");
    let diagnostics = plan.admit().expect_err("a repeated declaration is refused");
    assert_eq!(
        codes(&diagnostics),
        vec![
            "project_plan_role_duplicate",
            "project_plan_input_duplicate"
        ]
    );
    assert_eq!(
        paths(&diagnostics),
        vec!["workItems[0].roles[1].roleId", "workItems[0].inputs[1]"]
    );
}

#[test]
fn every_diagnostic_of_one_document_is_reported_together() {
    // One submission, one answer: a caller correcting a conversion sees all the
    // reasons at once instead of one per round trip.
    let mut item = work_item("work:read", "heading:Read");
    item["status"] = json!("done");
    item["estimate"] = json!(5);
    let diagnostics =
        PlanDocument::from_value(document(vec![item])).expect_err("the document is ambiguous");
    assert_eq!(
        codes(&diagnostics),
        vec![
            "project_plan_unsupported_field",
            "project_plan_progress_not_admitted"
        ]
    );
}

#[test]
fn declared_identities_and_text_are_bounded_before_any_effect() {
    assert!(SourceId::declare("source:roadmap").is_ok());
    assert!(SourceKind::declare("markdown").is_ok());
    assert!(SourceKind::declare("jira-export").is_ok());
    for refused in ["", "  ", "../etc", "a/b", "with space", "with\0nul"] {
        assert!(
            SourceId::declare(refused).is_err(),
            "{refused} must be refused as a source identity"
        );
        assert!(
            SourceKind::declare(refused).is_err(),
            "{refused} must be refused as a source kind"
        );
    }
    assert!(
        SourceId::declare("s".repeat(129)).is_err(),
        "a source identity is bounded"
    );

    // A locator is attribution a person reads, so prose is allowed and only
    // absence, size and NUL are refusals.
    assert_eq!(
        SourceLocator::declare("docs/plan.md §3 — Roadmap")
            .expect("prose is a usable locator")
            .as_str(),
        "docs/plan.md §3 — Roadmap"
    );
    for refused in ["", "   ", "with\0nul"] {
        assert!(
            SourceLocator::declare(refused).is_err(),
            "{refused} must be refused as a locator"
        );
    }
    assert!(
        SourceLocator::declare("l".repeat(4097)).is_err(),
        "a locator is bounded"
    );
}

#[test]
fn a_document_beyond_its_declared_bounds_is_refused_before_admission() {
    let items = (0..=MAX_PLAN_WORK_ITEMS)
        .map(|position| work_item(&format!("work:{position}"), &format!("heading:{position}")))
        .collect::<Vec<_>>();
    let diagnostics =
        PlanDocument::from_value(document(items)).expect_err("an oversized plan is refused");
    assert!(
        codes(&diagnostics).contains(&"project_plan_too_large"),
        "{diagnostics:?}"
    );

    let criteria = (0..=MAX_PLAN_ACCEPTANCE)
        .map(|position| json!(format!("criterion {position}")))
        .collect::<Vec<_>>();
    let mut item = work_item("work:read", "heading:Read");
    item["acceptance"] = Value::Array(criteria);
    let diagnostics =
        PlanDocument::from_value(document(vec![item])).expect_err("too many criteria is refused");
    assert!(
        codes(&diagnostics).contains(&"project_plan_too_large"),
        "{diagnostics:?}"
    );
}

#[test]
fn an_undeclared_text_field_is_refused_by_name() {
    let mut item = work_item("work:read", "heading:Read");
    item["outcome"] = json!("   ");
    let diagnostics =
        PlanDocument::from_value(document(vec![item])).expect_err("an empty outcome is refused");
    assert_eq!(codes(&diagnostics), vec!["project_plan_text_required"]);
    assert_eq!(paths(&diagnostics), vec!["workItems[0].outcome"]);
}

#[test]
fn the_role_reference_carries_a_reference_and_no_grant() {
    let role = RoleReference {
        role_id: RoleId::declare("role:maintainer").expect("a bounded role"),
        scope: RoleScope::WorkItem,
        capability: None,
    };
    let value = serde_json::to_value(&role).expect("a role reference serializes");
    let mut keys = value
        .as_object()
        .expect("a role reference is an object")
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    keys.sort();
    assert_eq!(keys, vec!["capability", "roleId", "scope"]);
    assert_eq!(RoleScope::parse("work-item"), Some(RoleScope::WorkItem));
    assert_eq!(RoleScope::parse("owner"), None);
    assert!(RoleId::declare("role:maintainer").is_ok());
    assert!(RoleId::declare("membership:owner secret").is_err());
}

#[test]
fn the_import_refusal_stage_is_the_one_the_owner_publishes() {
    assert_eq!(IMPORT_STAGE, "project/import");
    let failure = ProjectFailure::import("project_plan_source_identity_required");
    assert_eq!(failure.stage(), IMPORT_STAGE);
    assert_eq!(failure.code(), "project_plan_source_identity_required");
}

#[test]
fn a_declared_local_input_keeps_the_shape_the_dependency_owner_stores() {
    let mut item = work_item("work:write", "heading:Write");
    item["inputs"] = json!([
        {"kind": "local", "producerWorkItemId": "work:read", "path": "build/out.json"}
    ]);
    let admission =
        PlanDocument::from_value(document(vec![item, work_item("work:read", "heading:Read")]))
            .expect("the shape is canonical")
            .admit()
            .expect("the document admits");
    let write = WorkItemId::declare("work:write").expect("a bounded identity");
    let inputs = admission
        .inputs_of(&write)
        .expect("the work item is declared");
    assert_eq!(
        inputs,
        [
            ArtifactReference::local(WorkItemId::declare("work:read").unwrap(), "build/out.json")
                .unwrap()
        ]
    );
}
