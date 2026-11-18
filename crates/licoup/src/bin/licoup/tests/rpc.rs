//! Responsibility-level stdio RPC regression composition.

use super::super::stdio_rpc::*;
use super::*;

#[path = "../../../../tests/fixtures/claude_process_local_test_lock.rs"]
mod claude_process_local_test_lock;

#[path = "rpc/error.rs"]
mod error;
#[path = "rpc/line.rs"]
mod line;
#[path = "rpc/process_local.rs"]
mod process_local;
#[path = "rpc/request.rs"]
mod request;
#[path = "rpc/response.rs"]
mod response;
#[path = "rpc/selection.rs"]
mod selection;
#[path = "rpc/server.rs"]
mod server;
#[path = "rpc/state.rs"]
mod state;
