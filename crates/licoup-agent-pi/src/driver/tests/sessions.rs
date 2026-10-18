use super::*;

#[test]
fn missing_resume_session_fails_without_argv_identity() {
    let failure = resolve_session_path_in_roots("missing-session-id", &[]).unwrap_err();
    assert_eq!(failure.code, "pi_session_not_found");
    assert_eq!(failure.session_id.as_deref(), Some("missing-session-id"));
}

#[test]
fn exact_resume_uses_the_only_matching_session_path_over_rpc() {
    let root = temporary_directory("lico-pi-session");
    let project = root.join("nested-project");
    fs::create_dir_all(&project).unwrap();
    let session_path = project.join("session.jsonl");
    fs::write(
        &session_path,
        r#"{"type":"session","version":3,"id":"abc-session"}
{"type":"message","private":"not-read-for-identity"}
"#,
    )
    .unwrap();
    let resolved =
        resolve_session_path_in_roots("abc-session", std::slice::from_ref(&root)).unwrap();
    assert_eq!(resolved, session_path);

    let mut protocol = PiProtocol::new(resume_config("continue please", "abc-session", resolved));
    let request = protocol.initial_request();
    assert_eq!(request["type"], "switch_session");
    assert_eq!(
        request["sessionPath"],
        session_path.to_string_lossy().as_ref()
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn duplicate_exact_session_identity_fails_closed() {
    let root = temporary_directory("lico-pi-ambiguous");
    fs::create_dir_all(root.join("a")).unwrap();
    fs::create_dir_all(root.join("b")).unwrap();
    let header = "{\"type\":\"session\",\"version\":3,\"id\":\"duplicate-session\"}\n";
    fs::write(root.join("a/first.jsonl"), header).unwrap();
    fs::write(root.join("b/second.jsonl"), header).unwrap();
    let failure = resolve_session_path_in_roots("duplicate-session", std::slice::from_ref(&root))
        .unwrap_err();
    assert_eq!(failure.code, "pi_session_identity_ambiguous");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn configured_session_root_has_strict_precedence_and_scope() {
    let explicit = PathBuf::from("explicit-root");
    let roots = session_roots_from_sources(
        Some("explicit-root"),
        Some("agent-root"),
        Some(PathBuf::from("home-root")),
    );
    assert_eq!(roots, vec![explicit]);
    assert_eq!(
        session_roots_from_sources(None, Some("agent-root"), None),
        vec![PathBuf::from("agent-root").join("sessions")]
    );
}

#[test]
fn session_identity_must_be_in_the_bounded_first_header_record() {
    let root = temporary_directory("lico-pi-header");
    let valid = root.join("valid.jsonl");
    let misplaced = root.join("misplaced.jsonl");
    fs::write(&valid, "{\"type\":\"session\",\"id\":\"exact\"}\n").unwrap();
    fs::write(
        &misplaced,
        "{\"type\":\"message\"}\n{\"type\":\"session\",\"id\":\"exact\"}\n",
    )
    .unwrap();
    assert!(session_header_matches(&valid, "exact").unwrap());
    assert!(!session_header_matches(&misplaced, "exact").unwrap());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn exact_session_discovery_walks_beyond_the_former_file_cap() {
    let root = temporary_directory("lico-pi-complete-tree");
    fs::create_dir_all(&root).unwrap();
    for index in 0..4_100 {
        fs::write(
            root.join(format!("unrelated-{index:04}.jsonl")),
            format!("{{\"type\":\"session\",\"id\":\"other-{index}\"}}\n"),
        )
        .unwrap();
    }
    let expected = root.join("requested.jsonl");
    fs::write(
        &expected,
        "{\"type\":\"session\",\"id\":\"requested-session\"}\n",
    )
    .unwrap();
    assert_eq!(
        resolve_session_path_in_roots("requested-session", std::slice::from_ref(&root)).unwrap(),
        expected
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn oversized_session_header_is_a_named_failure_not_false_not_found() {
    let root = temporary_directory("lico-pi-oversized-header");
    fs::create_dir_all(&root).unwrap();
    fs::write(
        root.join("oversized.jsonl"),
        format!("{{\"padding\":\"{}\"}}\n", "x".repeat(64 * 1024)),
    )
    .unwrap();
    let failure = resolve_session_path_in_roots("requested-session", std::slice::from_ref(&root))
        .unwrap_err();
    assert_eq!(failure.code, "pi_session_header_line_too_large");
    assert_eq!(failure.session_id.as_deref(), Some("requested-session"));
    let _ = fs::remove_dir_all(root);
}
