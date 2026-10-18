//! The package's own replay parity evidence.
//!
//! `licoup-native` still composes the production parser set, and its own
//! `native_agent_parser` corpus check proves the thirteen-adapter corpus is
//! complete and frame-deterministic through that composition. This test proves
//! the same thing one layer down, where the parser now lives: the `antigravity`
//! transcript is replayed against *this* package's registration and replay arm,
//! so a projection that only the composition could still satisfy would fail
//! here.
//!
//! It reads the repository corpus through the SDK's own harness — the same
//! fixture root, the same scenario set, the same comparison — so the evidence is
//! the harness's and not a second reading of the fixtures.

use licoup_agent_adapter_sdk::replay::{fixture_root, replay_corpus};

use licoup_agent_antigravity::registration::parser_set;

#[test]
fn the_packages_own_parser_replays_the_recorded_antigravity_corpus() {
    let coverage = replay_corpus(&parser_set(), &fixture_root());
    assert_eq!(
        coverage.keys().collect::<Vec<_>>(),
        vec!["antigravity"],
        "a program that carries no Antigravity fixture is never a pass"
    );
    assert_eq!(coverage["antigravity"].len(), 5);
    for scenario in [
        "normal-turn",
        "user-cancel",
        "agent-error",
        "streaming-interruption",
        "native-resume",
    ] {
        assert!(
            coverage["antigravity"].contains(scenario),
            "the corpus must record {scenario}"
        );
    }
}
