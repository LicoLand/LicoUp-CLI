use super::*;

#[test]
fn failed_result_projects_bound_ids_and_parser_failure_transition() {
    let failure = ProtocolFailure::new("static", "Static failure.", "test")
        .with_session(Some("native-session"))
        .with_turn("turn-1");
    let result = RunResult::failed(failure, "started".to_string(), true, false);
    assert!(!result.ok);
    assert_eq!(result.session_id, "native-session");
    assert_eq!(result.thread_id, "native-session");
    assert_eq!(result.turn_id, "turn-1");
    assert!(matches!(
        result.transitions.last(),
        Some(licoup_agent_adapter_sdk::Transition::Failed { code, .. }) if code == "static"
    ));
    assert_eq!(result.effective.cwd, EffectiveSettings::default().cwd);
}

#[test]
fn transport_lifecycle_is_monotonic_and_single_claimed() {
    let lifecycle = TransportLifecycle::default();
    assert!(lifecycle.is_live());
    assert!(lifecycle.begin_closing());
    assert!(!lifecycle.is_live());
    assert!(lifecycle.is_closing());
    assert!(!lifecycle.begin_closing());
    assert!(lifecycle.mark_closed());
    assert!(lifecycle.is_closed());
    assert!(!lifecycle.mark_closed());
}

#[test]
fn transcript_keeps_every_turn_beyond_former_limits_and_pages_backward() {
    let mut transcript = CompleteTranscript::new();
    let oversized_output = "x".repeat(1024 * 1024 + 1);
    transcript.record_success(
        "turn-00",
        "prompt-00",
        vec![json!({"kind":"processing","evidenceKind":"tool"})],
        &oversized_output,
    );
    for index in 1..70 {
        transcript.record_success(
            &format!("turn-{index:02}"),
            &format!("prompt-{index:02}"),
            vec![json!({"kind":"processing","evidenceKind":"progress"})],
            &format!("output-{index:02}"),
        );
    }

    assert_eq!(transcript.turn_count(), 70);
    let (latest, next_before) = transcript.project_backward_page(None, 50);
    assert_eq!(latest.len(), 50);
    assert_eq!(latest[0]["turnId"], "turn-20");
    assert_eq!(latest[49]["turnId"], "turn-69");
    assert_eq!(next_before, Some(20));

    let (earlier, next_before) = transcript.project_backward_page(next_before, 100);
    assert_eq!(earlier.len(), 20);
    assert_eq!(earlier[0]["turnId"], "turn-00");
    assert_eq!(earlier[0]["prompt"], "prompt-00");
    assert_eq!(earlier[0]["events"][0]["evidenceKind"], "tool");
    assert_eq!(earlier[0]["output"], oversized_output);
    assert_eq!(next_before, None);

    transcript.clear();
    assert_eq!(transcript.turn_count(), 0);
    assert_eq!(transcript.byte_count(), 0);
}

#[test]
fn transcript_counts_complete_utf8_assistant_output_bytes() {
    let output = "甲🙂";
    let mut transcript = CompleteTranscript::new();
    transcript.record_success("turn", "提示", Vec::new(), output);
    assert_eq!(transcript.byte_count(), output.len());
    let (page, next_before) = transcript.project_backward_page(None, 50);
    assert_eq!(page[0]["output"], output);
    assert_eq!(next_before, None);
}
