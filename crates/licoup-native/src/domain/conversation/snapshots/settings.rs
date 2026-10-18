//! Snapshot roots, collection settings, archive destinations, and profile configuration.

use super::*;

pub(crate) fn root_get(params: &Value) -> Result<Value> {
    let store = client_state_store(params)?;
    let root = snapshot_root(&store, params)?;
    Ok(json!({
        "ok": true,
        "schemaVersion": COLLECTION_SCHEMA_VERSION,
        "snapshotRoot": display_path(&root.path),
        "mode": root.mode,
        "configured": root.mode == "user-controlled",
        "markerPath": display_path(&root.path.join(MARKER_FILE))
    }))
}

pub(crate) fn root_set(params: &Value) -> Result<Value> {
    let store = client_state_store(params)?;
    let Some(new_root_raw) = text_param(params, &["path", "root", "snapshotRoot"]) else {
        return Err(anyhow!("snapshots root set requires --path"));
    };
    let new_root = expand_home(&new_root_raw);
    let old_root = snapshot_root(&store, &json!({}))?;
    if equivalent_paths(&old_root.path, &new_root) {
        write_snapshot_root_setting(&store, &new_root)?;
        ensure_snapshot_root(&new_root)?;
        return Ok(json!({
            "ok": true,
            "status": "unchanged",
            "snapshotRoot": display_path(&new_root),
            "previousSnapshotRoot": display_path(&old_root.path),
            "migration": {"status": "not_needed"}
        }));
    }

    if new_root.exists() && !directory_is_empty(&new_root)? {
        return Ok(json!({
            "ok": false,
            "status": "directory_conflict",
            "snapshotRoot": display_path(&new_root),
            "previousSnapshotRoot": display_path(&old_root.path),
            "message": "Snapshot root already contains files. Choose an empty folder or keep the current root."
        }));
    }

    let migration = if old_root.path.exists() && old_root.path.join(MARKER_FILE).exists() {
        migrate_snapshot_root(&old_root.path, &new_root)?
    } else {
        ensure_snapshot_root(&new_root)?;
        json!({
            "status": "initialized",
            "from": display_path(&old_root.path),
            "to": display_path(&new_root)
        })
    };

    write_snapshot_root_setting(&store, &new_root)?;
    let activity = store.activity_log().append(
        "conversation_snapshots.root_set",
        json!({
            "target": "conversation-snapshots",
            "snapshotRoot": display_path(&new_root),
            "previousSnapshotRoot": display_path(&old_root.path),
            "migration": migration.clone()
        }),
    )?;
    Ok(json!({
        "ok": true,
        "status": "set",
        "snapshotRoot": display_path(&new_root),
        "previousSnapshotRoot": display_path(&old_root.path),
        "migration": migration,
        "activity": activity
    }))
}

pub(crate) fn collections_list(params: &Value) -> Result<Value> {
    let store = client_state_store(params)?;
    let root = snapshot_root(&store, params)?;
    let collections_dir = root.path.join("collections");
    let mut collections = Vec::<Value>::new();
    if collections_dir.exists() {
        collect_collection_summaries(&collections_dir, &mut collections)?;
    }
    collections.sort_by(|left, right| {
        right
            .get("refreshedAt")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .cmp(
                left.get("refreshedAt")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            )
    });
    Ok(json!({
        "ok": true,
        "schemaVersion": COLLECTION_SCHEMA_VERSION,
        "snapshotRoot": display_path(&root.path),
        "collections": collections
    }))
}

pub(super) fn collect_collection_summaries(dir: &Path, collections: &mut Vec<Value>) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let path = entry.path();
        let collection_path = path.join(COLLECTION_JSON);
        if collection_path.exists() {
            let collection = read_json_or_default(&collection_path, || json!({}))?;
            collections.push(collection_summary(&collection, &collection_path));
            continue;
        }
        collect_collection_summaries(&path, collections)?;
    }
    Ok(())
}

