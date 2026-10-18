use super::*;
use licoup_foundation::platform::process_supervisor::run_bounded_untrusted_agent_output;
use std::time::Duration;

const MAX_OUTPUT_BYTES: usize = 512 * 1024;
// Product policy: every model-catalog scan waits up to one minute.
const PI_CLI_MODEL_LOOKUP_TIMEOUT: Duration = Duration::from_secs(60);

pub(super) fn collect_pi_cli_model_catalog(
    params: &Value,
    entries: &mut BTreeMap<String, ModelCatalogEntry>,
    diagnostics: &mut Vec<Value>,
) -> bool {
    const SOURCE: &str = "pi-cli:list-models";
    if !agent_cli_model_lookup_enabled(params) {
        diagnostics.push(json!({"source": SOURCE, "status": "disabled"}));
        return false;
    }
    let program = param_string(params, "piCliPath")
        .map(PathBuf::from)
        .or_else(|| find_binary(&["pi"]));
    let Some(program) = program else { return false };
    if !crate::domain::targets::scan_paths::discovered_agent_may_execute(&program, true) {
        diagnostics.push(json!({"source": SOURCE, "status": "execution-denied"}));
        return false;
    }
    let mut command = Command::new(program);
    command.arg("--list-models");
    // Applied for uniformity with the other catalog lookups; this lookup runs
    // through the deliberately scrubbed untrusted-agent runner, whose
    // env_clear + whitelist is applied afterwards and still wins.
    crate::platform::user_shell_environment::apply_to_command(&mut command);
    let Ok(output) = run_bounded_untrusted_agent_output(
        &mut command,
        PI_CLI_MODEL_LOOKUP_TIMEOUT,
        MAX_OUTPUT_BYTES,
    ) else {
        diagnostics.push(json!({"source": SOURCE, "status": "command-failed"}));
        return false;
    };
    if output.timed_out || output.truncated || !output.status.is_some_and(|status| status.success())
    {
        diagnostics.push(json!({"source": SOURCE, "status": "unavailable"}));
        return false;
    }
    let raw = String::from_utf8_lossy(&output.stdout);
    if collect_pi_models_from_output(&raw, entries) == 0 {
        diagnostics.push(json!({"source": SOURCE, "status": "empty"}));
        return false;
    }
    true
}

fn collect_pi_models_from_output(
    raw: &str,
    entries: &mut BTreeMap<String, ModelCatalogEntry>,
) -> usize {
    let before = entries.len();
    for line in raw.lines().skip(1) {
        let columns = line.split_whitespace().collect::<Vec<_>>();
        if columns.len() < 2 {
            continue;
        }
        let efforts = (columns.get(4).copied() == Some("yes"))
            .then(|| ["off", "minimal", "low", "medium", "high", "xhigh", "max"])
            .into_iter()
            .flatten()
            .map(str::to_owned)
            .collect();
        let selector = format!("{}/{}", columns[0], columns[1]);
        add_model_catalog_entry_with_provider(
            entries,
            &selector,
            Some(columns[1]),
            Some(columns[0]),
            None,
            "pi-cli:list-models",
            efforts,
        );
    }
    entries.len().saturating_sub(before)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::targets::support::display_path;
    #[cfg(unix)]
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn parses_pi_native_model_table() {
        let mut entries = BTreeMap::new();
        let count = collect_pi_models_from_output(
            "provider model context max-out thinking images\nacme alpha 1M 8K yes no\n",
            &mut entries,
        );
        assert_eq!(count, 1);
        let model = entries.values().next().unwrap();
        assert_eq!(model.name, "acme/alpha");
        assert_eq!(model.display_name, "Alpha");
        assert_eq!(model.provider_id.as_deref(), Some("acme"));
        assert!(model.reasoning_efforts.contains(&"max".to_string()));
    }

    #[test]
    fn disabled_lookup_does_not_claim_a_native_refresh() {
        let catalog = model_catalog_for_target(
            &crate::port::fixtures::packaged(),
            "pi",
            None,
            &json!({
                "enableAgentCliModelLookup": false,
                "includeHistoryModelCatalog": false,
            }),
        );
        assert!(
            !catalog["sources"]
                .as_array()
                .is_some_and(|sources| sources.contains(&json!("pi-cli:list-models")))
        );
    }

    #[cfg(unix)]
    #[test]
    fn successful_lookup_claims_a_native_refresh() {
        let dir = std::env::temp_dir().join(format!("licoup-pi-models-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let executable = dir.join("pi");
        fs::write(
            &executable,
            "#!/bin/sh\nprintf 'provider model context max-out thinking images\\nacme alpha 1M 8K yes no\\n'\n",
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let catalog = model_catalog_for_target(
            &crate::port::fixtures::packaged(),
            "pi",
            None,
            &json!({
                "enableAgentCliModelLookup": true,
                "includeHistoryModelCatalog": false,
                "piCliPath": display_path(executable),
            }),
        );
        assert!(
            catalog["sources"]
                .as_array()
                .is_some_and(|sources| sources.contains(&json!("pi-cli:list-models")))
        );
        fs::remove_dir_all(dir).unwrap();
    }
}
