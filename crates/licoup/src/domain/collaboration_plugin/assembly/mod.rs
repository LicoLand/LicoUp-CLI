mod apply;
mod cleanup;
mod manifest;
mod model;
mod payload_inventory;
mod runtime;
mod snapshot;
mod store;
mod transaction;

#[cfg(test)]
mod tests;

pub(super) const ASSEMBLY_ADAPTER_ID: &str = "licoup-builtin-local-http-v1";
pub(super) const ASSEMBLY_MANIFEST_FILE: &str = "licoup-assembly.json";
pub(super) const ASSEMBLY_MANIFEST_SCHEMA: &str = "licoup.local-server-assembly.v3";
pub(super) const ASSEMBLY_SNAPSHOT_FILE: &str = "licoup-sealed-snapshot.bin";
pub(super) const ASSEMBLY_STATE_SCHEMA: &str = "licoup.local-server-state.v4";
pub(super) const ASSEMBLED_RUNNER_DIRECTORY: &str = "runner";
pub(super) const ASSEMBLED_RUNTIME_DATA_DIRECTORY: &str = "runtime-data";

pub(super) use apply::{
    apply_local_assembly, plan_local_assembly, plan_projection, record_projection,
};
pub(super) use model::{LocalAssemblyRecord, PlannedLocalAssembly};
pub(super) use runtime::{has_assemblies, start, status, stop, stop_all, uninstall};