pub(crate) fn profiles_list(params: &Value) -> Result<Value> {
    let store = client_state_store(params)?;
    let document = store.read_collection(PROFILES_COLLECTION)?;
    let mut profiles = document
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    profiles.sort_by(|left, right| {
        text_value(left, "profileId")
            .unwrap_or_default()
            .cmp(&text_value(right, "profileId").unwrap_or_default())
    });
    Ok(json!({
        "ok": true,
        "schemaVersion": COLLECTION_SCHEMA_VERSION,
        "profiles": profiles
    }))
}

pub(crate) fn profile_get(params: &Value) -> Result<Value> {
    let store = client_state_store(params)?;
    let profile = load_archive_profile(&store, params)?;
    Ok(json!({
        "ok": true,
        "schemaVersion": COLLECTION_SCHEMA_VERSION,
        "profile": archive_profile_value(&profile)
    }))
}

pub(crate) fn profile_import(params: &Value) -> Result<Value> {
    let store = client_state_store(params)?;
    let profile_value = archive_profile_input(params)?;
    let profile = parse_archive_profile(&profile_value)?;
    let normalized = archive_profile_value(&profile);
    let mut document = store
        .read_collection(PROFILES_COLLECTION)?
        .as_object()
        .cloned()
        .unwrap_or_default();
    let mut items = document
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if let Some(existing) = items.iter_mut().find(|item| {
        item.get("profileId").and_then(Value::as_str) == Some(profile.profile_id.as_str())
    }) {
        *existing = normalized.clone();
    } else {
        items.push(normalized.clone());
    }
    document.insert("items".to_string(), Value::Array(items));
    store.write_collection(PROFILES_COLLECTION, Value::Object(document))?;
    Ok(json!({
        "ok": true,
        "status": "imported",
        "schemaVersion": COLLECTION_SCHEMA_VERSION,
        "profile": normalized
    }))
}

pub(super) fn snapshot_root(store: &ClientStateStore, params: &Value) -> Result<SnapshotRoot> {
    if let Some(root) = text_param(params, &["snapshotRoot"]) {
        if !root.trim().is_empty() {
            return Ok(SnapshotRoot {
                path: expand_home(&root),
                mode: "override",
            });
        }
    }
    let settings = store.read_collection(SETTINGS_COLLECTION)?;
    for key in ["conversationSnapshotRoot", "snapshotRoot"] {
        if let Some(root) = settings.get(key).and_then(Value::as_str) {
            if !root.trim().is_empty() {
                return Ok(SnapshotRoot {
                    path: expand_home(root),
                    mode: "user-controlled",
                });
            }
        }
    }
    Ok(SnapshotRoot {
        path: store.root().join(DEFAULT_SNAPSHOT_ROOT_DIR),
        mode: "default",
    })
}

pub(super) fn write_snapshot_root_setting(store: &ClientStateStore, root: &Path) -> Result<()> {
    let mut settings = store
        .read_collection(SETTINGS_COLLECTION)?
        .as_object()
        .cloned()
        .unwrap_or_default();
    settings.insert(
        "conversationSnapshotRoot".to_string(),
        json!(display_path(root)),
    );
    store.write_collection(SETTINGS_COLLECTION, Value::Object(settings))?;
    Ok(())
}

pub(super) fn ensure_snapshot_root(root: &Path) -> Result<()> {
    fs::create_dir_all(root.join("collections"))?;
    let marker_path = root.join(MARKER_FILE);
    if !marker_path.exists() {
        atomic_write_json(
            &marker_path,
            &json!({
                "schemaVersion": COLLECTION_SCHEMA_VERSION,
                "kind": "lico-native-conversation-snapshot-root",
                "createdAt": timestamp_rfc3339()
            }),
        )?;
    }
    Ok(())
}

pub(super) fn migrate_snapshot_root(old_root: &Path, new_root: &Path) -> Result<Value> {
    if let Some(parent) = new_root.parent() {
        fs::create_dir_all(parent)?;
    }
    match fs::rename(old_root, new_root) {
        Ok(()) => Ok(json!({
            "status": "migrated",
            "method": "rename",
            "from": display_path(old_root),
            "to": display_path(new_root)
        })),
        Err(_) => {
            copy_dir_all(old_root, new_root)?;
            fs::remove_dir_all(old_root)?;
            Ok(json!({
                "status": "migrated",
                "method": "copy",
                "from": display_path(old_root),
                "to": display_path(new_root)
            }))
        }
    }
}

