use serde_json::Value;

pub fn render_semantic_markdown(semantic: &Value) -> String {
    let mut out = String::new();
    out.push_str("# Semantic Conversation\n\n");
    out.push_str("Default view: **thread**. Execution is collapsible. Audit and raw evidence are diagnostic-only.\n\n");

    out.push_str("## Thread\n\n");
    if let Some(thread) = semantic.get("thread").and_then(Value::as_array) {
        if thread.is_empty() {
            out.push_str("_No thread messages._\n\n");
        }
        for event in thread {
            let role = event
                .get("role")
                .and_then(Value::as_str)
                .unwrap_or("assistant");
            let text = event.get("text").and_then(Value::as_str).unwrap_or("");
            out.push_str(&format!("### {}\n\n{}\n\n", role_heading(role), text));
        }
    }

    out.push_str(
        "## Execution\n\n<details>\n<summary>Execution trace (collapsed by default)</summary>\n\n",
    );
    if let Some(execution) = semantic.get("execution").and_then(Value::as_array) {
        if execution.is_empty() {
            out.push_str("_No execution events._\n\n");
        }
        for event in execution {
            out.push_str(&format!(
                "- **{}** (`{}`): {}\n",
                event
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or("Event"),
                event
                    .get("eventKind")
                    .and_then(Value::as_str)
                    .unwrap_or("event"),
                event.get("summary").and_then(Value::as_str).unwrap_or("")
            ));
        }
        out.push('\n');
    }
    out.push_str("</details>\n\n");

    out.push_str("## Artifacts\n\n");
    if let Some(artifacts) = semantic.get("artifacts").and_then(Value::as_array) {
        if artifacts.is_empty() {
            out.push_str("_No artifacts._\n\n");
        }
        for artifact in artifacts {
            out.push_str(&format!(
                "- `{}` ({}) → `{}`\n",
                artifact
                    .get("label")
                    .and_then(Value::as_str)
                    .unwrap_or("artifact"),
                artifact
                    .get("kind")
                    .and_then(Value::as_str)
                    .unwrap_or("document"),
                artifact
                    .get("ref")
                    .and_then(Value::as_str)
                    .unwrap_or("(ref)")
            ));
        }
        out.push('\n');
    }

    out.push_str("## Audit (diagnostics)\n\n");
    if let Some(audit) = semantic.get("audit") {
        out.push_str(&format!(
            "- Adapter: `{}`\n- Host: `{}`\n- Source kind: `{}`\n- Native session: `{}`\n- Redaction: `{}`\n- Validation: `{}`\n",
            audit.get("adapterId").and_then(Value::as_str).unwrap_or(""),
            audit.get("hostApp").and_then(Value::as_str).unwrap_or(""),
            audit.get("sourceKind").and_then(Value::as_str).unwrap_or(""),
            audit.get("nativeSessionId").and_then(Value::as_str).unwrap_or(""),
            audit.get("redactionStatus").and_then(Value::as_str).unwrap_or(""),
            audit.get("validationStatus").and_then(Value::as_str).unwrap_or("")
        ));
        if let Some(evidence) = audit.get("sourceEvidence") {
            out.push_str(&format!(
                "- Evidence: `{}` hash=`{}`\n",
                evidence
                    .get("pathRef")
                    .and_then(Value::as_str)
                    .unwrap_or(""),
                evidence
                    .get("contentHash")
                    .and_then(Value::as_str)
                    .unwrap_or("")
            ));
        }
        if let Some(warnings) = audit.get("parseWarnings").and_then(Value::as_array)
            && !warnings.is_empty()
        {
            out.push_str("- Parse warnings:\n");
            for warning in warnings {
                if let Some(text) = warning.as_str() {
                    out.push_str(&format!("  - {}\n", text));
                }
            }
        }
        out.push('\n');
    }

    out.push_str("## Raw evidence (diagnostics)\n\n");
    if let Some(refs) = semantic
        .get("raw")
        .and_then(|raw| raw.get("evidenceRefs"))
        .and_then(Value::as_array)
    {
        for evidence in refs {
            out.push_str(&format!(
                "- `{}` (`{}`) hash=`{}`\n",
                evidence
                    .get("pathRef")
                    .and_then(Value::as_str)
                    .unwrap_or(""),
                evidence
                    .get("kind")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown"),
                evidence
                    .get("contentHash")
                    .and_then(Value::as_str)
                    .unwrap_or("")
            ));
        }
    }
    out
}

fn role_heading(role: &str) -> &'static str {
    match role {
        "user" => "User",
        _ => "Assistant",
    }
}
