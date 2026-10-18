//! The package's own replay parity evidence.
//!
//! `licoup-native` still composes the production parser set, and its own
//! `native_agent_parser` corpus check proves the thirteen-adapter corpus is
//! complete and frame-deterministic through that composition. This test proves
//! the same thing one layer down, where the parser now lives: the `opencode`
//! transcript is replayed against *this* package's registration and replay arm,
//! so a projection that only the composition could still satisfy would fail
//! here.
//!
//! It reads the repository corpus through the SDK's own harness — the same
//! fixture root, the same scenario set, the same comparison — so the evidence is
//! the harness's and not a second reading of the fixtures. The harness rejects a
//! frame whose recorded channel is not this package's declared framing and a
//! projection that differs from what the real parser produced, so this claim
//! fails when the protocol constant changes and when the parser changes.

use licoup_agent_adapter_sdk::replay::{fixture_root, replay_corpus};

use licoup_agent_opencode::registration::parser_set;

#[test]
fn the_packages_own_parser_replays_the_recorded_opencode_corpus() {
    let coverage = replay_corpus(&parser_set(), &fixture_root());
    assert_eq!(
        coverage.keys().collect::<Vec<_>>(),
        vec!["opencode"],
        "a program that carries no OpenCode fixture is never a pass"
    );
    assert_eq!(coverage["opencode"].len(), 5);
    for scenario in [
        "normal-turn",
        "user-cancel",
        "agent-error",
        "streaming-interruption",
        "native-resume",
    ] {
        assert!(
            coverage["opencode"].contains(scenario),
            "the corpus must record {scenario}"
        );
    }
}