pub(super) fn archive_destination(params: &Value) -> Result<PathBuf> {
    let raw = text_param(
        params,
        &[
            "path",
            "archiveRoot",
            "destination",
            "destinationPath",
            "outputDir",
            "snapshotRoot",
        ],
    )
    .filter(|value| !value.trim().is_empty())
    .ok_or_else(|| anyhow!("archive collect requires --path"))?;
    let expanded = expand_home(&raw);
    // Reject path traversal components.
    ensure!(
        !expanded.to_string_lossy().contains(".."),
        "archive destination path must not contain traversal components"
    );
    // Canonicalize if the path exists, otherwise canonicalize the parent.
    let canonical = if expanded.exists() {
        expanded.canonicalize()?
    } else if let Some(parent) = expanded.parent() {
        if !parent.exists() {
            return Err(anyhow!(
                "archive destination parent does not exist: {}",
                parent.display()
            ));
        }
        let canonical_parent = parent.canonicalize()?;
        canonical_parent.join(
            expanded
                .file_name()
                .ok_or_else(|| anyhow!("archive destination has no file name"))?,
        )
    } else {
        return Err(anyhow!(
            "archive destination cannot be canonicalized: {}",
            expanded.display()
        ));
    };
    // Reject destinations not owned by the current user.
    licoup_foundation::platform::file_security::validate_export_destination(&canonical)?;
    Ok(canonical)
}

pub(super) fn archive_profile_input(params: &Value) -> Result<Value> {
    if let Some(raw) = text_param(params, &["profileJson"]) {
        if !raw.trim().is_empty() {
            return Ok(serde_json::from_str(&raw)?);
        }
    }
    if let Some(path) = text_param(params, &["profileFile"]) {
        if !path.trim().is_empty() {
            return Ok(serde_json::from_str(&fs::read_to_string(expand_home(
                &path,
            ))?)?);
        }
    }
    let mut object = Map::<String, Value>::new();
    for key in [
        "profileId",
        "displayName",
        "archiveRoot",
        "baselineIndexPath",
    ] {
        if let Some(value) = text_param(params, &[key]) {
            if !value.trim().is_empty() {
                object.insert(key.to_string(), json!(value));
            }
        }
    }
    for key in [
        "canonicalNames",
        "aliasNames",
        "projectPaths",
        "expectedAgents",
        "expectedSources",
        "exclusionRules",
        "collectionPathSegments",
    ] {
        if let Some(value) = text_param(params, &[key]) {
            object.insert(key.to_string(), json!(split_path_list(&value)));
        }
    }
    Ok(Value::Object(object))
}

pub(super) fn parse_archive_profile(value: &Value) -> Result<ArchiveProfile> {
    let display_name = text_value(value, "displayName")
        .or_else(|| text_value(value, "name"))
        .or_else(|| text_value(value, "profileId"))
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow!("archive profile requires displayName or profileId"))?;
    let profile_id = text_value(value, "profileId")
        .or_else(|| topic_key(&display_name))
        .map(|value| sanitize_id(&value))
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow!("archive profile id is empty"))?;
    let collection_path_segments = collection_path_segments_value(value, &profile_id);
    let expected_agents = string_list_value(value, "expectedAgents")
        .into_iter()
        .map(|agent| normalize_agent_alias(&agent))
        .collect::<Vec<_>>();
    Ok(ArchiveProfile {
        profile_id,
        display_name,
        collection_path_segments,
        archive_root: text_value(value, "archiveRoot").map(|path| expand_home(&path)),
        canonical_names: string_list_value(value, "canonicalNames"),
        alias_names: string_list_value(value, "aliasNames"),
        project_paths: string_list_value(value, "projectPaths"),
        expected_agents,
        expected_sources: string_list_value(value, "expectedSources"),
        exclusion_rules: string_list_value(value, "exclusionRules"),
        baseline_index_path: text_value(value, "baselineIndexPath").map(|path| expand_home(&path)),
        raw: value.clone(),
    })
}

