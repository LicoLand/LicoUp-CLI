use super::*;
use serde_json::{Value, json};
use std::env;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[path = "tests/core_commands.rs"]
mod core_commands;
#[path = "tests/parsing.rs"]
mod parsing;
#[path = "tests/rpc.rs"]
mod rpc;
#[path = "tests/skill_commands.rs"]
mod skill_commands;
#[path = "tests/support.rs"]
mod support;
