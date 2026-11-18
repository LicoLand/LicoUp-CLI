//! The published A4AA Codex dialect this package compiles against.
//!
//! `a4aa-codex` owns the standard description of Codex: its launch program and
//! argv, its permission presets, its mechanism surfaces and its app-server RPC
//! vocabulary. This package used to retype those facts beside its own parser and
//! driver, which meant one card change could leave the two answers disagreeing.
//! `build.rs` now projects the published card into the constants below, and the
//! package reads them instead of keeping a second copy.
//!
//! What stays LicoUp-owned is the product mapping: the adapter id this client
//! publishes, the packaged runtime protocol name, the client identity it sends
//! in `initialize`, the portable data root it binds into the child, and the
//! bounded transport that runs under the client's synchronous process owner.
//! Those are LicoUp decisions, not facts about Codex, and they are declared
//! here rather than derived from the card.
//!
//! The projection is build-time rather than a runtime dependency because
//! `a4aa`'s engine is async: this package is driven from the client's
//! synchronous process supervisor, and linking the engine here would replace
//! that owner instead of converging the dialect.

include!(concat!(env!("OUT_DIR"), "/codex_dialect.rs"));

/// The adapter id LicoUp publishes for this Agent.
///
/// The published card names the vendor catalog id `codex`; LicoUp's adapter
/// registry uses the same spelling, so the two identities are asserted equal in
/// `tests::the_published_card_is_the_lico_up_adapter`.
pub const ADAPTER_ID: &str = "codex";

/// The runtime protocol LicoUp packages and reports for this adapter.
///
/// This names the LicoUp package wire, not the upstream card: the card declares
/// an RPC lane and the methods on it, while the packaged protocol id is the
/// client's stable record key.
pub const RUNTIME_PROTOCOL: &str = "codex-app-server-stdio-jsonrpc";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_server::config::ProtocolConfig;
    use crate::app_server::driver::CodexLaunchSpec;

    #[test]
    fn the_published_card_is_the_lico_up_adapter() {
        assert_eq!(UPSTREAM_CARD_ID, ADAPTER_ID);
        assert_eq!(crate::registration::ADAPTER_ID, ADAPTER_ID);
        assert_eq!(UPSTREAM_LANE_KIND, "rpc");
        assert_eq!(UPSTREAM_PROGRAM, "codex");
    }

    #[test]
    fn the_driver_launches_the_published_app_server_lane() {
        let launch = CodexLaunchSpec::new(UPSTREAM_PROGRAM, None);
        assert_eq!(launch.executable, UPSTREAM_PROGRAM);
        assert_eq!(launch.args, UPSTREAM_LAUNCH_ARGS);
    }

    #[test]
    fn every_published_sandbox_mode_is_admitted_and_nothing_else_is() {
        for mode in UPSTREAM_PERMISSION_MODES {
            let params = serde_json::json!({ "sandbox": mode });
            ProtocolConfig::from_params(&params, "hello", "", None)
                .unwrap_or_else(|failure| panic!("published mode {mode} was refused: {failure:?}"));
        }
        let params = serde_json::json!({ "sandbox": "not-a-published-mode" });
        assert!(ProtocolConfig::from_params(&params, "hello", "", None).is_err());
    }

    #[test]
    fn the_published_rpc_vocabulary_is_complete() {
        for method in [
            UPSTREAM_INITIALIZE_METHOD,
            UPSTREAM_INITIALIZED_METHOD,
            UPSTREAM_SESSION_OPEN_METHOD,
            UPSTREAM_SESSION_RESUME_METHOD,
            UPSTREAM_PROMPT_METHOD,
            UPSTREAM_CANCEL_METHOD,
            UPSTREAM_TEXT_EVENT_METHOD,
            UPSTREAM_COMPLETION_METHOD,
            UPSTREAM_MODEL_LIST_METHOD,
            UPSTREAM_RATE_LIMITS_METHOD,
            UPSTREAM_UNARCHIVE_METHOD,
            UPSTREAM_STEER_METHOD,
        ] {
            assert!(!method.is_empty(), "the card declared an empty method");
        }
        const { assert!(!UPSTREAM_KILL_ON_CANCEL) };
        assert_eq!(UPSTREAM_COMPLETION_STATUS_FIELD, "status");
        assert_eq!(UPSTREAM_COMPLETION_TURN_FIELD, "id");
        assert_eq!(UPSTREAM_INSTRUCTIONS_PARAM, Some("developerInstructions"));
    }

    #[test]
    fn the_lico_up_owned_facts_stay_product_decisions() {
        // The card has no notion of LicoUp's packaged protocol name or its
        // MCP mechanism for the Subagent connector; both stay local and must
        // not be silently replaced by a card fact.
        assert_eq!(RUNTIME_PROTOCOL, "codex-app-server-stdio-jsonrpc");
        assert_eq!(UPSTREAM_MCP_FILE, None);
    }
}
