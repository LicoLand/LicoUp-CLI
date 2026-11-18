mod apply_local;
mod apply_mcp;
mod cancel;
mod destination_policy;
mod package_revalidation;
mod plan_local;
mod plan_mcp;
mod projection;
mod staging;
mod validation;

pub(super) use apply_local::local_deployment_apply;
pub(super) use apply_mcp::mcp_install_apply;
pub(super) use cancel::workflow_cancel;
pub(super) use plan_local::local_deployment_plan;
pub(super) use plan_mcp::mcp_install_plan;

#[cfg(test)]
mod tests;
