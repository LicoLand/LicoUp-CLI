//! Rebase app-owned snapshot metadata in an owned copy, including unpublished staging.

use super::*;
use std::ffi::OsStr;

pub(crate) fn relocate_copied_data_home_references(
    copied_data_root: &Path,
    previous_data_root: &Path,
    new_data_root: &Path,
) -> Result<()> {
    let copied_root = lexical_absolute(copied_data_root)
        .ok_or_else(|| anyhow!("copied data root must be absolute"))?;
    let previous_root = lexical_absolute(previous_data_root)
        .ok_or_else(|| anyhow!("previous data root must be absolute"))?;
    let new_root =
        lexical_absolute(new_data_root).ok_or_else(|| anyhow!("new data root must be absolute"))?;
    ensure!(
        copied_directory(&copied_root, &copied_root).is_some(),
        "copied data root is not a regular directory"
    );
    if previous_root == new_root {
        return Ok(());
    }

    let store = ClientStateStore::new(copied_root.join("client-state"))?;
    let mut root_candidates = BTreeSet::<PathBuf>::new();
    root_candidates.insert(store.root().join(DEFAULT_SNAPSHOT_ROOT_DIR));

    let mut settings = store.read_collection(SETTINGS_COLLECTION)?;
    let mut settings_changed = false;
    for key in ["conversationSnapshotRoot", "snapshotRoot"] {
        settings_changed |=
            relocate_path_field(&mut settings, key, &previous_root, &new_root, &copied_root);
        collect_root_locator(
            &settings,
            key,
            &copied_root,
            &new_root,
            &mut root_candidates,
        );
    }
    if settings_changed {
        store.write_collection(SETTINGS_COLLECTION, settings)?;
    }

    let mut profiles = store.read_collection(PROFILES_COLLECTION)?;
    let mut profiles_changed = false;
    if let Some(items) = profiles.get_mut("items").and_then(Value::as_array_mut) {
        for profile in items {
            profiles_changed |= relocate_path_field(
                profile,
                "archiveRoot",
                &previous_root,
                &new_root,
                &copied_root,
            );
            profiles_changed |= relocate_path_field(
                profile,
                "baselineIndexPath",
                &previous_root,
                &new_root,
                &copied_root,
            );
            collect_root_locator(
                profile,
                "archiveRoot",
                &copied_root,
                &new_root,
                &mut root_candidates,
            );
        }
    }
    if profiles_changed {
        store.write_collection(PROFILES_COLLECTION, profiles)?;
    }

    let marker_roots = root_candidates
        .into_iter()
        .filter(|root| {
            copied_directory(&copied_root, root).is_some()
                && copied_file(&copied_root, &root.join(MARKER_FILE)).is_some()
        })
        .collect::<Vec<_>>();
    let mut marked_files = BTreeSet::<PathBuf>::new();
    for marker_root in &marker_roots {
        collect_marked_snapshot_files(marker_root, &mut marked_files)?;
    }

    let mut archive_collections = BTreeMap::<PathBuf, PathBuf>::new();
    for path in &marked_files {
        match path.file_name().and_then(OsStr::to_str) {
            Some(SNAPSHOT_JSON) => {
                let mut snapshot = read_json_file(path)?;
                let mut changed = false;
                for key in [
                    "semanticDocumentPath",
                    "semanticMarkdownPath",
                    "rawContentPath",
                ] {
                    changed |= relocate_path_field(
                        &mut snapshot,
                        key,
                        &previous_root,
                        &new_root,
                        &copied_root,
                    );
                }
                if changed {
                    atomic_write_json(path, &snapshot)?;
                }
            }
            Some(COLLECTION_JSON) => {
                let mut collection = read_json_file(path)?;
                let mut changed = relocate_path_field(
                    &mut collection,
                    "snapshotRoot",
                    &previous_root,
                    &new_root,
                    &copied_root,
                );
                if let Some(conversations) = collection
                    .get_mut("conversations")
                    .and_then(Value::as_array_mut)
                {
                    for record in conversations {
                        for key in [
                            "snapshotPath",
                            "semanticDocumentPath",
                            "semanticMarkdownPath",
                            "rawContentPath",
                        ] {
                            changed |= relocate_path_field(
                                record,
                                key,
                                &previous_root,
                                &new_root,
                                &copied_root,
                            );
                        }
                    }
                }
                if let Some(profile) = collection.get_mut("archiveProfile") {
                    changed |= relocate_path_field(
                        profile,
                        "archiveRoot",
                        &previous_root,
                        &new_root,
                        &copied_root,
                    );
                    changed |= relocate_path_field(
                        profile,
                        "baselineIndexPath",
                        &previous_root,
                        &new_root,
                        &copied_root,
                    );
                }
                if changed {
                    atomic_write_json(path, &collection)?;
                }
                if collection.get("archiveProfile").is_some() {
                    let marker_root = marker_root_for(path, &marker_roots)
                        .ok_or_else(|| anyhow!("snapshot collection is outside a marked root"))?;
                    archive_collections.insert(path.clone(), marker_root.to_path_buf());
                }
            }
            Some(CONVERSATION_INDEX_JSONL) => {
                let mut records = read_index_records(path)?;
                let mut changed = false;
                for record in &mut records {
                    for key in [
                        "snapshot_path",
                        "raw_content_path",
                        "semantic_document_path",
                        "semantic_markdown_path",
                    ] {
                        changed |= relocate_path_field(
                            record,
                            key,
                            &previous_root,
                            &new_root,
                            &copied_root,
                        );
                    }
                }
                if changed {
                    write_jsonl(path, &records)?;
                }
            }
            _ => {}
        }
    }

    for (collection_path, marker_root) in archive_collections {
        regenerate_archive_reports(&copied_root, &new_root, &marker_root, &collection_path)?;
    }
    Ok(())
}

