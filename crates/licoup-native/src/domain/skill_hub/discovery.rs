//! Read-only discovery of skills installed in a paired agent's local skill root.

use super::{
    PathBuf, Result, Value, absolute_lexical_path, directory_exists_no_follow, fs,
    inspect_skill_dir, json, resolve_skill_root, validate_no_symlink_ancestors,
};

pub(super) fn discover(agent_id: &str, params: &Value) -> Result<Vec<Value>> {
    let root = absolute_lexical_path(&resolve_skill_root(agent_id, params)?)?;
    validate_no_symlink_ancestors(&root)?;
    if !directory_exists_no_follow(&root)? {
        return Ok(Vec::new());
    }

    let mut directories = fs::read_dir(&root)?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .collect::<Vec<PathBuf>>();
    directories.sort();
    let mut skills = Vec::new();
    for directory in directories {
        let metadata = match fs::symlink_metadata(&directory) {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            continue;
        }
        let preview = match inspect_skill_dir(&directory) {
            Ok(preview) => preview,
            Err(_) => continue,
        };
        skills.push(json!({
            "kind": "skill",
            "skillId": preview.skill_id,
            "agentId": agent_id,
            "target": agent_id,
            "title": preview.title,
            "description": preview.description,
            "version": preview.version,
            "path": directory.to_string_lossy(),
            "skillRoot": root.to_string_lossy(),
            "source": {"kind": "local-agent-skill-root"},
            "protocolStatus": "local",
            "installer": "agent-local",
            "packageDigestSha256": preview.digest_sha256,
            "fileCount": preview.file_count
        }));
    }
    Ok(skills)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use uuid::Uuid;

    #[test]
    fn discovers_only_valid_direct_child_skill_packages_in_stable_order() {
        let root = env::temp_dir().join(format!("lico-skill-discovery-{}", Uuid::new_v4()));
        fs::create_dir_all(root.join("zeta")).unwrap();
        fs::create_dir_all(root.join("alpha")).unwrap();
        fs::create_dir_all(root.join("not-a-skill")).unwrap();
        fs::write(
            root.join("zeta/SKILL.md"),
            "---\nname: zeta\ntitle: Zeta\nversion: 1\n---\n",
        )
        .unwrap();
        fs::write(
            root.join("alpha/SKILL.md"),
            "---\nname: alpha\ntitle: Alpha\nversion: 2\n---\n",
        )
        .unwrap();
        let root = root.canonicalize().unwrap();

        let skills = discover(
            "custom-agent",
            &json!({"skillRoot": root.to_string_lossy()}),
        )
        .unwrap();
        assert_eq!(skills.len(), 2);
        assert_eq!(skills[0]["skillId"], "alpha");
        assert_eq!(skills[1]["skillId"], "zeta");
        assert_eq!(skills[0]["protocolStatus"], "local");
    }
}
