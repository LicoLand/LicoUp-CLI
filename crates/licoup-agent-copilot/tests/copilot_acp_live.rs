//! Live ACP acceptance for the published Copilot card.
//!
//! The default suite is hermetic; this arm is opt-in because it needs the
//! GitHub Copilot CLI on `PATH`. It drives the exact launch declaration
//! `a4aa-copilot` publishes — not a local copy — through the same shared ACP
//! engine the host composes, and accepts only a completed `initialize`
//! handshake.
//!
//! Run with:
//! `cargo test -p licoup-agent-copilot --test copilot_acp_live -- --ignored`

use licoup_agent_copilot::driver::{capability_probe, driver};
use std::path::Path;

#[test]
#[ignore = "requires the GitHub Copilot CLI on PATH"]
fn the_published_card_launches_copilot_and_completes_the_acp_handshake() {
    assert!(
        licoup_agent_drivers::acp_driver_runtime::parser_port::install(vec![
            licoup_agent_copilot::registration::DIALECT
        ]),
        "this binary installs the Copilot dialect once"
    );
    let (program, args) = licoup_agent_copilot::upstream::acp_launch();
    assert_eq!(program, "copilot");
    assert_eq!(args, ["--acp", "--stdio", "--no-auto-update"]);
    assert_eq!(driver().launch_args, args);

    let probe = capability_probe(
        program,
        Path::new(env!("CARGO_MANIFEST_DIR")),
        30_000,
        Some(64 * 1024),
        16 * 1024,
    )
    .expect("the published ACP command completes initialize");
    assert!(
        probe.protocol_version.is_some(),
        "the agent reported an ACP protocol version"
    );
}
