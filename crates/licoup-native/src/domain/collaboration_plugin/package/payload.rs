use super::inspection::validate_relative_components;
use super::{InspectedPackage, SelectedPayloadFile, WorkflowChoice};
use anyhow::{Result, anyhow, ensure};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::path::PathBuf;

pub(in crate::domain::collaboration_plugin) fn selected_payload_files(
    package: &InspectedPackage,
    choices: &[WorkflowChoice],
    selected_ids: &[String],
    namespace_by_selection: bool,
) -> Result<Vec<SelectedPayloadFile>> {
    ensure!(
        !selected_ids.is_empty() && selected_ids.len() <= choices.len(),
        "collaboration_plugin_workflow_selection_required"
    );
    let choices_by_id = choices
        .iter()
        .map(|choice| (choice.id.as_str(), choice))
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut destinations = BTreeSet::new();
    let mut selected = Vec::new();
    for selection_id in selected_ids {
        let choice = choices_by_id
            .get(selection_id.as_str())
            .ok_or_else(|| anyhow!("collaboration_plugin_workflow_selection_unknown"))?;
        let mut matched = 0usize;
        for file in &package.files {
            let relative = if file.relative_path == choice.package_path {
                PathBuf::from(file.relative_path.file_name().ok_or_else(|| {
                    anyhow!("collaboration_plugin_workflow_package_payload_invalid")
                })?)
            } else if file.relative_path.starts_with(&choice.package_path) {
                file.relative_path
                    .strip_prefix(&choice.package_path)
                    .map_err(|_| anyhow!("collaboration_plugin_workflow_package_payload_invalid"))?
                    .to_path_buf()
            } else {
                continue;
            };
            validate_relative_components(&relative)?;
            let destination_relative_path = if namespace_by_selection {
                PathBuf::from(selection_id).join(&relative)
            } else {
                relative
            };
            validate_relative_components(&destination_relative_path)?;
            ensure!(
                destinations.insert(destination_relative_path.clone()),
                "collaboration_plugin_workflow_destination_collision"
            );
            selected.push(SelectedPayloadFile {
                selection_id: selection_id.clone(),
                source_relative_path: file.relative_path.clone(),
                destination_relative_path,
                digest_sha256: format!("{:x}", Sha256::digest(&file.bytes)),
                bytes: file.bytes.clone(),
            });
            matched += 1;
        }
        ensure!(
            matched > 0,
            "collaboration_plugin_workflow_package_payload_missing"
        );
    }
    selected.sort_by(|left, right| {
        left.destination_relative_path
            .cmp(&right.destination_relative_path)
    });
    Ok(selected)
}
