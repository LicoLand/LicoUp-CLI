//! Production orchestration for directly approved MCP transfers.
//!
//! The domain layer owns exact-scope preview/approval binding and protocol
//! response validation. HTTP I/O is injected by the FFI composition root so
//! this module depends only on the service-neutral MCP core.

mod approval;
mod execution;
mod plan;
mod sse;

pub use execution::{McpHttpTransportResponse, execute_http_transfer, preview_http_transfer};
pub use plan::McpApprovalPlanStore;

#[cfg(test)]
mod tests;
