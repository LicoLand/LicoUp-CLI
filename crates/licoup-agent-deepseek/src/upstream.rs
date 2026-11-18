//! The published A4AA DeepSeek Harness dialect this package compiles against.
//!
//! `a4aa-dsh` owns the standard description of the DeepSeek Harness: its launch
//! program and SDK argv, its `--profile sdk` RPC vocabulary, its mechanism
//! surfaces and its persisted session store. This package used to retype the
//! launch argv and the JSON-RPC method names beside its own parser and
//! transport pool, which meant one card change could leave the two answers
//! disagreeing. `build.rs` now projects the published card into the constants
//! below, and the package reads them instead of keeping a second copy.
//!
//! What stays LicoUp-owned is the product mapping: the adapter id this client
//! publishes (the card's catalog id is `dsh`), the packaged runtime protocol
//! name, the process-scoped transport pool keyed by caller session, the
//! initialization client identity, the durable Zstandard session-log reader,
//! and the synchronous supervised process. Those are LicoUp decisions, not
//! facts about the Harness.
//!
//! The projection is build-time rather than a runtime dependency because
//! `a4aa`'s engine is async: this package is driven from the client's
//! synchronous process supervisor, and linking the engine here would replace
//! that owner instead of converging the dialect.

include!(concat!(env!("OUT_DIR"), "/deepseek_dialect.rs"));

/// The adapter id LicoUp publishes for this Agent.
///
/// The published card names the vendor catalog id `dsh`; LicoUp publishes
/// `deepseek-harness`. That is a product spelling, not a card fact, so the
/// mapping is declared here and asserted in
/// `tests::the_published_card_maps_to_the_lico_up_adapter`.
pub const ADAPTER_ID: &str = "deepseek-harness";

/// The runtime protocol LicoUp packages and reports for this adapter.
pub const RUNTIME_PROTOCOL: &str = "deepseek-harness-sdk-stdio-jsonrpc";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::driver::EffectiveSettings;
    use crate::parser::{initialize_request, prompt_request, shutdown_request};

    #[test]
    fn the_published_card_maps_to_the_lico_up_adapter() {
        assert_eq!(UPSTREAM_CARD_ID, "dsh");
        assert_eq!(ADAPTER_ID, "deepseek-harness");
        assert_eq!(crate::registration::ADAPTER_ID, ADAPTER_ID);
        assert_eq!(UPSTREAM_LANE_KIND, "rpc");
        assert_eq!(UPSTREAM_PROGRAM, "dsh");
        assert_eq!(crate::driver::RUNTIME_PROTOCOL, RUNTIME_PROTOCOL);
    }

    #[test]
    fn the_launch_args_are_the_published_sdk_profile() {
        assert_eq!(UPSTREAM_LAUNCH_ARGS, ["--profile", "sdk"]);
        assert_eq!(UPSTREAM_SESSION_SCOPE, "process");
        const { assert!(!UPSTREAM_HAS_CANCEL) };
    }

    #[test]
    fn the_parser_speaks_the_published_rpc_vocabulary() {
        let initialize = initialize_request("/work", "deepseek", "deepseek-v4", None, None);
        assert_eq!(initialize["method"], UPSTREAM_INITIALIZE_METHOD);
        let prompt = prompt_request("1", "session-1", "hello");
        assert_eq!(prompt["method"], UPSTREAM_PROMPT_METHOD);
        let shutdown = shutdown_request();
        assert_eq!(shutdown["method"], UPSTREAM_SHUTDOWN_METHOD);
        assert_eq!(UPSTREAM_TEXT_EVENT_METHOD, "session.event");
        assert_eq!(UPSTREAM_COMPLETION_METHOD, "session.status");
        assert_eq!(UPSTREAM_COMPLETION_STATUS, "idle");
        assert_eq!(UPSTREAM_TEXT_PATH, "event.data.message.content");
        assert_eq!(UPSTREAM_TEXT_POINTER, "/params/event/data/message/content");
    }

    #[test]
    fn the_lico_up_owned_facts_stay_product_decisions() {
        // The card names no permission preset for the SDK lane; LicoUp's
        // product selector is its own vocabulary and must not be projected
        // from an empty card set.
        assert!(UPSTREAM_PERMISSION_MODES.is_empty());
        assert_eq!(EffectiveSettings::default().permission_mode, None);
        assert_eq!(RUNTIME_PROTOCOL, "deepseek-harness-sdk-stdio-jsonrpc");
    }
}
