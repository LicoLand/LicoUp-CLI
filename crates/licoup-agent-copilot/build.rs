//! Project the published `a4aa-copilot` card and mechanism dialect into the
//! constants this package compiles against.
//!
//! This is a build-time bridge, not a second declaration: the values are read
//! from the published crate on every build, and no edited copy lives here. The
//! dependency stays in the build graph so `a4aa`'s ACP runtime feature set
//! cannot turn on `serde_json/preserve_order` for the client's runtime.

use a4aa::LaneSpec;
use a4aa::card::AgentSurface;
use a4aa::dialect::{HookSurface, McpSurface, SubagentSurface};
use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    let card = a4aa_copilot::card();
    let spec = a4aa_copilot::spec();
    let dialect = a4aa_copilot::dialect();

    let AgentSurface::Acp { spawn_args, .. } = &card.surface else {
        panic!("the published Copilot card's primary lane is not ACP");
    };
    assert_eq!(
        spec.command.args.len(),
        spawn_args.len(),
        "the card and its materialized spec disagree on the ACP launch"
    );
    for (arg, expected) in spec.command.args.iter().zip(spawn_args.iter()) {
        assert_eq!(arg, expected, "the card and spec disagree on a launch arg");
    }

    let subagents = match dialect.subagents {
        SubagentSurface::MarkdownDir(dir) => Some(dir),
        SubagentSurface::None | SubagentSurface::CodexToml(_) | SubagentSurface::KiroJson(_) => {
            None
        }
    };
    let (mcp_file, mcp_adapted) = match dialect.mcp {
        McpSurface::JsonMerge { file, adapted, .. } => (Some(file), adapted),
        McpSurface::None => (None, false),
    };

    let mut generated = String::new();
    writeln!(
        generated,
        "pub(crate) const UPSTREAM_CARD_ID: &str = {:?};",
        card.id
    )
    .unwrap();
    writeln!(
        generated,
        "pub(crate) const UPSTREAM_CARD_LABEL: &str = {:?};",
        card.label
    )
    .unwrap();
    writeln!(
        generated,
        "pub(crate) const UPSTREAM_PROGRAM: &str = {:?};",
        spec.command.program
    )
    .unwrap();
    generated.push_str("pub(crate) const UPSTREAM_LAUNCH_ARGS: &[&str] = &[");
    for arg in &spec.command.args {
        write!(generated, "{arg:?}, ").unwrap();
    }
    generated.push_str("];\n");
    writeln!(
        generated,
        "pub(crate) const UPSTREAM_SURFACE_IS_ACP: bool = {:?};",
        matches!(spec.lane, LaneSpec::Acp { .. })
    )
    .unwrap();
    writeln!(
        generated,
        "pub(crate) const UPSTREAM_HOOKS_NONE: bool = {:?};",
        matches!(dialect.hooks, HookSurface::None)
    )
    .unwrap();
    writeln!(
        generated,
        "pub(crate) const UPSTREAM_SKILLS_DIR: Option<&str> = {:?};",
        dialect.skills_dir
    )
    .unwrap();
    writeln!(
        generated,
        "pub(crate) const UPSTREAM_SUBAGENTS_DIR: Option<&str> = {subagents:?};"
    )
    .unwrap();
    writeln!(
        generated,
        "pub(crate) const UPSTREAM_RULES_FILE: Option<&str> = {:?};",
        dialect.rules.map(|target| target.path)
    )
    .unwrap();
    writeln!(
        generated,
        "pub(crate) const UPSTREAM_MCP_FILE: Option<&str> = {mcp_file:?};"
    )
    .unwrap();
    writeln!(
        generated,
        "pub(crate) const UPSTREAM_MCP_ADAPTED: bool = {mcp_adapted:?};"
    )
    .unwrap();

    let out = PathBuf::from(env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
    fs::write(out.join("copilot_card.rs"), generated).expect("write generated card facts");
}
