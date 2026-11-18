//! The published A4AA Claude dialect this package compiles against.
//!
//! `a4aa-claude` owns the standard description of Claude Code: its launch
//! program, its stream-json argv template, its permission presets, its
//! mechanism surfaces and its persisted transcript shape. This package used to
//! retype the fixed argv prefix and the permission preset set beside its own
//! driver, which meant one card change could leave the two answers disagreeing.
//! `build.rs` now projects the published card into the constants below, and the
//! package reads them instead of keeping a second copy.
//!
//! What stays LicoUp-owned is the product mapping: the adapter id this client
//! publishes, the packaged runtime protocol name, the launch default when the
//! caller selects no permission mode, the `--allowedTools` retry channel this
//! client adds for an approved retry, the private system-prompt guidance path,
//! the session registry and the synchronous supervised process. Those are
//! LicoUp decisions, not facts about Claude Code.
//!
//! The projection is build-time rather than a runtime dependency because
//! `a4aa`'s engine is async: this package is driven from the client's
//! synchronous process supervisor, and linking the engine here would replace
//! that owner instead of converging the dialect.

include!(concat!(env!("OUT_DIR"), "/claude_dialect.rs"));

/// The adapter id LicoUp publishes for this Agent.
///
/// The published card names the vendor catalog id `claude`; LicoUp publishes
/// `claude-code`. That is a product spelling, not a card fact, so the mapping
/// is declared here and asserted in `tests::the_published_card_maps_to_the_lico_up_adapter`.
pub const ADAPTER_ID: &str = "claude-code";

/// The runtime protocol LicoUp packages and reports for this adapter.
pub const RUNTIME_PROTOCOL: &str = "claude-code-cli-stream-json";

/// The launch default LicoUp applies when a caller selects no permission mode.
///
/// The published card leaves `permission_default` empty so a host does not
/// silently pick a posture for the user. LicoUp's own product decision has been
/// the vendor YOLO mode for an unpinned launch; it stays here as a LicoUp fact
/// and is asserted to be one of the card's accepted presets.
pub const DEFAULT_PERMISSION_MODE: &str = "bypassPermissions";

/// The retry channel LicoUp adds on top of the card's launch template.
///
/// The card declares no allowed-tools flag; LicoUp passes one after an approved
/// permission retry so the same denial is not raised twice. It is a product
/// decision and deliberately not derived from the card.
pub const ALLOWED_TOOLS_FLAG: &str = "--allowedTools";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::launch::LaunchIdentity;
    use crate::protocol::params::DriverConfig;

    #[test]
    fn the_published_card_maps_to_the_lico_up_adapter() {
        assert_eq!(UPSTREAM_CARD_ID, "claude");
        assert_eq!(ADAPTER_ID, "claude-code");
        assert_eq!(crate::registration::ADAPTER_ID, ADAPTER_ID);
        assert_eq!(UPSTREAM_LANE_KIND, "cli");
        assert_eq!(UPSTREAM_PROGRAM, "claude");
    }

    #[test]
    fn the_fixed_stream_prefix_is_the_published_prefix() {
        assert_eq!(
            crate::protocol::launch::FIXED_STREAM_ARGS,
            UPSTREAM_FIXED_ARGS
        );
        assert!(UPSTREAM_FIXED_ARGS.contains(&"--print"));
        assert!(UPSTREAM_FIXED_ARGS.contains(&"--include-partial-messages"));
    }

    #[test]
    fn the_stream_payload_is_the_published_template() {
        assert_eq!(UPSTREAM_SESSION_ID_FIELD, "session_id");
        assert_eq!(UPSTREAM_RESULT_SELECTOR, "result");
        assert_eq!(UPSTREAM_RESULT_FIELD, "result");
        assert!(UPSTREAM_STDIN_TEMPLATE.contains("{prompt}"));
        let expected: serde_json::Value =
            serde_json::from_str(&UPSTREAM_STDIN_TEMPLATE.replace("{prompt}", "hello"))
                .expect("the published stdin template is JSON");
        assert_eq!(crate::protocol::parser::user_message("hello"), expected);
    }

    #[test]
    fn the_launch_mapping_uses_the_published_flags() {
        let config = DriverConfig::from_params(
            &serde_json::json!({
                "model": "claude-sonnet-4-5",
                "reasoningEffort": "high",
                "permissionMode": "plan",
                "privateInstructions": "stay exact",
            }),
            "hello",
            "session-1",
            None,
        )
        .expect("the fixture request is valid");
        let identity = LaunchIdentity::new(UPSTREAM_PROGRAM, &config, None);
        let args = identity.args();
        assert!(args.contains(&UPSTREAM_MODEL_FLAG.to_string()));
        assert!(args.contains(&UPSTREAM_EFFORT_FLAG.to_string()));
        assert!(args.contains(&UPSTREAM_PERMISSION_FLAG.to_string()));
        assert!(args.contains(&UPSTREAM_INSTRUCTIONS_FLAG.to_string()));
        assert!(args.contains(&UPSTREAM_RESUME_FLAG.to_string()));
        assert!(!args.contains(&ALLOWED_TOOLS_FLAG.to_string()));
    }

    #[test]
    fn every_published_permission_mode_is_admitted_and_nothing_else_is() {
        for mode in UPSTREAM_PERMISSION_MODES {
            DriverConfig::from_params(
                &serde_json::json!({ "permissionMode": mode }),
                "hello",
                "",
                None,
            )
            .unwrap_or_else(|failure| {
                panic!("published permission mode {mode} was refused: {failure:?}")
            });
        }
        assert!(
            UPSTREAM_PERMISSION_MODES.contains(&DEFAULT_PERMISSION_MODE),
            "the LicoUp default must be a preset the published card accepts"
        );
        assert!(
            DriverConfig::from_params(
                &serde_json::json!({ "permissionMode": "not-a-published-mode" }),
                "hello",
                "",
                None,
            )
            .is_err()
        );
    }
}
