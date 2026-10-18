use serde_json::json;

use super::super::markdown::render_semantic_markdown;

#[test]
fn markdown_keeps_default_thread_visible_and_diagnostics_explicit() {
    let semantic = json!({
        "thread": [{"role": "user", "text": "Hello"}],
        "execution": [{"title": "Read", "eventKind": "tool-call", "summary": "Hidden details"}],
        "artifacts": [{"label": "Report", "kind": "document", "ref": "report.md"}],
        "audit": {
            "adapterId": "codex", "hostApp": "codex", "sourceKind": "jsonl",
            "nativeSessionId": "session-1", "redactionStatus": "applied",
            "validationStatus": "ok", "parseWarnings": []
        },
        "raw": {"evidenceRefs": []}
    });
    let markdown = render_semantic_markdown(&semantic);
    assert!(markdown.contains("## Thread"));
    assert!(markdown.contains("### User\n\nHello"));
    assert!(markdown.contains("<details>"));
    assert!(markdown.contains("## Audit (diagnostics)"));
    assert!(markdown.contains("## Raw evidence (diagnostics)"));
}
