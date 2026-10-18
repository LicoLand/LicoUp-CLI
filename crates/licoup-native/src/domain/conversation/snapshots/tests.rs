use super::*;
use crate::domain::conversation::snapshot_codec::export_jsonl_source;
use rusqlite::Connection;
use std::env;
use std::sync::atomic::{AtomicU64, Ordering};

static TEMP_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

mod discovery;
mod materialization;
mod orchestration;
mod privacy_projection;
mod relocation;
mod reporting;
mod selection;
mod selection_plan;
mod settings;
mod support;
mod test_support;
mod validation;

use test_support::*;

#[test]
fn split_snapshot_module_composition_keeps_the_public_facade() {
    let facade = include_str!("../../conversation_snapshots.rs");
    assert!(facade.lines().count() < 100);
    assert!(facade.contains("conversation::snapshots::archive_collect"));
    assert_eq!(
        COLLECTION_SCHEMA_VERSION,
        "v0.0.1:agent:native-conversation-snapshot-1"
    );
}
