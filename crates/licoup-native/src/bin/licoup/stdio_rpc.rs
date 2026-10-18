//! Bounded stdio RPC composition facade.
//!
//! Session dispatch, request decoding, line framing, response encoding, and
//! error projection are separate leaves so each protocol responsibility can be
//! changed and tested without coupling it to the CLI entry point.

use super::*;

#[path = "stdio_rpc/context.rs"]
mod context;
#[path = "stdio_rpc/error.rs"]
mod error;
#[path = "stdio_rpc/frame_lane.rs"]
mod frame_lane;
#[path = "stdio_rpc/line.rs"]
mod line;
#[path = "stdio_rpc/model.rs"]
mod model;
#[path = "stdio_rpc/request.rs"]
mod request;
#[path = "stdio_rpc/response.rs"]
mod response;
#[path = "stdio_rpc/selection.rs"]
mod selection;
#[path = "stdio_rpc/server.rs"]
mod server;

pub(super) use context::*;
pub(super) use error::*;
pub(super) use frame_lane::*;
pub(super) use line::*;
pub(super) use model::*;
pub(super) use request::*;
pub(super) use response::*;
pub(super) use selection::*;
pub(super) use server::*;
