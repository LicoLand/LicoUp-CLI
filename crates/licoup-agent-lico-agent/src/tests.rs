//! This package's own claims about the Lico Agent adapter it carries.
//!
//! A claim that names Lico Agent belongs here rather than in the SDK or in the
//! client: which adapter id this package registers, how one frame becomes one
//! effect, how one execution outcome becomes the shared transition vocabulary,
//! which native session identities and which transcript and plan locations this
//! protocol resumes by, and — the claim that matters most — that the five
//! recorded transcripts still replay through the real parser.

use licoup_agent_adapter_sdk::adapters::NativeLineParser;
use licoup_agent_adapter_sdk::replay::{SCENARIOS, fixture_root, replay_corpus};
use licoup_agent_adapter_sdk::{LifecycleStage, Transition};
use serde_json::json;

use crate::port::execution;
use crate::registration::{self, ADAPTER_ID, CONTRACT, FRAMING};
use crate::{parser, session};

/// The five recorded transcripts, in the order the harness checks them.
const LICO_AGENT_SCENARIOS: [&str; 5] = SCENARIOS;

/// One data root no other test shares.
fn temp_root(label: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!(
        "lico-agent-package-{label}-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&root).expect("a disposable data root");
    root
}

#[test]
fn the_package_registers_exactly_one_adapter_through_the_sdk_port() {
    let set = registration::parser_set();
    assert_eq!(set.registered_ids(), [ADAPTER_ID]);
    assert_eq!(set.all().len(), 1);

    let contract = set
        .contract(ADAPTER_ID)
        .expect("the composed set answers for the adapter it carries");
    assert_eq!(contract, CONTRACT);
    assert_eq!(contract.id, ADAPTER_ID);
    assert_eq!(contract.framing, FRAMING);
    assert_eq!(registration::contract(), Some(CONTRACT));

    // Lico Agent reports its transitions with its own execution result and the
    // Subagent mesh never dispatches it, so both SDK queries stay declared and
    // unanswered rather than inheriting a neighbouring Agent's answer.
    let entry = set
        .registration(ADAPTER_ID)
        .expect("the composed set answers for the adapter it carries");
    let outcome = licoup_agent_adapter_sdk::port::ExecutionOutcome {
        output: "a reply the query must not project",
        failure: None,
    };
    assert_eq!((entry.execution_transitions)(&outcome), Vec::new());
    assert!(!(entry.valid_identity)(
        &licoup_agent_adapter_sdk::port::DurableIdentityRequest {
            session_id: "11111111-2222-3333-4444-555555555555",
            location: None,
        }
    ));

    // Another adapter is refused rather than defaulted: this program carries one
    // Agent, and it says so.
    assert_eq!(set.contract("codex"), None);
    assert!(set.registration("kimi-code").is_none());
}

#[test]
fn one_line_becomes_one_effect_and_is_never_classified_twice() {
    let mut frames = parser::RpcParser;

    assert_eq!(
        frames
            .parse_line(br#"{"type":"response","success":true}"#)
            .unwrap(),
        parser::RpcEffect::Handshake {
            accepted: true,
            session_id: None
        }
    );
    assert_eq!(
        frames
            .parse_line(br#"{"type":"response","success":true,"data":{"sessionId":"native-1"}}"#)
            .unwrap(),
        parser::RpcEffect::Handshake {
            accepted: true,
            session_id: Some("native-1".to_owned())
        }
    );
    // A handshake the program refused is its own effect, not an accepted one
    // with a missing session identity.
    assert_eq!(
        frames
            .parse_line(br#"{"type":"response","success":false,"data":{"sessionId":"native-1"}}"#)
            .unwrap(),
        parser::RpcEffect::Handshake {
            accepted: false,
            session_id: Some("native-1".to_owned())
        }
    );
    assert_eq!(
        frames
            .parse_line(br#"{"assistantMessageEvent":{"delta":"hello"}}"#)
            .unwrap(),
        parser::RpcEffect::Text {
            delta: "hello".to_owned()
        }
    );
    for frame in [
        br#"{"type":"agent.event"}"#.as_slice(),
        br#"{"type":"agent.progress"}"#.as_slice(),
        br#"{"type":"agent.tool"}"#.as_slice(),
    ] {
        assert_eq!(
            frames.parse_line(frame).unwrap(),
            parser::RpcEffect::Processing
        );
    }
    assert_eq!(
        frames
            .parse_line(br#"{"type":"agent.interaction"}"#)
            .unwrap(),
        parser::RpcEffect::Control {
            method: "agent.interaction".to_owned()
        }
    );
    assert_eq!(
        frames.parse_line(br#"{"type":"agent_end"}"#).unwrap(),
        parser::RpcEffect::Completed
    );
    assert_eq!(
        frames
            .parse_line(br#"{"type":"error","code":"lico_agent_transcript_persist_failed"}"#)
            .unwrap(),
        parser::RpcEffect::Failed {
            code: Some("lico_agent_transcript_persist_failed".to_owned())
        }
    );
    // An error frame without a code is still a failure: the classification is
    // the frame's type, and the code is the optional detail.
    assert_eq!(
        frames.parse_line(br#"{"type":"error"}"#).unwrap(),
        parser::RpcEffect::Failed { code: None }
    );
    // A frame this protocol carries no fact for is reported rather than
    // dropped, so a reader can tell "nothing to project" from "never seen".
    assert_eq!(
        frames.parse_line(br#"{"type":"something_else"}"#).unwrap(),
        parser::RpcEffect::Ignored
    );
    assert_eq!(
        frames.parse_line(b"  ").unwrap_err(),
        parser::FrameError::Empty
    );
    assert_eq!(
        frames.parse_line(b"not-json").unwrap_err(),
        parser::FrameError::InvalidJson
    );
}

#[test]
fn the_request_envelopes_are_the_protocols_own_and_are_lf_terminated() {
    assert_eq!(
        parser::readiness_request(),
        json!({"id":"lico-1","type":"get_state"})
    );
    assert_eq!(
        parser::prompt_request("exact user prompt"),
        json!({"id":"lico-2","type":"prompt","message":"exact user prompt"})
    );
    let encoded =
        parser::encode_request(&parser::prompt_request("line\nbreak")).expect("a request encodes");
    assert_eq!(encoded.last(), Some(&b'\n'));
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&encoded[..encoded.len() - 1])
            .expect("one JSON document"),
        json!({"id":"lico-2","type":"prompt","message":"line\nbreak"})
    );
}

#[test]
fn one_execution_outcome_becomes_the_shared_transition_vocabulary() {
    // The reducer walks the declared machine from its initial state to the one
    // the outcome reports, so the first entry is the machine's own start rather
    // than the stage the driver reported.
    assert_eq!(
        parser::success_transitions("hello", true, &[]),
        [
            Transition::Lifecycle(LifecycleStage::Submitted),
            Transition::Lifecycle(LifecycleStage::Accepted),
            Transition::Lifecycle(LifecycleStage::Processing),
            Transition::Lifecycle(LifecycleStage::Responding),
            Transition::Text {
                unit_id: "lico-agent:reply".to_owned(),
                text: "hello".to_owned()
            },
            Transition::Lifecycle(LifecycleStage::Completed),
        ]
    );
    // An empty answer reaches the terminal stage without inventing a text unit.
    assert_eq!(
        parser::success_transitions("", false, &[]).last(),
        Some(&Transition::Lifecycle(LifecycleStage::Completed))
    );
    let controls = parser::success_transitions("", false, &["agent.interaction".to_owned()]);
    // An answer with no text still walks the machine to its terminal stage, and
    // the walk is prefix closed: the stages between the reported one and the
    // terminal one are the reducer's own, in order.
    assert_eq!(
        controls,
        [
            Transition::Lifecycle(LifecycleStage::Submitted),
            Transition::Lifecycle(LifecycleStage::Accepted),
            Transition::Control {
                method: "agent.interaction".to_owned(),
                summary: "Native agent interaction requires an explicit client response."
                    .to_owned()
            },
            Transition::Lifecycle(LifecycleStage::Processing),
            Transition::Lifecycle(LifecycleStage::Responding),
            Transition::Lifecycle(LifecycleStage::Completed),
        ]
    );
    let failure = parser::failure_transitions("lico_agent_x", "session/resume", "message");
    assert_eq!(
        failure.first(),
        Some(&Transition::Lifecycle(LifecycleStage::Submitted))
    );
    assert!(
        matches!(failure.last(), Some(Transition::Failed { .. })),
        "{failure:?}"
    );
}

#[test]
fn the_session_identity_rule_accepts_only_canonical_native_identities() {
    // A UUID whose canonical spelling carries hex letters, so a differently
    // cased spelling is a different string rather than the same one.
    let canonical = "abcdef01-2345-6789-abcd-ef0123456789";
    assert_eq!(
        session::canonical_session_id(canonical),
        Ok(canonical.to_owned())
    );
    // Surrounding whitespace is the caller's, not the identity's.
    assert_eq!(
        session::canonical_session_id(&format!("  {canonical}  ")),
        Ok(canonical.to_owned())
    );
    // A differently-spelled identity would open a second transcript for one
    // conversation, so it is refused rather than normalized.
    assert_eq!(
        session::canonical_session_id(&canonical.to_ascii_uppercase()),
        Err(session::SESSION_ID_INVALID)
    );
    assert_eq!(
        session::canonical_session_id("not-a-uuid"),
        Err(session::SESSION_ID_INVALID)
    );
    assert_eq!(
        session::canonical_session_id(""),
        Err(session::SESSION_ID_INVALID)
    );
}

#[test]
fn the_published_session_and_plan_layout_is_the_one_this_package_derives() {
    // The locations the skill `lico-agent-target-lico-agent` publishes as
    // adapter facts, asserted here so a rename cannot pass silently.
    assert_eq!(
        session::SESSIONS_RELATIVE_PATH,
        "client-state/lico-agent/sessions"
    );
    assert_eq!(session::PLAN_DIRECTORY_RELATIVE_PATH, "client-state/plans");
    assert_eq!(session::ACTIVE_PLAN_FILE, "active-plan.md");
    assert_eq!(session::TRANSCRIPT_EXTENSION, "jsonl");

    let root = std::path::Path::new("/data");
    assert_eq!(
        session::sessions_dir(root),
        std::path::Path::new("/data/client-state/lico-agent/sessions")
    );
    assert_eq!(
        session::transcript_path(&session::sessions_dir(root), "abc"),
        std::path::Path::new("/data/client-state/lico-agent/sessions/abc.jsonl")
    );
    assert_eq!(
        session::active_plan_path(root),
        std::path::Path::new("/data/client-state/plans/active-plan.md")
    );
}

#[test]
fn one_turn_is_prepared_against_a_private_store_under_its_own_data_root() {
    let root = temp_root("session");
    let prepared = session::prepare(&root, "").expect("a fresh data root allocates an identity");
    assert!(!prepared.resume, "an allocated session is not a resume");
    assert_eq!(
        prepared.session_id,
        session::canonical_session_id(&prepared.session_id).unwrap()
    );
    assert_eq!(
        prepared.transcript,
        session::transcript_path(&session::sessions_dir(&root), &prepared.session_id)
    );
    assert!(
        !prepared.transcript.exists(),
        "nothing is written before the turn"
    );

    // A caller-named identity resolves to the same transcript and resumes it;
    // whether that transcript is readable is the transcript owner's answer.
    let resumed = session::prepare(&root, &prepared.session_id).expect("a canonical identity");
    assert!(resumed.resume);
    assert_eq!(resumed.transcript, prepared.transcript);

    assert_eq!(
        session::prepare(&root, "not-a-uuid").unwrap_err(),
        session::SESSION_ID_INVALID
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_plan_mode_turn_is_bound_to_an_absolute_path_or_the_roots_active_plan() {
    let root = temp_root("plan");
    let absolute = root.join("named-plan.md");

    assert_eq!(
        session::named_plan_path(&json!({"planPath": absolute})),
        Some(absolute.clone()),
        "an absolute caller-named path wins"
    );
    assert_eq!(
        session::named_plan_path(&json!({"plan_path": absolute})),
        Some(absolute.clone()),
        "both spellings name the same fact"
    );
    // A relative caller-named path is refused rather than resolved against a
    // process working directory this package does not own.
    assert_eq!(
        session::named_plan_path(&json!({"planPath": "relative.md"})),
        None
    );
    assert_eq!(session::named_plan_path(&json!({})), None);

    let active = session::ensure_active_plan(&root);
    assert_eq!(active, session::active_plan_path(&root));
    assert!(
        active.is_file(),
        "the active plan exists before the program is launched"
    );
    // A second call reuses the file the first one made rather than rewriting it.
    std::fs::write(&active, b"# a plan the turn wrote\n").expect("the plan is writable");
    assert_eq!(session::ensure_active_plan(&root), active);
    assert_eq!(
        std::fs::read_to_string(&active).expect("the plan is readable"),
        "# a plan the turn wrote\n"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// The five committed transcripts, replayed through the same parser a live turn
/// drives, with every projection required to equal the corpus's own record.
#[test]
fn the_recorded_corpus_replays_through_the_real_parser() {
    let coverage = replay_corpus(&registration::parser_set(), &fixture_root());
    assert_eq!(coverage.len(), 1, "one Agent parser is composed");
    let scenarios = coverage
        .get(ADAPTER_ID)
        .expect("the corpus covers the adapter this package carries");
    assert_eq!(
        scenarios.len(),
        LICO_AGENT_SCENARIOS.len(),
        "every scenario class is replayed"
    );
    for scenario in LICO_AGENT_SCENARIOS {
        assert!(
            scenarios.contains(scenario),
            "lico-agent/{scenario}.json was replayed"
        );
    }
}

#[test]
fn the_replay_arm_refuses_an_adapter_this_package_does_not_carry() {
    let error = crate::replay::replay_arm("codex")
        .err()
        .expect("an adapter this package does not carry has no arm");
    assert!(error.contains("codex"), "{error}");
    assert!(crate::replay::replay_arm(ADAPTER_ID).is_ok());

    // The arm refuses a frame recorded on another channel rather than guessing,
    // so a transcript cannot be replayed through the wrong boundary.
    let mut arm = crate::replay::replay_arm(ADAPTER_ID).expect("this package's own arm");
    let error = arm
        .feed(&licoup_agent_adapter_sdk::replay::RecordedFrame {
            index: 0,
            direction: "agent-to-client".to_owned(),
            channel: "lf-ndjson-acp".to_owned(),
            payload: "{}".to_owned(),
        })
        .err()
        .expect("a foreign channel is refused");
    assert!(error.contains(FRAMING), "{error}");
}

#[test]
fn the_execution_port_is_fail_closed_until_the_host_answers() {
    // Nothing in this package installs the port: the host does, once per
    // process, through the extension host that starts this package's binary.
    // Until then a package running outside the client cannot claim it was
    // admitted.
    assert!(!execution::installed());
    assert!(!execution::admits_execution());

    execution::install(execution::ExecutionPort {
        admits_execution: || false,
    })
    .expect("the port is installed once");
    assert!(execution::installed());
    assert!(
        !execution::admits_execution(),
        "the host's refusal is an answer, not a missing one"
    );
    assert!(
        execution::install(execution::ExecutionPort {
            admits_execution: || true,
        })
        .is_err(),
        "installation is once per process"
    );
}