fn regenerate_archive_reports(
    copied_root: &Path,
    new_root: &Path,
    marker_root: &Path,
    collection_path: &Path,
) -> Result<()> {
    let collection_dir = collection_path
        .parent()
        .ok_or_else(|| anyhow!("snapshot collection has no parent directory"))?;
    let mut collection = read_json_file(collection_path)?;
    let profile_value = collection
        .get("archiveProfile")
        .cloned()
        .ok_or_else(|| anyhow!("archive collection is missing archiveProfile"))?;
    let profile = parse_archive_profile(&profile_value)?;
    let index_path = collection_dir.join(CONVERSATION_INDEX_JSONL);
    let index_records = if copied_file(copied_root, &index_path).is_some() {
        read_index_records(&index_path)?
    } else {
        Vec::new()
    };

    let validation_path = collection_dir.join(VALIDATION_JSON);
    let prior_validation = if copied_file(copied_root, &validation_path).is_some() {
        Some(read_json_file(&validation_path)?)
    } else {
        None
    };
    let baseline_is_unavailable_in_copy = profile
        .baseline_index_path
        .as_ref()
        .is_some_and(|path| copied_logical_file(copied_root, new_root, path).is_none());
    let preserved_external_baseline = baseline_is_unavailable_in_copy.then(|| {
        prior_validation
            .as_ref()
            .and_then(|validation| validation.get("baseline"))
            .cloned()
            .or_else(|| {
                collection
                    .get("archiveHealth")
                    .and_then(|health| health.get("baseline"))
                    .cloned()
            })
    });
    let preserved_external_baseline = preserved_external_baseline.flatten();
    let validation = validate_archive_collection_for_relocation(
        collection_dir,
        &index_records,
        &profile,
        copied_root,
        new_root,
        preserved_external_baseline,
    )?;

    atomic_write_json(&validation_path, &validation)?;
    collection
        .as_object_mut()
        .ok_or_else(|| anyhow!("archive collection must be an object"))?
        .insert("archiveHealth".to_string(), validation.clone());
    atomic_write_json(collection_path, &collection)?;

    let sources_path = collection_dir.join(SOURCES_JSON);
    let sources = if copied_file(copied_root, &sources_path).is_some() {
        read_json_file(&sources_path)?
    } else {
        json!({})
    };
    let source_summaries = sources
        .get("sources")
        .and_then(Value::as_array)
        .cloned()
        .or_else(|| collection.get("sources").and_then(Value::as_array).cloned())
        .unwrap_or_default();
    let candidate_count = collection
        .pointer("/latestRefreshSummary/candidateCount")
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or_default();
    atomic_write_text(
        &collection_dir.join(CONVERSATION_INDEX_MD),
        &conversation_index_markdown(&profile, &index_records, &validation),
    )?;
    atomic_write_text(
        &collection_dir.join(SUMMARY_MD),
        &archive_summary_markdown(
            &profile,
            &new_root.join(marker_root.strip_prefix(copied_root)?),
            candidate_count,
            &source_summaries,
            &index_records,
            &validation,
        ),
    )?;
    Ok(())
}

fn collect_marked_snapshot_files(root: &Path, files: &mut BTreeSet<PathBuf>) -> Result<()> {
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                pending.push(entry.path());
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            if matches!(
                entry.file_name().to_str(),
                Some(SNAPSHOT_JSON | COLLECTION_JSON | CONVERSATION_INDEX_JSONL)
            ) {
                files.insert(entry.path());
            }
        }
    }
    Ok(())
}

fn marker_root_for<'a>(path: &Path, roots: &'a [PathBuf]) -> Option<&'a Path> {
    roots
        .iter()
        .filter(|root| path.starts_with(root))
        .max_by_key(|root| root.components().count())
        .map(PathBuf::as_path)
}

