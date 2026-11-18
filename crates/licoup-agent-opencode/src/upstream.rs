//! The published A4AA OpenCode dialect this package compiles against.
//!
//! `a4aa-opencode` owns the standard description of OpenCode: its `serve`
//! launch, its ordered ACP alternate, its mechanism surfaces and its live
//! usage event. This package used to retype the serve subcommand beside its own
//! endpoint policy, which meant one card change could leave the two answers
//! disagreeing. `build.rs` now projects the published card into the constants
//! below, and the package reads them instead of keeping a second copy.
//!
//! What stays LicoUp-owned is the product mapping: the adapter id this client
//! publishes, the packaged runtime protocol name, the endpoint identity, its
//! preferred port and port scan, the reserved-port list, the state record, the
//! health/session/config/provider paths and the failure codes. Those are
//! LicoUp's serve policy, not facts about OpenCode; the card names the program
//! and the subcommand, and the client's engine owns how it binds.
//!
//! The projection is build-time rather than a runtime dependency because
//! `a4aa`'s engine is async: this package is driven from the client's
//! synchronous serve engine, and linking the engine here would replace that
//! owner instead of converging the dialect.

include!(concat!(env!("OUT_DIR"), "/opencode_dialect.rs"));

/// The adapter id LicoUp publishes for this Agent.
pub const ADAPTER_ID: &str = "opencode";

/// The runtime protocol LicoUp packages and reports for this adapter.
pub const RUNTIME_PROTOCOL: &str = "opencode-serve-http-v1";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::driver::OPENCODE_DRIVER;
    use crate::policy;

    #[test]
    fn the_published_card_is_the_lico_up_adapter() {
        assert_eq!(UPSTREAM_CARD_ID, ADAPTER_ID);
        assert_eq!(crate::registration::ADAPTER_ID, ADAPTER_ID);
        assert_eq!(UPSTREAM_LANE_KIND, "serve");
        assert_eq!(UPSTREAM_PROGRAM, "opencode");
    }

    #[test]
    fn the_endpoint_launches_the_published_subcommand() {
        assert_eq!(UPSTREAM_SERVE_SUBCOMMAND, "serve");
        assert_eq!(OPENCODE_DRIVER.launch_args, [UPSTREAM_SERVE_SUBCOMMAND]);
        assert_eq!(OPENCODE_DRIVER.runtime_protocol, RUNTIME_PROTOCOL);
        assert_eq!(policy::SPEC.default_executable, UPSTREAM_PROGRAM);
    }

    #[test]
    fn the_published_lane_order_is_serve_then_acp() {
        assert_eq!(UPSTREAM_SERVE_SUBCOMMAND, "serve");
        assert_eq!(UPSTREAM_ACP_SUBCOMMAND, "acp");
        const { assert!(UPSTREAM_ACP_PREFER_LOAD) };
    }

    #[test]
    fn the_lico_up_serve_policy_is_not_a_card_fact() {
        // The card names `serve --port 0`; LicoUp's engine picks the port and
        // passes an explicit host, so the rest of the argv must stay local.
        assert_eq!(UPSTREAM_SERVE_SPAWN_ARGS, ["serve", "--port", "0"]);
        assert_ne!(
            UPSTREAM_SERVE_SPAWN_ARGS.len(),
            OPENCODE_DRIVER.launch_args.len(),
            "the driver declares only the subcommand, not the card's port template"
        );
        assert_eq!(policy::SPEC.identity, "opencode_serve");
        assert_eq!(policy::SPEC.default_port, 24173);
    }

    #[test]
    fn the_published_mechanism_surfaces_are_full_paths() {
        assert_eq!(UPSTREAM_SKILLS_DIR, Some(".opencode/skills"));
        assert_eq!(UPSTREAM_RULES_FILE, Some("AGENTS.md"));
        assert_eq!(UPSTREAM_MCP_FILE, Some(".config/opencode/opencode.json"));
        assert_eq!(UPSTREAM_MCP_KEY, Some("mcp"));
        const { assert!(UPSTREAM_MCP_ADAPTED) };
        assert_eq!(UPSTREAM_LIVE_USAGE_EVENT, Some("session/update"));
    }
}
