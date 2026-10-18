//! Desktop Agent Hub: warehouse Manifest plus one TOML recipe per agent.

pub(crate) mod acquisition;
pub(crate) mod argv;
pub(crate) mod capabilities;
pub(crate) mod catalog;
pub(crate) mod confirmation;
pub(crate) mod contract;
pub(crate) mod engine;
pub(crate) mod ownership;
pub(crate) mod package_versions;
pub(crate) mod recipes;
pub(crate) mod selector;
pub(crate) mod version;
pub(crate) mod version_check;

use anyhow::Result;
use serde_json::Value;

pub use contract::{
    AgentRecipe, ArtifactIntegrity, ArtifactSpec, InstallChannel, InstallPlacement,
    PlatformInstallCapabilities, RecipeRegistryDocument, SCHEMA_VERSION, contract_surface,
};
pub use engine::{HubContext, apply, apply_with, plan, plan_with};
pub use recipes::{manifest, registry};

pub fn catalog(params: &Value) -> Result<Value> {
    catalog::catalog(params)
}

pub fn install_plan(params: &Value) -> Result<Value> {
    engine::plan(params)
}

pub fn install_apply(params: &Value) -> Result<Value> {
    engine::apply(params)
}

pub fn update_plan(params: &Value) -> Result<Value> {
    let mut next = params.clone();
    next["operation"] = Value::from("update");
    engine::plan(&next)
}

pub fn update_apply(params: &Value) -> Result<Value> {
    let mut next = params.clone();
    next["operation"] = Value::from("update");
    engine::apply(&next)
}

pub fn uninstall_plan(params: &Value) -> Result<Value> {
    let mut next = params.clone();
    next["operation"] = Value::from("uninstall");
    engine::plan(&next)
}

pub fn uninstall_apply(params: &Value) -> Result<Value> {
    let mut next = params.clone();
    next["operation"] = Value::from("uninstall");
    engine::apply(&next)
}

#[cfg(test)]
mod tests;