pub(super) fn collection_path_segments_value(
    value: &Value,
    fallback_profile_id: &str,
) -> Vec<String> {
    let mut segments = Vec::<String>::new();
    let mut seen = BTreeSet::<String>::new();
    for raw in string_list_value(value, "collectionPathSegments") {
        let segment = topic_key(&raw).unwrap_or_else(|| sanitize_id(&raw));
        if !segment.is_empty() && seen.insert(segment.clone()) {
            segments.push(segment);
        }
    }
    if segments.is_empty() {
        segments.push(fallback_profile_id.to_string());
    }
    segments
}

pub(super) fn archive_profile_value(profile: &ArchiveProfile) -> Value {
    let mut object = profile.raw.as_object().cloned().unwrap_or_default();
    object.insert(
        "schemaVersion".to_string(),
        json!(COLLECTION_SCHEMA_VERSION),
    );
    object.insert("profileId".to_string(), json!(profile.profile_id));
    object.insert("displayName".to_string(), json!(profile.display_name));
    object.insert(
        "collectionPathSegments".to_string(),
        json!(profile.collection_path_segments),
    );
    object.insert(
        "archiveRoot".to_string(),
        profile
            .archive_root
            .as_ref()
            .map(|path| json!(display_path(path)))
            .unwrap_or_else(|| json!(null)),
    );
    object.insert("canonicalNames".to_string(), json!(profile.canonical_names));
    object.insert("aliasNames".to_string(), json!(profile.alias_names));
    object.insert("projectPaths".to_string(), json!(profile.project_paths));
    object.insert("expectedAgents".to_string(), json!(profile.expected_agents));
    object.insert(
        "expectedSources".to_string(),
        json!(profile.expected_sources),
    );
    object.insert("exclusionRules".to_string(), json!(profile.exclusion_rules));
    object.insert(
        "baselineIndexPath".to_string(),
        profile
            .baseline_index_path
            .as_ref()
            .map(|path| json!(display_path(path)))
            .unwrap_or_else(|| json!(null)),
    );
    Value::Object(object)
}

pub(super) fn load_archive_profile(
    store: &ClientStateStore,
    params: &Value,
) -> Result<ArchiveProfile> {
    let profile_id = text_param(params, &["profile", "profileId"])
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow!("archive profile command requires --profile"))?;
    let document = store.read_collection(PROFILES_COLLECTION)?;
    let items = document
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let wanted = sanitize_id(&profile_id);
    let value = items
        .iter()
        .find(|item| {
            item.get("profileId")
                .and_then(Value::as_str)
                .map(|id| id == wanted || id == profile_id)
                .unwrap_or(false)
        })
        .cloned()
        .ok_or_else(|| anyhow!("unknown conversation archive profile: {}", profile_id))?;
    parse_archive_profile(&value)
}

pub(super) fn archive_root_for_profile(
    store: &ClientStateStore,
    params: &Value,
    profile: &ArchiveProfile,
) -> Result<PathBuf> {
    if let Some(root) = text_param(params, &["archiveRoot", "snapshotRoot"]) {
        if !root.trim().is_empty() {
            return Ok(expand_home(&root));
        }
    }
    if let Some(root) = &profile.archive_root {
        return Ok(root.clone());
    }
    Ok(snapshot_root(store, params)?.path)
}

pub(super) fn collection_dir_for_profile(root: &Path, profile: &ArchiveProfile) -> PathBuf {
    collection_dir_for_profile_layout(root, profile, ArchiveCollectionLayout::CollectionsSubdir)
}

pub(super) fn collection_dir_for_profile_layout(
    root: &Path,
    profile: &ArchiveProfile,
    layout: ArchiveCollectionLayout,
) -> PathBuf {
    let mut dir = root.join("collections");
    if layout == ArchiveCollectionLayout::DirectKeywordFolders {
        dir = root.to_path_buf();
    }
    for segment in &profile.collection_path_segments {
        dir = dir.join(segment);
    }
    dir
}
