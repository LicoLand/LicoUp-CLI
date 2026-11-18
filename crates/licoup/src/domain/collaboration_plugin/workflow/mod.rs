mod commit;
mod mcp_transaction;
mod model;
mod operations;
mod store;

#[cfg(test)]
mod tests;

use anyhow::Result;
use serde_json::Value;

pub fn local_deployment_plan(params: &Value) -> Result<Value> {
    operations::local_deployment_plan(params)
}

pub fn local_deployment_apply(params: &Value) -> Result<Value> {
    operations::local_deployment_apply(params)
}

pub fn mcp_install_plan(params: &Value) -> Result<Value> {
    operations::mcp_install_plan(params)
}

pub fn mcp_install_apply(params: &Value) -> Result<Value> {
    operations::mcp_install_apply(params)
}

pub fn cancel(params: &Value) -> Result<Value> {
    operations::workflow_cancel(params)
}

pub(super) fn commit_directory_no_replace(
    source: &std::path::Path,
    destination: &std::path::Path,
) -> Result<()> {
    commit::commit_directory_no_replace(source, destination)
}
