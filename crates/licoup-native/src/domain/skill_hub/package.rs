use anyhow::{Result, anyhow, ensure};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Component, Path, PathBuf};

const MAX_SKILL_FILES: usize = 2048;
pub(super) const MAX_SKILL_BYTES: u64 = 64 * 1024 * 1024;
const MAX_SKILL_DEPTH: usize = 32;
const MAX_SKILL_MANIFEST_BYTES: u64 = 256 * 1024;

#[derive(Clone, Debug)]
pub(super) struct SkillPackagePreview {
    pub(super) skill_id: String,
    pub(super) title: String,
    pub(super) description: String,
    pub(super) version: String,
    pub(super) digest_sha256: String,
    pub(super) file_count: usize,
}

pub(super) fn inspect_skill_dir(path: &Path) -> Result<SkillPackagePreview> {
    let root_metadata =
        fs::symlink_metadata(path).map_err(|_| anyhow!("skill_package_path_unavailable"))?;
    ensure!(
        root_metadata.file_type().is_dir() && !root_metadata.file_type().is_symlink(),
        "skill_package_path_invalid"
    );
    let skill_md = path.join("SKILL.md");
    let metadata =
        fs::symlink_metadata(&skill_md).map_err(|_| anyhow!("skill_package_manifest_missing"))?;
    ensure!(
        metadata.file_type().is_file()
            && !metadata.file_type().is_symlink()
            && metadata.len() <= MAX_SKILL_MANIFEST_BYTES,
        "skill_package_manifest_invalid"
    );
    let mut manifest_bytes = Vec::with_capacity(metadata.len() as usize);
    File::open(&skill_md)?
        .take(MAX_SKILL_MANIFEST_BYTES + 1)
        .read_to_end(&mut manifest_bytes)?;
    ensure!(
        manifest_bytes.len() as u64 <= MAX_SKILL_MANIFEST_BYTES,
        "skill_package_manifest_too_large"
    );
    let content = std::str::from_utf8(&manifest_bytes)
        .map_err(|_| anyhow!("skill_package_manifest_utf8_invalid"))?;
    let manifest = parse_skill_metadata(content);
    let fallback_id = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("skill");
    let skill_id = sanitize_skill_id(
        manifest
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(fallback_id),
    )?;
    let title = manifest
        .get("title")
        .or_else(|| manifest.get("name"))
        .and_then(Value::as_str)
        .unwrap_or(&skill_id)
        .to_string();
    let description = manifest
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let version = manifest
        .get("version")
        .and_then(Value::as_str)
        .unwrap_or("local")
        .to_string();
    let files = collect_regular_files(path)?;
    Ok(SkillPackagePreview {
        skill_id,
        title,
        description,
        version,
        digest_sha256: digest_files(path, &files)?,
        file_count: files.len(),
    })
}

pub(super) fn sanitize_skill_id(value: &str) -> Result<String> {
    let id = value
        .trim()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    ensure!(!id.is_empty(), "skill_id_invalid");
    Ok(id)
}

pub(super) fn collect_regular_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::<PathBuf>::new();
    let mut total_bytes = 0u64;
    collect_regular_files_into(root, root, 0, &mut files, &mut total_bytes)?;
    ensure!(!files.is_empty(), "skill_package_empty");
    files.sort();
    Ok(files)
}

fn collect_regular_files_into(
    root: &Path,
    current: &Path,
    depth: usize,
    files: &mut Vec<PathBuf>,
    total_bytes: &mut u64,
) -> Result<()> {
    ensure!(depth <= MAX_SKILL_DEPTH, "skill_package_depth_exceeded");
    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        ensure!(
            !metadata.file_type().is_symlink(),
            "skill_package_symlink_rejected"
        );
        if metadata.is_dir() {
            if entry.file_name().to_string_lossy() == ".git" {
                continue;
            }
            collect_regular_files_into(root, &path, depth + 1, files, total_bytes)?;
            continue;
        }
        if metadata.is_file() {
            ensure!(
                files.len() < MAX_SKILL_FILES,
                "skill_package_file_limit_exceeded"
            );
            *total_bytes = total_bytes
                .checked_add(metadata.len())
                .ok_or_else(|| anyhow!("skill_package_too_large"))?;
            ensure!(*total_bytes <= MAX_SKILL_BYTES, "skill_package_too_large");
            let relative = path.strip_prefix(root)?.to_path_buf();
            validate_relative_path(&relative)?;
            files.push(relative);
        }
    }
    Ok(())
}

pub(super) fn validate_relative_path(path: &Path) -> Result<()> {
    ensure!(
        !path.as_os_str().is_empty()
            && !path.is_absolute()
            && path
                .components()
                .all(|part| matches!(part, Component::Normal(_))),
        "skill_package_path_invalid"
    );
    Ok(())
}

fn digest_files(root: &Path, files: &[PathBuf]) -> Result<String> {
    let mut hasher = Sha256::new();
    let mut total_bytes = 0u64;
    for relative in files {
        validate_relative_path(relative)?;
        let path = root.join(relative);
        let metadata = fs::symlink_metadata(&path)?;
        ensure!(
            metadata.file_type().is_file() && !metadata.file_type().is_symlink(),
            "skill_package_file_invalid"
        );
        let remaining = MAX_SKILL_BYTES.saturating_sub(total_bytes);
        let mut bytes = Vec::with_capacity(metadata.len().min(remaining) as usize);
        File::open(&path)?
            .take(remaining + 1)
            .read_to_end(&mut bytes)?;
        total_bytes = total_bytes
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| anyhow!("skill_package_too_large"))?;
        ensure!(total_bytes <= MAX_SKILL_BYTES, "skill_package_too_large");
        hasher.update((relative.as_os_str().len() as u64).to_be_bytes());
        hasher.update(relative.to_string_lossy().as_bytes());
        hasher.update((bytes.len() as u64).to_be_bytes());
        hasher.update(bytes);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn parse_skill_metadata(content: &str) -> Value {
    let mut lines = content.lines();
    if lines.next().map(str::trim) != Some("---") {
        return json!({});
    }
    let mut metadata = serde_json::Map::new();
    for line in lines {
        let trimmed = line.trim();
        if trimmed == "---" {
            break;
        }
        let Some((key, value)) = trimmed.split_once(':') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        metadata.insert(
            key.to_string(),
            json!(value.trim().trim_matches('"').trim_matches('\'')),
        );
    }
    Value::Object(metadata)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn skill_id_normalization_is_stable_and_non_empty() {
        assert_eq!(
            sanitize_skill_id(" Review Helper ").unwrap(),
            "review-helper"
        );
        assert!(sanitize_skill_id("---").is_err());
    }

    #[test]
    fn package_inspection_is_bounded_and_requires_a_manifest() {
        let root =
            std::env::temp_dir().join(format!("lico-skill-package-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        assert!(inspect_skill_dir(&root).is_err());
        fs::write(
            root.join("SKILL.md"),
            "---\nname: review-helper\nversion: 1\n---\n",
        )
        .unwrap();
        let preview = inspect_skill_dir(&root).unwrap();
        assert_eq!(preview.skill_id, "review-helper");
        assert_eq!(preview.file_count, 1);
        fs::remove_dir_all(root).unwrap();
    }
}