fn collect_root_locator(
    document: &Value,
    key: &str,
    copied_root: &Path,
    new_root: &Path,
    roots: &mut BTreeSet<PathBuf>,
) {
    let Some(raw) = document.get(key).and_then(Value::as_str) else {
        return;
    };
    let Some(path) = lexical_absolute(Path::new(raw)).and_then(|path| {
        path.strip_prefix(new_root)
            .ok()
            .map(|relative| copied_root.join(relative))
    }) else {
        return;
    };
    if path_is_within_root(&path, copied_root) && copied_directory(copied_root, &path).is_some() {
        if let Some(normalized) = lexical_absolute(&path) {
            roots.insert(normalized);
        }
    }
}

fn relocate_path_field(
    document: &mut Value,
    key: &str,
    previous_root: &Path,
    new_root: &Path,
    copied_root: &Path,
) -> bool {
    let Some(raw) = document.get(key).and_then(Value::as_str) else {
        return false;
    };
    let Some(path) = relocated_path(raw, previous_root, new_root, copied_root) else {
        return false;
    };
    let replacement = display_path(&path);
    if raw == replacement {
        return false;
    }
    if let Some(object) = document.as_object_mut() {
        object.insert(key.to_string(), Value::String(replacement));
        return true;
    }
    false
}

fn relocated_path(
    raw: &str,
    previous_root: &Path,
    new_root: &Path,
    copied_root: &Path,
) -> Option<PathBuf> {
    let old_path = lexical_absolute(Path::new(raw))?;
    let relative = old_path.strip_prefix(previous_root).ok()?;
    let replacement = lexical_absolute(&new_root.join(relative))?;
    if !replacement.starts_with(new_root)
        || copied_path_prefix(copied_root, &copied_root.join(relative)).is_none()
    {
        return None;
    }
    Some(replacement)
}

fn path_is_within_root(path: &Path, root: &Path) -> bool {
    lexical_absolute(path)
        .zip(lexical_absolute(root))
        .is_some_and(|(path, root)| path.starts_with(root))
}

pub(super) fn copied_file(root: &Path, path: &Path) -> Option<PathBuf> {
    let resolved = copied_path(root, path)?;
    fs::symlink_metadata(&resolved)
        .ok()
        .filter(|metadata| metadata.is_file())?;
    Some(resolved)
}

/// Resolve an owner-managed logical locator only within its verified physical copy.
pub(super) fn copied_logical_file(
    copied_root: &Path,
    logical_root: &Path,
    path: &Path,
) -> Option<PathBuf> {
    let logical = lexical_absolute(path)?;
    let relative = logical.strip_prefix(logical_root).ok()?;
    copied_file(copied_root, &copied_root.join(relative))
}

fn copied_directory(root: &Path, path: &Path) -> Option<PathBuf> {
    let resolved = copied_path(root, path)?;
    fs::symlink_metadata(&resolved)
        .ok()
        .filter(|metadata| metadata.is_dir())?;
    Some(resolved)
}

fn copied_path(root: &Path, path: &Path) -> Option<PathBuf> {
    let root = lexical_absolute(root)?;
    let path = lexical_absolute(path)?;
    let relative = path.strip_prefix(&root).ok()?;
    let mut current = root;
    let mut components = relative.components().peekable();
    while let Some(component) = components.next() {
        let std::path::Component::Normal(segment) = component else {
            return None;
        };
        current.push(segment);
        let metadata = fs::symlink_metadata(&current).ok()?;
        if metadata.file_type().is_symlink() || (components.peek().is_some() && !metadata.is_dir())
        {
            return None;
        }
    }
    if relative.as_os_str().is_empty() {
        let metadata = fs::symlink_metadata(&current).ok()?;
        if metadata.file_type().is_symlink() {
            return None;
        }
    }
    Some(current)
}

fn copied_path_prefix(root: &Path, path: &Path) -> Option<PathBuf> {
    let root = lexical_absolute(root)?;
    let path = lexical_absolute(path)?;
    let relative = path.strip_prefix(&root).ok()?;
    let root_metadata = fs::symlink_metadata(&root).ok()?;
    if !root_metadata.is_dir() || root_metadata.file_type().is_symlink() {
        return None;
    }
    let mut current = root;
    let mut components = relative.components().peekable();
    while let Some(component) = components.next() {
        let std::path::Component::Normal(segment) = component else {
            return None;
        };
        current.push(segment);
        match fs::symlink_metadata(&current) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink()
                    || (components.peek().is_some() && !metadata.is_dir())
                {
                    return None;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Some(path),
            Err(_) => return None,
        }
    }
    Some(path)
}

fn read_json_file(path: &Path) -> Result<Value> {
    Ok(serde_json::from_str(&fs::read_to_string(path)?)?)
}

fn lexical_absolute(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            std::path::Component::RootDir => normalized.push(component.as_os_str()),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if normalized.file_name().is_some() {
                    normalized.pop();
                }
            }
            std::path::Component::Normal(segment) => normalized.push(segment),
        }
    }
    Some(normalized)
}
