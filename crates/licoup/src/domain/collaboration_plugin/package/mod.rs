mod descriptor;
mod inspection;
mod inventory;
mod payload;
mod runner;
mod secure_file;
mod writer;

#[cfg(test)]
mod tests;

use super::manifest::ValidatedManifest;
use std::path::PathBuf;

#[derive(Clone)]
pub(super) struct PackageFile {
    pub(super) relative_path: PathBuf,
    pub(super) bytes: Vec<u8>,
}

#[derive(Clone)]
pub(super) struct InspectedPackage {
    pub manifest: ValidatedManifest,
    pub digest_sha256: String,
    pub file_count: usize,
    pub total_bytes: usize,
    pub(super) files: Vec<PackageFile>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct WorkflowChoice {
    pub(super) id: String,
    pub(super) package_path: PathBuf,
    pub(super) endpoint: Option<String>,
}

#[derive(Clone)]
pub(super) struct SelectedPayloadFile {
    pub(super) selection_id: String,
    pub(super) source_relative_path: PathBuf,
    pub(super) destination_relative_path: PathBuf,
    pub(super) digest_sha256: String,
    pub(super) bytes: Vec<u8>,
}

pub(super) use descriptor::{local_deployment_choices, mcp_install_choices};
pub(super) use inspection::inspect_package;
#[cfg(test)]
pub(super) use inventory::signed_inventory_digest;
pub(super) use payload::selected_payload_files;
pub(super) use runner::{
    SelectedServerRunner, select_current_server_runner, sha256 as runner_sha256,
};
pub(super) use secure_file::read_file_no_follow;
#[cfg(test)]
pub(super) use writer::write_inspected_package_with_hook;
pub(super) use writer::{
    SecureNewTree, open_directory_path_no_follow, write_inspected_package,
    write_selected_payload_tree,
};
