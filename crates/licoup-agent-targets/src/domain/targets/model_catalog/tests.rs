use super::*;

/// The packaged driver declarations these catalog fixtures project.
fn fixture() -> crate::port::AgentTargetPort {
    crate::port::fixtures::packaged()
}

/// The canonical-registry facts arrive through the port, so this fixture
/// answers them the way `licoup-model-facts` answers them: one admitted
/// selector resolves to one canonical identity, and the snapshot carries a
/// revision. The projection claim is the same one this test always made — the
/// catalog attaches the canonical identity without rewriting the admitted
/// selector — and it is now stated at the boundary the identity crosses.
fn registry_fixture() -> crate::port::AgentTargetPort {
    fn resolve(
        raw: &str,
        provider_id: Option<&str>,
        _source_agent_id: Option<&str>,
    ) -> Option<crate::port::CanonicalModelFacts> {
        if raw == "native-selector" && provider_id == Some("relay") {
            return Some(crate::port::CanonicalModelFacts {
                id: "example/model-3".to_string(),
                lab_id: "example".to_string(),
            });
        }
        None
    }
    fn revision() -> String {
        "example-registry-revision".to_string()
    }
    crate::port::AgentTargetPort {
        resolve_canonical_model: resolve,
        model_registry_revision: revision,
        ..crate::port::fixtures::packaged()
    }
}

#[test]
fn native_model_catalog_attaches_registry_identity_without_rewriting_admitted_selector() {
    let mut entries = BTreeMap::new();
    add_model_catalog_entry_with_provider(
        &mut entries,
        "native-selector",
        Some("Native Model"),
        Some("relay"),
        Some("Relay"),
        "native-cli",
        BTreeSet::from(["high".to_owned()]),
    );
    let value = build_model_catalog(
        &registry_fixture(),
        "codex",
        entries,
        BTreeSet::from(["native-cli".to_owned()]),
        Vec::new(),
        Some("native-selector".to_owned()),
    );
    assert_eq!(value["defaultModel"], "native-selector");
    assert_eq!(value["models"][0]["name"], "native-selector");
    assert_eq!(value["models"][0]["canonicalModelId"], "example/model-3");
    assert_eq!(value["models"][0]["modelLabId"], "example");
    assert_eq!(value["models"][0]["reasoningEfforts"], json!(["high"]));
    assert_eq!(value["modelRegistryRevision"], "example-registry-revision");
}

/// The registry resolves both native speed selectors of one model to a single
/// canonical identity; that resolution rule is model facts' claim and is
/// asserted by its own tests. This fixture answers it the way the registry
/// answers it, so the claim here is the catalog's: every row carries the
/// identity the owner resolved and the admitted selector is never rewritten.
fn composer_registry_fixture() -> crate::port::AgentTargetPort {
    fn resolve(
        raw: &str,
        provider_id: Option<&str>,
        _source_agent_id: Option<&str>,
    ) -> Option<crate::port::CanonicalModelFacts> {
        if provider_id.is_some_and(|provider| provider != "cursor") {
            return None;
        }
        raw.strip_prefix("composer-")
            .filter(|suffix| *suffix == "2.5" || *suffix == "2.5-fast")
            .map(|_| crate::port::CanonicalModelFacts {
                id: "cursor/composer-2.5".to_string(),
                lab_id: "cursor".to_string(),
            })
    }
    crate::port::AgentTargetPort {
        resolve_canonical_model: resolve,
        ..crate::port::fixtures::packaged()
    }
}

#[test]
fn composer_catalog_retains_native_speed_selectors_with_one_global_identity() {
    for target in ["cursor", "opencode"] {
        let mut entries = BTreeMap::new();
        for selector in ["composer-2.5", "composer-2.5-fast"] {
            add_model_catalog_entry(&mut entries, selector, "native-cli", BTreeSet::new());
        }
        let value = build_model_catalog(
            &composer_registry_fixture(),
            target,
            entries,
            BTreeSet::from(["native-cli".to_owned()]),
            Vec::new(),
            Some("composer-2.5-fast".to_owned()),
        );
        assert_eq!(value["defaultModel"], "composer-2.5-fast");
        let models = value["models"].as_array().unwrap();
        assert_eq!(models.len(), 2);
        for model in models {
            assert_eq!(model["canonicalModelId"], "cursor/composer-2.5");
            assert_eq!(model["modelLabId"], "cursor");
        }
        assert_eq!(
            models
                .iter()
                .map(|model| model["name"].as_str().unwrap())
                .collect::<BTreeSet<_>>(),
            BTreeSet::from(["composer-2.5", "composer-2.5-fast"])
        );
    }
}

#[test]
fn native_catalog_uses_shared_display_typography_for_unresolved_models() {
    for (name, expected) in [
        ("custom/deepseek-v4-flash", "DeepSeek V4 Flash"),
        ("custom/gpt-reserve", "GPT Reserve"),
        ("custom/grok-bot-default", "Grok Bot"),
    ] {
        let mut entries = BTreeMap::new();
        add_model_catalog_entry(&mut entries, name, "native-cli", BTreeSet::new());
        let catalog = build_model_catalog(
            &fixture(),
            "codex",
            entries,
            BTreeSet::new(),
            Vec::new(),
            None,
        );
        assert_eq!(catalog["models"][0]["name"], name);
        assert_eq!(catalog["models"][0]["displayName"], expected);
        assert!(catalog["models"][0].get("canonicalModelId").is_none());
        let display_only = json!({"displayName": name});
        assert_eq!(model_name_from_value(&display_only), name);
        assert_eq!(
            model_display_name_from_value(&display_only, "").as_deref(),
            Some(expected)
        );
    }
}

#[test]
fn ephemeral_models_are_presented_before_the_regular_catalog() {
    let mut entries = BTreeMap::new();
    add_model_catalog_entry(&mut entries, "gpt-6-luna", "native-cli", BTreeSet::new());
    add_model_catalog_entry(
        &mut entries,
        "gpt-reserve",
        "codex-app-server",
        BTreeSet::new(),
    );
    entries
        .get_mut("gpt-reserve")
        .expect("reserve entry")
        .ephemeral = true;

    let catalog = build_model_catalog(
        &fixture(),
        "test-target",
        entries,
        BTreeSet::from(["codex-app-server".to_owned()]),
        Vec::new(),
        None,
    );
    assert_eq!(catalog["models"][0]["name"], "gpt-reserve");
    assert_eq!(catalog["models"][0]["ephemeral"], true);
}

#[test]
fn codex_reserve_rows_from_non_authoritative_sources_are_hidden() {
    let catalog = model_catalog_for_target(
        &fixture(),
        "codex",
        None,
        &json!({
            "includeHistoryModelCatalog": false,
            "modelCatalogFixture": {
                "codex": {
                    "defaultModel": "gpt-reserve",
                    "models": [
                        {"name": "gpt-6-luna"},
                        {"name": "gpt-reserve"}
                    ]
                }
            }
        }),
    );
    assert_eq!(catalog["defaultModel"], "");
    assert!(
        catalog["models"]
            .as_array()
            .unwrap()
            .iter()
            .all(|model| model["name"] != "gpt-reserve")
    );
}

#[cfg(unix)]
#[test]
fn codex_catalog_reuses_the_selected_binary_for_live_rate_limits() {
    use std::sync::OnceLock;

    // The App Server reader is the engines' owner, and its own tests assert the
    // rate-limit projection that produces the reserve row. What this asserts is
    // the inventory's half: the selected executable is the one the reader is
    // asked to read, because a second PATH lookup here would make the live
    // account response disappear.
    static OBSERVED_BINARY: OnceLock<String> = OnceLock::new();
    fn app_server_catalog(binary: &Path) -> anyhow::Result<Value> {
        OBSERVED_BINARY
            .set(binary.to_string_lossy().into_owned())
            .ok();
        Ok(json!({
            "defaultModel": "gpt-6-luna",
            "models": [
                {"name": "gpt-6-luna", "displayName": "GPT-6 Luna"},
                {"name": "gpt-reserve", "displayName": "GPT Reserve", "ephemeral": true},
            ],
        }))
    }
    let port = crate::port::AgentTargetPort {
        codex_app_server_model_catalog: app_server_catalog,
        ..fixture()
    };
    let selected = temp_test_dir("codex-selected-catalog").join("codex");
    fs::write(&selected, "synthetic selected Codex executable").unwrap();

    let catalog = model_catalog_for_target(
        &port,
        "codex",
        None,
        &json!({
            "includeHistoryModelCatalog": false,
            "enableAgentCliModelLookup": true,
            "codexCliPath": display_path(selected.clone()),
        }),
    );

    assert_eq!(
        OBSERVED_BINARY.get().map(String::as_str),
        Some(selected.to_string_lossy().as_ref())
    );
    assert!(
        catalog["sources"]
            .as_array()
            .unwrap()
            .contains(&json!("codex-app-server"))
    );
    let reserve = catalog["models"]
        .as_array()
        .unwrap()
        .iter()
        .find(|model| model["name"] == "gpt-reserve")
        .unwrap();
    assert_eq!(reserve["ephemeral"], true);
}

// Catalog tests must not discover the developer's real Agent configuration or
// launch a login shell. Explicit fixture homes remain owned by each case.
fn model_catalog_for_target(
    port: &crate::port::AgentTargetPort,
    target: &str,
    config_path: Option<&Path>,
    params: &Value,
) -> Value {
    let _snapshot =
        crate::platform::user_shell_environment::pin_process_env_snapshot_for_testing(&[]);
    let mut params = params.clone();
    params
        .as_object_mut()
        .unwrap()
        .entry("homeDir")
        .or_insert_with(|| json!(display_path(temp_test_dir("isolated-home"))));
    super::model_catalog_for_target(port, target, config_path, &params)
}

mod config_documents {
    use super::*;

    #[test]
    fn model_catalog_reads_models_from_client_config() {
        let dir = temp_test_dir("model-catalog-config");
        let config_path = dir.join("config.toml");
        fs::write(
            &config_path,
            r#"
model = "gpt-5.5"
model_reasoning_effort = "high"

[profiles.review]
model = "gpt-5.4-mini"
"#,
        )
        .unwrap();

        let catalog = model_catalog_for_target(
            &fixture(),
            "codex",
            Some(&config_path),
            &json!({"includeHistoryModelCatalog": false}),
        );
        let models = catalog["models"].as_array().unwrap();
        assert!(models.iter().any(|model| {
            model["name"] == "gpt-5.5"
                && model["displayName"] == "GPT-5.5"
                && model["reasoningEfforts"]
                    .as_array()
                    .unwrap()
                    .contains(&json!("high"))
        }));
        assert!(models.iter().any(|model| {
            model["name"] == "gpt-5.4-mini" && model["displayName"] == "GPT-5.4 Mini"
        }));
        let rendered = serde_json::to_string(&catalog).unwrap();
        assert!(!rendered.contains("api_key"));
    }

    #[test]
    fn model_catalog_reads_default_model_from_top_level_config() {
        let dir = temp_test_dir("model-catalog-default-model");
        let config_path = dir.join("config.toml");
        fs::write(
            &config_path,
            r#"
model = "gpt-5.5"

[profiles.review]
model = "gpt-5.4-mini"
"#,
        )
        .unwrap();

        let catalog = model_catalog_for_target(
            &fixture(),
            "codex",
            Some(&config_path),
            &json!({"includeHistoryModelCatalog": false}),
        );
        // The top-level `model` key is the configured default; nested profile
        // models stay picker entries and never become the default.
        assert_eq!(catalog["defaultModel"], json!("gpt-5.5"));
    }

    #[test]
    fn model_catalog_default_model_prefers_fixture_over_config() {
        let dir = temp_test_dir("model-catalog-default-fixture");
        let config_path = dir.join("config.toml");
        fs::write(&config_path, "model = \"gpt-5.5\"\n").unwrap();

        let catalog = model_catalog_for_target(
            &fixture(),
            "codex",
            Some(&config_path),
            &json!({
                "includeHistoryModelCatalog": false,
                "modelCatalogFixture": {
                    "codex": {
                        "defaultModel": "fixture-model",
                        "models": ["fixture-model"],
                    },
                },
            }),
        );
        assert_eq!(catalog["defaultModel"], json!("fixture-model"));
    }

    #[test]
    fn model_catalog_without_default_model_emits_empty_string() {
        let dir = temp_test_dir("model-catalog-no-default-model");
        let config_path = dir.join("config.toml");
        fs::write(&config_path, "model_reasoning_effort = \"high\"\n").unwrap();

        let catalog = model_catalog_for_target(
            &fixture(),
            "codex",
            Some(&config_path),
            &json!({"includeHistoryModelCatalog": false}),
        );
        assert_eq!(catalog["defaultModel"], json!(""));
    }

    #[test]
    fn model_catalog_reads_codex_structured_model_catalog() {
        let home = temp_test_dir("codex-model-catalog");
        let catalog_path = home
            .join(".codex")
            .join("model-catalogs")
            .join("available.json");
        fs::create_dir_all(catalog_path.parent().unwrap()).unwrap();
        fs::write(
            &catalog_path,
            json!({
                "models": [
                    {
                        "slug": "gpt-5.4",
                        "display_name": "gpt-5.4",
                        "supported_reasoning_levels": [
                            {"effort": "medium"}
                        ]
                    },
                    {
                        "slug": "gpt-5.4-mini",
                        "display_name": "GPT-5.4 Mini",
                        "supported_reasoning_levels": [
                            {"effort": "low"},
                            {"effort": "medium"},
                            {"effort": "high"},
                            {"effort": "xhigh"}
                        ]
                    },
                    {
                        "slug": "deepseek-v4-pro",
                        "display_name": "DeepSeek V4 Pro",
                        "supported_reasoning_levels": [
                            {"effort": "high"}
                        ]
                    },
                    {
                        "slug": "codex-auto-review",
                        "display_name": "Codex Auto Review",
                        "visibility": "hide",
                        "supported_reasoning_levels": [
                            {"effort": "high"}
                        ]
                    }
                ]
            })
            .to_string(),
        )
        .unwrap();

        let catalog = model_catalog_for_target(
            &fixture(),
            "codex",
            None,
            &json!({
                "homeDir": display_path(home),
                "includeHistoryModelCatalog": false,
            }),
        );
        let models = catalog["models"].as_array().unwrap();
        assert!(models.iter().any(|model| {
            model["name"] == "gpt-5.4"
                && model["displayName"] == "GPT-5.4"
                && model["reasoningEfforts"]
                    .as_array()
                    .unwrap()
                    .contains(&json!("medium"))
        }));
        assert!(models.iter().any(|model| {
            model["name"] == "gpt-5.4-mini"
                && model["displayName"] == "GPT-5.4 Mini"
                && model["reasoningEfforts"]
                    .as_array()
                    .unwrap()
                    .contains(&json!("xhigh"))
        }));
        assert!(models.iter().any(|model| {
            model["name"] == "deepseek-v4-pro"
                && model["displayName"] == "DeepSeek V4 Pro"
                && model["reasoningEfforts"]
                    .as_array()
                    .unwrap()
                    .contains(&json!("high"))
        }));
        assert!(
            !models
                .iter()
                .any(|model| model["name"] == "codex-auto-review")
        );
    }

    #[test]
    fn model_catalog_reads_codex_models_cache_json() {
        let home = temp_test_dir("codex-models-cache");
        let cache_path = home.join(".codex").join("models_cache.json");
        fs::create_dir_all(cache_path.parent().unwrap()).unwrap();
        fs::write(
            &cache_path,
            json!({
                "fetched_at": "2026-08-05T13:00:00.334386Z",
                "etag": "W/\"bbfb24a13d309e497dccc03e80defa5f\"",
                "client_version": "0.146.0",
                "models": [
                    {
                        "slug": "gpt-5.6-sol",
                        "display_name": "GPT-5.6-Sol",
                        "visibility": "list",
                        "supported_reasoning_levels": [
                            {"effort": "low"},
                            {"effort": "max"}
                        ]
                    },
                    {
                        "slug": "gpt-5.6-terra",
                        "display_name": "GPT-5.6-Terra",
                        "visibility": "list",
                        "supported_reasoning_levels": [
                            {"effort": "medium"}
                        ]
                    },
                    {
                        "slug": "gpt-5.6-luna",
                        "display_name": "GPT-5.6-Luna",
                        "visibility": "list",
                        "supported_reasoning_levels": [
                            {"effort": "high"}
                        ]
                    },
                    {
                        "slug": "gpt-5.5",
                        "display_name": "GPT-5.5",
                        "visibility": "list",
                        "supported_reasoning_levels": [
                            {"effort": "medium"}
                        ]
                    },
                    {
                        "slug": "gpt-5.3-codex-spark",
                        "display_name": "GPT-5.3-Codex-Spark",
                        "visibility": "list",
                        "supported_reasoning_levels": [
                            {"effort": "low"}
                        ]
                    },
                    {
                        "slug": "gpt-5.6-sol-wm",
                        "display_name": "GPT-5.6-Sol-WM",
                        "visibility": "hide",
                        "supported_reasoning_levels": [
                            {"effort": "low"}
                        ]
                    }
                ]
            })
            .to_string(),
        )
        .unwrap();
        let custom_catalog = home
            .join(".codex")
            .join("model-catalogs")
            .join("deepseek.json");
        fs::create_dir_all(custom_catalog.parent().unwrap()).unwrap();
        fs::write(
            &custom_catalog,
            json!({
                "models": [
                    {
                        "slug": "deepseek-v4-flash",
                        "display_name": "DeepSeek V4 Flash",
                        "visibility": "list",
                        "supported_reasoning_levels": [{"effort": "max"}]
                    }
                ]
            })
            .to_string(),
        )
        .unwrap();

        let catalog = model_catalog_for_target(
            &fixture(),
            "codex",
            None,
            &json!({
                "homeDir": display_path(home),
                "includeHistoryModelCatalog": false,
            }),
        );
        let models = catalog["models"].as_array().unwrap();
        let names: Vec<&str> = models
            .iter()
            .filter_map(|model| model["name"].as_str())
            .collect();
        assert!(names.contains(&"gpt-5.6-sol"));
        assert!(names.contains(&"gpt-5.6-terra"));
        assert!(names.contains(&"gpt-5.6-luna"));
        assert!(names.contains(&"gpt-5.5"));
        assert!(names.contains(&"gpt-5.3-codex-spark"));
        assert!(names.contains(&"deepseek-v4-flash"));
        assert!(!names.contains(&"gpt-5.6-sol-wm"));
        assert!(
            !names
                .iter()
                .any(|name| name.contains('T') || name.starts_with("W/") || *name == "0.146.0"),
            "cache metadata leaked into model names: {names:?}"
        );
    }
}

mod config_copilot {
    use super::*;

    fn write_workspace_storage_models(home: &Path) {
        let models_path = home
            .join("Library")
            .join("Application Support")
            .join("Code")
            .join("User")
            .join("workspaceStorage")
            .join("workspace-hash")
            .join("GitHub.copilot-chat")
            .join("models.json");
        fs::create_dir_all(models_path.parent().unwrap()).unwrap();
        fs::write(
            &models_path,
            json!([
                {"model": "copilot-custom-flash", "name": "Copilot Custom Flash"},
                {"model": "copilot-custom-opus"}
            ])
            .to_string(),
        )
        .unwrap();
    }

    #[test]
    fn copilot_selected_catalog_reads_workspace_storage_models() {
        let home = temp_test_dir("copilot-selected-model-cache");
        write_workspace_storage_models(&home);
        let catalog = model_catalog_for_target(
            &fixture(),
            "copilot",
            None,
            &json!({
                "homeDir": display_path(home),
                "includeHistoryModelCatalog": false,
                "enableAgentCliModelLookup": true,
            }),
        );
        let names = catalog["models"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|model| model["name"].as_str())
            .collect::<Vec<_>>();
        assert!(names.contains(&"copilot-custom-flash"));
        assert!(names.contains(&"copilot-custom-opus"));
        assert!(
            catalog["sources"]
                .as_array()
                .unwrap()
                .contains(&json!("model-cache"))
        );
    }

    #[test]
    fn copilot_unused_catalog_skips_other_app_workspace_storage() {
        let home = temp_test_dir("copilot-unused-model-cache");
        write_workspace_storage_models(&home);
        let catalog = model_catalog_for_target(
            &fixture(),
            "copilot",
            None,
            &json!({
                "homeDir": display_path(home),
                "includeHistoryModelCatalog": false,
            }),
        );
        let names = catalog["models"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|model| model["name"].as_str())
            .collect::<Vec<_>>();
        assert!(
            !names.contains(&"copilot-custom-flash"),
            "unused Copilot catalog must not read VS Code workspaceStorage"
        );
        assert!(!names.contains(&"copilot-custom-opus"));
    }
}

mod antigravity {
    use super::super::antigravity::collect_model_catalog_from_cli_lines;
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    #[cfg(unix)]
    use std::time::{Duration, Instant};

    #[test]
    fn model_catalog_preserves_antigravity_available_model_names() {
        let catalog = model_catalog_for_target(
            &fixture(),
            "antigravity",
            None,
            &json!({
                "includeHistoryModelCatalog": false,
                "antigravityAvailableModelsJson": json!({
                    "models": {
                        "gemini-flash-medium": {
                            "displayName": "Gemini 3.5 Flash (Medium)",
                            "reasoningEfforts": ["low", "medium", "high"]
                        },
                        "claude-opus-thinking": {
                            "displayName": "Claude Opus 4.6 (Thinking)",
                            "reasoningEfforts": ["high"]
                        }
                    }
                }).to_string()
            }),
        );

        let names = catalog["models"]
            .as_array()
            .unwrap()
            .iter()
            .map(|model| model["name"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert!(names.contains(&"Gemini 3.5 Flash (Medium)"));
        assert!(names.contains(&"Claude Opus 4.6 (Thinking)"));
        assert!(
            catalog["models"]
                .as_array()
                .unwrap()
                .iter()
                .all(|model| model["reasoningEfforts"].as_array().unwrap().is_empty())
        );
    }

    #[test]
    fn model_catalog_reads_antigravity_cli_model_lines() {
        let mut entries = BTreeMap::<String, ModelCatalogEntry>::new();
        let added = collect_model_catalog_from_cli_lines(
            r#"
Model ID          Name
Gemini 3.5 Flash (Medium)
Gemini 3.5 Flash (High)
Claude Opus 4.6 (Thinking)
"#,
            "antigravity-cli:models",
            &mut entries,
        );

        assert_eq!(added, 3);
        assert!(entries.values().any(|entry| {
            entry.name == "Gemini 3.5 Flash (Medium)" && entry.provider.is_none()
        }));
        assert!(entries.values().all(|entry| !entry.provider_inferred));
    }

    #[test]
    fn antigravity_cli_model_lines_split_id_from_display_name() {
        let mut entries = BTreeMap::<String, ModelCatalogEntry>::new();
        let added = collect_model_catalog_from_cli_lines(
            "gemini-3.7-flash-high Gemini 3.7 Flash (High)\ngemini-3.7-flash-low  Gemini 3.7 Flash (Low)\n",
            "antigravity-cli:models",
            &mut entries,
        );

        assert_eq!(added, 2);
        let high = entries
            .values()
            .find(|entry| entry.name == "gemini-3.7-flash-high")
            .expect("id-only selector entry");
        assert_eq!(high.display_name, "Gemini 3.7 Flash (High)");
        assert!(
            entries
                .values()
                .any(|entry| entry.name == "gemini-3.7-flash-low"
                    && entry.display_name == "Gemini 3.7 Flash (Low)")
        );
        // A capitalized line stays a bare display name with no id column.
        let mut named = BTreeMap::<String, ModelCatalogEntry>::new();
        collect_model_catalog_from_cli_lines(
            "Gemini 3.5 Flash (Medium)\n",
            "antigravity-cli:models",
            &mut named,
        );
        assert!(
            named
                .values()
                .any(|entry| entry.name == "Gemini 3.5 Flash (Medium)")
        );
    }

    #[cfg(unix)]
    #[test]
    fn antigravity_cli_model_lookup_preserves_real_results() {
        let dir = temp_test_dir("antigravity-cli-models");
        let config_path = dir.join("settings.json");
        fs::write(&config_path, r#"{"model":"gemini-3.1-pro-preview"}"#).unwrap();
        let executable = dir.join("agent-models");
        fs::write(
            &executable,
            r#"#!/bin/sh
printf 'gemini-3.6-flash-medium\nclaude-opus-4-6-thinking\ngpt-oss-120b-medium\n'
"#,
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();

        let catalog = model_catalog_for_target(
            &fixture(),
            "antigravity",
            Some(&config_path),
            &json!({
                "includeHistoryModelCatalog": false,
                "enableAgentCliModelLookup": true,
                "antigravityCliPath": display_path(executable),
            }),
        );

        assert!(
            catalog["models"]
                .as_array()
                .unwrap()
                .iter()
                .any(|model| model["name"] == "gemini-3.6-flash-medium")
        );
        assert!(
            !catalog["models"]
                .as_array()
                .unwrap()
                .iter()
                .any(|model| model["name"] == "gemini-3.1-pro-preview")
        );
        assert!(
            catalog["sources"]
                .as_array()
                .unwrap()
                .contains(&json!("antigravity-cli"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn antigravity_cli_model_lookup_uses_the_user_shell_environment() {
        let dir = temp_test_dir("antigravity-cli-shell-env");
        let executable = dir.join("agent-models");
        fs::write(
            &executable,
            r#"#!/bin/sh
if [ "$LICO_TEST_SHELL_SNAPSHOT_MARKER" != "shell-snapshot-env" ]; then
exit 1
fi
printf 'gemini-account-model\nclaude-account-model\n'
"#,
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        // The lookup child observes the user shell snapshot; pin it so the
        // marker channel is explicit instead of ambient process inheritance.
        let _pin = crate::platform::user_shell_environment::pin_process_env_snapshot_for_testing(
            &[("LICO_TEST_SHELL_SNAPSHOT_MARKER", "shell-snapshot-env")],
        );
        let catalog = super::super::model_catalog_for_target(
            &fixture(),
            "antigravity",
            None,
            &json!({
                "homeDir": display_path(dir),
                "includeHistoryModelCatalog": false,
                "enableAgentCliModelLookup": true,
                "antigravityCliPath": display_path(executable),
            }),
        );
        let names = catalog["models"]
            .as_array()
            .unwrap()
            .iter()
            .map(|model| model["name"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert!(names.contains(&"gemini-account-model"));
        assert!(names.contains(&"claude-account-model"));
    }

    #[cfg(unix)]
    #[test]
    fn antigravity_cli_model_lookup_times_out_without_blocking_catalog() {
        let dir = temp_test_dir("antigravity-cli-timeout");
        let executable = dir.join("agent-models");
        fs::write(&executable, "#!/bin/sh\nsleep 30\n").unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let started = Instant::now();

        let catalog = model_catalog_for_target(
            &fixture(),
            "antigravity",
            None,
            &json!({
                "includeHistoryModelCatalog": false,
                "enableAgentCliModelLookup": true,
                "antigravityCliPath": display_path(executable),
                "antigravityCliModelLookupTimeoutMs": 100,
            }),
        );

        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(catalog["diagnostics"].as_array().unwrap().iter().any(
            |diagnostic| diagnostic["source"] == "antigravity-cli:models"
                && diagnostic["status"] == "timeout"
        ));
    }

    #[cfg(unix)]
    #[test]
    fn antigravity_cli_timeout_keeps_configured_settings_models_only() {
        // Mirrors the Adaptive Flywheel failure mode: when `agy models` does
        // not finish in time, only the models named in local settings remain.
        let dir = temp_test_dir("antigravity-cli-timeout-settings");
        let home = dir.join("home");
        let gemini = home.join(".gemini");
        fs::create_dir_all(gemini.join("antigravity-cli")).unwrap();
        fs::write(
            gemini.join("settings.json"),
            r#"{"model":{"name":"gemini-3.1-pro-preview"}}"#,
        )
        .unwrap();
        fs::write(
            gemini.join("antigravity-cli").join("settings.json"),
            r#"{"model":"Gemini 3.5 Flash (High)"}"#,
        )
        .unwrap();
        let executable = dir.join("agent-models");
        fs::write(&executable, "#!/bin/sh\nsleep 30\n").unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();

        let catalog = model_catalog_for_target(
            &fixture(),
            "antigravity",
            None,
            &json!({
                "homeDir": display_path(home),
                "includeHistoryModelCatalog": false,
                "enableAgentCliModelLookup": true,
                "antigravityCliPath": display_path(executable),
                "antigravityCliModelLookupTimeoutMs": 100,
            }),
        );

        let names = catalog["models"]
            .as_array()
            .unwrap()
            .iter()
            .map(|model| model["name"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"gemini-3.1-pro-preview"));
        assert!(names.contains(&"Gemini 3.5 Flash (High)"));
    }

    #[cfg(unix)]
    #[test]
    fn antigravity_local_catalog_unions_cli_instead_of_replacing() {
        let dir = temp_test_dir("antigravity-local-union");
        let home = dir.join("home");
        let available = home
            .join(".gemini")
            .join("antigravity")
            .join("available-models.json");
        fs::create_dir_all(available.parent().unwrap()).unwrap();
        fs::write(
            &available,
            json!({
                "models": {
                    "gemini-3-pro": { "displayName": "Gemini 3 Pro" },
                    "claude-sonnet-4-6": { "displayName": "Claude Sonnet 4.6" }
                }
            })
            .to_string(),
        )
        .unwrap();
        let executable = dir.join("agent-models");
        fs::write(
            &executable,
            r#"#!/bin/sh
printf 'gemini-3-flash\nclaude-sonnet-4-6\n'
"#,
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();

        let catalog = model_catalog_for_target(
            &fixture(),
            "antigravity",
            None,
            &json!({
                "homeDir": display_path(home),
                "includeHistoryModelCatalog": false,
                "enableAgentCliModelLookup": true,
                "antigravityCliPath": display_path(executable),
            }),
        );
        let names = catalog["models"]
            .as_array()
            .unwrap()
            .iter()
            .map(|model| model["name"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert!(
            names
                .iter()
                .any(|name| name.contains("Gemini 3 Pro") || *name == "gemini-3-pro")
        );
        assert!(names.contains(&"gemini-3-flash"));
        assert!(
            names
                .iter()
                .any(|name| name.contains("Claude Sonnet 4.6") || *name == "claude-sonnet-4-6")
        );
    }
}

mod cursor {
    use super::super::cursor::collect_cursor_model_catalog_from_cli_output;
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn cursor_cli_output_preserves_native_selector_ids() {
        let mut entries = BTreeMap::<String, ModelCatalogEntry>::new();
        let added = collect_cursor_model_catalog_from_cli_output(
            "Available models\n\ncursor-grok-4.5-high - Cursor Grok 4.5\ngemini-3.6-flash-high - Gemini 3.6 Flash\n\nTip: use --model <id>\n",
            "cursor-cli:models",
            &mut entries,
        );

        assert_eq!(added.added, 2);
        let grok = entries.get("cursor-grok-4.5-high").unwrap();
        assert_eq!(grok.name, "cursor-grok-4.5-high");
        assert_eq!(grok.display_name, "Cursor Grok 4.5");
        assert!(!entries.contains_key("Available models"));
    }

    #[cfg(unix)]
    #[test]
    fn cursor_cli_model_lookup_reads_installed_catalog() {
        let dir = temp_test_dir("cursor-cli-models");
        let executable = dir.join("cursor-agent");
        fs::write(
            &executable,
            r#"#!/bin/sh
printf 'Available models\n\nauto - Auto (default)\ncursor-grok-4.5-high - Cursor Grok 4.5\ncomposer-2.5 - Composer 2.5 (current)\n'
"#,
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();

        let catalog = model_catalog_for_target(
            &fixture(),
            "cursor",
            None,
            &json!({
                "includeHistoryModelCatalog": false,
                "enableAgentCliModelLookup": true,
                "cursorCliPath": display_path(executable),
            }),
        );

        assert!(catalog["models"].as_array().unwrap().iter().any(|model| {
            model["name"] == "cursor-grok-4.5-high" && model["displayName"] == "Cursor Grok 4.5"
        }));
        assert_eq!(catalog["defaultModel"], json!("auto"));
        assert!(catalog["models"].as_array().unwrap().iter().any(|model| {
            model["name"] == "composer-2.5" && model["displayName"] == "Composer 2.5"
        }));
        assert!(
            catalog["sources"]
                .as_array()
                .unwrap()
                .contains(&json!("cursor-cli"))
        );
    }

    #[test]
    fn cursor_cli_output_keeps_default_and_current_rows_separate() {
        let mut entries = BTreeMap::<String, ModelCatalogEntry>::new();
        let parsed = collect_cursor_model_catalog_from_cli_output(
            "Available models\n\nauto - Auto (default)\ncomposer-2.5 - Composer 2.5 (current)\n",
            "cursor-cli:models",
            &mut entries,
        );

        assert_eq!(parsed.default_model.as_deref(), Some("auto"));
        assert_eq!(parsed.current_model.as_deref(), Some("composer-2.5"));
        assert_eq!(entries.get("auto").unwrap().display_name, "Auto");
    }

    #[cfg(unix)]
    #[test]
    fn cursor_native_catalog_replaces_stale_config_models() {
        let dir = temp_test_dir("cursor-authoritative-models");
        let config_path = dir.join("settings.json");
        fs::write(&config_path, r#"{"model":"stale-cursor-model"}"#).unwrap();
        let executable = dir.join("cursor-agent");
        fs::write(
            &executable,
            r#"#!/bin/sh
printf 'Available models\n\ncomposer-2.5 - Composer 2.5 (current)\n'
"#,
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();

        let catalog = model_catalog_for_target(
            &fixture(),
            "cursor",
            Some(&config_path),
            &json!({
                "includeHistoryModelCatalog": false,
                "enableAgentCliModelLookup": true,
                "cursorCliPath": display_path(executable),
            }),
        );

        let names = catalog["models"]
            .as_array()
            .unwrap()
            .iter()
            .map(|model| model["name"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["composer-2.5"]);
        assert_eq!(catalog["defaultModel"], json!("composer-2.5"));
    }

    #[cfg(unix)]
    #[test]
    fn cursor_cli_model_lookup_uses_the_user_shell_environment() {
        let dir = temp_test_dir("cursor-cli-shell-env");
        let executable = dir.join("cursor-agent");
        fs::write(
            &executable,
            r#"#!/bin/sh
if [ "$LICO_TEST_SHELL_SNAPSHOT_MARKER" != "shell-snapshot-env" ]; then
printf 'Available models\n\nstale-isolated - Isolated\n'
exit 0
fi
printf 'Available models\n\nauto - Auto (default)\nfull-cursor-model - Full Cursor Model\n'
"#,
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        // The lookup child observes the user shell snapshot; pin it so the
        // marker channel is explicit instead of ambient process inheritance.
        let _pin = crate::platform::user_shell_environment::pin_process_env_snapshot_for_testing(
            &[("LICO_TEST_SHELL_SNAPSHOT_MARKER", "shell-snapshot-env")],
        );
        let catalog = super::super::model_catalog_for_target(
            &fixture(),
            "cursor",
            None,
            &json!({
                "homeDir": display_path(dir),
                "includeHistoryModelCatalog": false,
                "enableAgentCliModelLookup": true,
                "cursorCliPath": display_path(executable),
            }),
        );
        let names = catalog["models"]
            .as_array()
            .unwrap()
            .iter()
            .map(|model| model["name"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert!(names.contains(&"full-cursor-model"));
        assert!(!names.contains(&"stale-isolated"));
    }
}

mod kimi_code {
    use super::*;

    #[test]
    fn qualified_history_names_fold_into_official_native_ids() {
        let catalog = model_catalog_for_target(
            &fixture(),
            "kimi-code",
            None,
            &json!({
                "includeHistoryModelCatalog": false,
                "modelCatalogFixture": {
                    "kimi-code": {
                        "defaultModel": "kimi-code/k3",
                        "models": [
                            "k3",
                            "kimi-code/k3",
                            "k3-256k",
                            "kimi-code/k3-256k",
                            "kimi-for-coding",
                            "kimi-code/kimi-for-coding"
                        ]
                    }
                }
            }),
        );

        let names = catalog["models"]
            .as_array()
            .unwrap()
            .iter()
            .map(|model| model["name"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["k3", "k3-256k", "kimi-for-coding"]);
        assert_eq!(catalog["defaultModel"], json!("k3"));
    }
}

mod config_collections {
    use super::*;

    #[test]
    fn model_collection_cache_reads_root_model_array() {
        let dir = temp_test_dir("model-catalog-cache");
        let cache_path = dir.join("models.json");
        fs::write(
            &cache_path,
            json!([
                {
                    "id": "gpt-5.5",
                    "name": "GPT-5.5",
                    "vendor": "OpenAI"
                },
                {
                    "id": "claude-sonnet-4.6",
                    "name": "Claude Sonnet 4.6",
                    "vendor": "Anthropic"
                }
            ])
            .to_string(),
        )
        .unwrap();

        let mut entries = BTreeMap::<String, ModelCatalogEntry>::new();
        let mut diagnostics = Vec::<Value>::new();
        collect_model_catalog_from_model_collection_path(
            &cache_path,
            "model-cache",
            &mut entries,
            &mut diagnostics,
        );

        assert!(diagnostics.is_empty());
        assert!(entries.values().any(|entry| {
            entry.name == "GPT-5.5" && entry.provider.as_deref() == Some("OpenAI")
        }));
        assert!(entries.values().any(|entry| {
            entry.name == "Claude Sonnet 4.6" && entry.provider.as_deref() == Some("Anthropic")
        }));
    }
}

mod kilo {
    use super::super::kilo::collect_kilo_model_catalog_from_cli_output;
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn kilo_cli_output_preserves_every_provider_qualified_selector() {
        let mut entries = BTreeMap::<String, ModelCatalogEntry>::new();
        let added = collect_kilo_model_catalog_from_cli_output(
            "kilo/kilo-auto/free\nanthropic/claude-opus-4-6\nopenai/gpt-5.5\nignored log line\n",
            "kilo-cli:models",
            &mut entries,
        );

        assert_eq!(added, 3);
        let names = entries
            .values()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>();
        assert!(names.contains(&"kilo/kilo-auto/free"));
        assert!(names.contains(&"anthropic/claude-opus-4-6"));
        assert!(names.contains(&"openai/gpt-5.5"));
        assert_eq!(
            entries
                .values()
                .find(|entry| entry.name == "openai/gpt-5.5")
                .and_then(|entry| entry.provider_id.as_deref()),
            Some("openai")
        );
    }

    #[cfg(unix)]
    #[test]
    fn kilo_selected_catalog_uses_full_cli_output_and_user_shell_environment() {
        let home = temp_test_dir("kilo-cli-model-catalog");
        let executable = home.join("kilo");
        fs::write(
            &executable,
            r#"#!/bin/sh
if [ "$LICO_TEST_SHELL_SNAPSHOT_MARKER" != "shell-snapshot-env" ]; then
exit 1
fi
printf 'kilo/kilo-auto/free\nanthropic/claude-opus-4-6\nopenai/gpt-5.5\n'
"#,
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        // The lookup child observes the user shell snapshot; pin it so the
        // marker channel is explicit instead of ambient process inheritance.
        let _pin = crate::platform::user_shell_environment::pin_process_env_snapshot_for_testing(
            &[("LICO_TEST_SHELL_SNAPSHOT_MARKER", "shell-snapshot-env")],
        );
        let catalog = super::super::model_catalog_for_target(
            &fixture(),
            "kilo-code",
            None,
            &json!({
                "homeDir": display_path(home),
                "includeHistoryModelCatalog": false,
                "enableAgentCliModelLookup": true,
                "kiloCliPath": display_path(executable),
            }),
        );
        let names = catalog["models"]
            .as_array()
            .unwrap()
            .iter()
            .map(|model| model["name"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(names.len(), 3);
        assert!(names.contains(&"kilo/kilo-auto/free"));
        assert!(names.contains(&"anthropic/claude-opus-4-6"));
        assert!(names.contains(&"openai/gpt-5.5"));
        assert_eq!(catalog["sources"], json!(["kilo-cli"]));
    }

    #[test]
    fn kilo_model_catalog_reads_vscode_state_and_local_db() {
        let home = temp_test_dir("kilo-model-catalog");
        let vscode_root = match std::env::consts::OS {
            "windows" => default_app_data_dir(&home).join("Code"),
            "macos" => home
                .join("Library")
                .join("Application Support")
                .join("Code"),
            _ => home.join(".config").join("Code"),
        };
        let vscode_state = vscode_root
            .join("User")
            .join("globalStorage")
            .join("state.vscdb");
        fs::create_dir_all(vscode_state.parent().unwrap()).unwrap();
        let connection = Connection::open(&vscode_state).unwrap();
        connection
            .execute(
                "CREATE TABLE ItemTable (key TEXT PRIMARY KEY, value TEXT)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO ItemTable (key, value) VALUES (?1, ?2)",
                (
                    "kilocode.kilo-code",
                    json!({
                        "recentModels": [
                            {
                                "providerID": "kilo",
                                "modelID": "anthropic/claude-opus-4.6",
                                "variant": "max"
                            }
                        ],
                        "favoriteModels": [
                            {
                                "providerID": "kilo",
                                "modelID": "~anthropic/claude-opus-latest"
                            }
                        ],
                        "variantSelections": {
                            "agent/code/kilo/anthropic/claude-opus-4.6": "low"
                        }
                    })
                    .to_string(),
                ),
            )
            .unwrap();
        drop(connection);

        let kilo_db = home
            .join(".local")
            .join("share")
            .join("kilo")
            .join("kilo.db");
        fs::create_dir_all(kilo_db.parent().unwrap()).unwrap();
        let connection = Connection::open(&kilo_db).unwrap();
        connection
            .execute(
                "CREATE TABLE session_message (type TEXT, time_created INTEGER, data TEXT)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO session_message (type, time_created, data) VALUES (?1, ?2, ?3)",
                (
                    "model-switched",
                    1_i64,
                    json!({
                        "model": {
                            "providerID": "kilo",
                            "id": "deepseek/deepseek-v4",
                            "variant": "default"
                        }
                    })
                    .to_string(),
                ),
            )
            .unwrap();
        drop(connection);

        let catalog = model_catalog_for_target(
            &fixture(),
            "kilo-code",
            None,
            &json!({
                "homeDir": display_path(home),
                "includeHistoryModelCatalog": false,
                "enableAgentCliModelLookup": true,
                "disableKiloCliModelLookup": true,
            }),
        );
        let models = catalog["models"].as_array().unwrap();
        assert!(models.iter().any(|model| {
            model["name"] == "kilo/anthropic/claude-opus-4.6"
                && model["providerId"] == "kilo"
                && model["reasoningEfforts"]
                    .as_array()
                    .unwrap()
                    .contains(&json!("max"))
                && model["reasoningEfforts"]
                    .as_array()
                    .unwrap()
                    .contains(&json!("low"))
        }));
        assert!(models.iter().any(
            |model| model["name"] == "kilo/~anthropic/claude-opus-latest"
                && model["providerId"] == "kilo"
        ));
        assert!(
            models
                .iter()
                .any(|model| model["name"] == "kilo/deepseek/deepseek-v4"
                    && model["providerId"] == "kilo")
        );
    }

    #[test]
    fn kilo_model_catalog_excludes_session_identities() {
        let home = temp_test_dir("kilo-session-identities");
        let vscode_root = match std::env::consts::OS {
            "windows" => default_app_data_dir(&home).join("Code"),
            "macos" => home
                .join("Library")
                .join("Application Support")
                .join("Code"),
            _ => home.join(".config").join("Code"),
        };
        let vscode_state = vscode_root
            .join("User")
            .join("globalStorage")
            .join("state.vscdb");
        fs::create_dir_all(vscode_state.parent().unwrap()).unwrap();
        let connection = Connection::open(&vscode_state).unwrap();
        connection
            .execute(
                "CREATE TABLE ItemTable (key TEXT PRIMARY KEY, value TEXT)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO ItemTable (key, value) VALUES (?1, ?2)",
                (
                    "kilocode.kilo-code",
                    json!({
                        "recentModels": [
                            {
                                "providerID": "kilo",
                                "modelID": "anthropic/claude-sonnet-4.6"
                            },
                            {
                                "id": "session/ses_01fakekilosessionid"
                            }
                        ],
                        "variantSelections": {
                            "session/ses_01anotherfakeid": "max",
                            "agent/code/kilo/anthropic/claude-sonnet-4.6": "low"
                        }
                    })
                    .to_string(),
                ),
            )
            .unwrap();
        drop(connection);

        let kilo_db = home
            .join(".local")
            .join("share")
            .join("kilo")
            .join("kilo.db");
        fs::create_dir_all(kilo_db.parent().unwrap()).unwrap();
        let connection = Connection::open(&kilo_db).unwrap();
        connection
            .execute(
                "CREATE TABLE session (model TEXT, time_updated INTEGER)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO session (model, time_updated) VALUES (?1, ?2)",
                ("session/ses_01storedsession", 1_i64),
            )
            .unwrap();
        drop(connection);

        let catalog = model_catalog_for_target(
            &fixture(),
            "kilo-code",
            None,
            &json!({
                "homeDir": display_path(home),
                "includeHistoryModelCatalog": false,
                "enableAgentCliModelLookup": true,
                "disableKiloCliModelLookup": true,
            }),
        );
        let names = catalog["models"]
            .as_array()
            .unwrap()
            .iter()
            .map(|model| model["name"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert!(names.contains(&"kilo/anthropic/claude-sonnet-4.6"));
        assert!(
            !names
                .iter()
                .any(|name| name.contains("session/") || name.contains("ses_")),
            "session identities leaked into Kilo models: {names:?}"
        );
    }

    #[test]
    fn kilo_unused_agent_catalog_skips_other_app_vscode_state() {
        let home = temp_test_dir("kilo-unused-vscode");
        let vscode_root = match std::env::consts::OS {
            "windows" => default_app_data_dir(&home).join("Code"),
            "macos" => home
                .join("Library")
                .join("Application Support")
                .join("Code"),
            _ => home.join(".config").join("Code"),
        };
        let vscode_state = vscode_root
            .join("User")
            .join("globalStorage")
            .join("state.vscdb");
        fs::create_dir_all(vscode_state.parent().unwrap()).unwrap();
        let connection = Connection::open(&vscode_state).unwrap();
        connection
            .execute(
                "CREATE TABLE ItemTable (key TEXT PRIMARY KEY, value TEXT)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO ItemTable (key, value) VALUES (?1, ?2)",
                (
                    "kilocode.kilo-code",
                    json!({
                        "recentModels": [
                            {
                                "providerID": "kilo",
                                "modelID": "anthropic/claude-opus-4.6"
                            }
                        ]
                    })
                    .to_string(),
                ),
            )
            .unwrap();
        drop(connection);

        let unused = model_catalog_for_target(
            &fixture(),
            "kilo-code",
            None,
            &json!({
                "homeDir": display_path(home.clone()),
                "includeHistoryModelCatalog": false,
            }),
        );
        let selected = model_catalog_for_target(
            &fixture(),
            "kilo-code",
            None,
            &json!({
                "homeDir": display_path(home),
                "includeHistoryModelCatalog": false,
                "enableAgentCliModelLookup": true,
                "disableKiloCliModelLookup": true,
            }),
        );
        let unused_names = unused["models"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|model| model["name"].as_str())
            .collect::<Vec<_>>();
        let selected_names = selected["models"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|model| model["name"].as_str())
            .collect::<Vec<_>>();
        let unused_has_vscode = unused_names.contains(&"kilo/anthropic/claude-opus-4.6");
        assert!(selected_names.contains(&"kilo/anthropic/claude-opus-4.6"));
        if cfg!(target_os = "macos") {
            assert!(
                !unused_has_vscode,
                "unused-agent Kilo catalog must not stat VS Code Application Support"
            );
        }
        assert!(
            selected["sources"]
                .as_array()
                .unwrap()
                .contains(&json!("kilo-vscode-state"))
        );
    }
}

mod history {
    use super::super::history::history_model_catalog_params;
    use super::*;

    #[test]
    fn history_projection_forwards_only_bounded_discovery_parameters() {
        let ignored_secret = ["must", "not", "forward"].join("-");
        let projected = history_model_catalog_params(
            "codex",
            &json!({
                "homeDir": "/bounded-home",
                "historyRoot": "/bounded-history",
                "historyModelCatalogLimit": 12,
                "historyModelCatalogFileLimit": 7,
                "secret": ignored_secret
            }),
        );

        assert_eq!(projected["agent"], "codex");
        assert_eq!(projected["limit"], 12);
        assert_eq!(projected["historyModelCatalogFileLimit"], 7);
        assert_eq!(projected["historyRoot"], "/bounded-history");
        assert!(projected.get("secret").is_none());
    }
}

mod normalization {
    use super::*;

    #[test]
    fn default_model_projection_reads_explicit_nested_native_state_only() {
        let document = json!({
            "env": {
                "ANTHROPIC_MODEL": "deepseek-v4-pro[1m]"
            },
            "profiles": {
                "review": {
                    "selectedModel": "profile-only-model"
                }
            }
        });

        assert_eq!(
            default_model_name_from_config_document(&document).as_deref(),
            Some("deepseek-v4-pro[1m]")
        );
        assert_eq!(
            default_model_name_from_config_document(&json!({
                "profiles": {
                    "review": {
                        "selectedModel": "profile-only-model"
                    }
                }
            })),
            None
        );
    }

    #[test]
    fn default_model_projection_reads_flagged_catalog_entry() {
        let document = json!({
            "themes": [
                {"name": "dark", "default": true}
            ],
            "models": [
                {"name": "model-a"},
                {"name": "model-b", "isDefault": true}
            ]
        });

        assert_eq!(
            default_model_name_from_config_document(&document).as_deref(),
            Some("model-b")
        );
    }

    #[test]
    fn model_normalization_rejects_untrusted_names_and_canonicalizes_known_families() {
        assert_eq!(canonical_model_display_name("gpt-5.5-mini"), "GPT-5.5 Mini");
        assert_eq!(
            canonical_model_display_name("deepseek-v4-pro"),
            "DeepSeek V4 Pro"
        );
        assert!(sanitize_model_name("https://example.invalid/model").is_none());
        assert!(sanitize_model_name("$MODEL_FROM_ENV").is_none());
        assert!(sanitize_model_name("model-with-api_key").is_none());
    }
}

mod provider {
    use super::*;

    #[test]
    fn provider_projection_uses_canonical_labels_without_inventing_identity() {
        assert_eq!(
            provider_label_from_provider_id("openai").as_deref(),
            Some("OpenAI")
        );
        assert_eq!(
            provider_label_from_provider_id("custom-provider").as_deref(),
            Some("Custom Provider")
        );
        assert!(provider_label_from_provider_id("  ").is_none());
    }
}

mod reasoning {
    use super::*;

    #[test]
    fn reasoning_projection_collects_nested_unique_bounded_options() {
        let efforts = reasoning_efforts_from_value(&json!({
            "reasoning": {
                "supportedReasoningEfforts": ["low", "high", "high"]
            },
                "thinking": {"thinkingLevel": "medium"}
        }));

        assert_eq!(
            efforts.into_iter().collect::<Vec<_>>(),
            vec!["high", "low", "medium"]
        );
    }
}

mod merge {
    use super::*;

    #[test]
    fn model_merge_deduplicates_sources_and_preserves_explicit_provider() {
        let mut entries = BTreeMap::<String, ModelCatalogEntry>::new();
        add_model_catalog_entry_with_provider(
            &mut entries,
            "gpt-5.5",
            Some("GPT-5.5"),
            Some("openai"),
            Some("OpenAI"),
            "config",
            ["high".to_string()].into_iter().collect(),
        );
        add_model_catalog_entry_with_provider(
            &mut entries,
            "gpt-5.5",
            None,
            Some("openai"),
            None,
            "history",
            ["low".to_string()].into_iter().collect(),
        );

        let catalog = build_model_catalog(
            &fixture(),
            "codex",
            entries,
            ["config".to_string(), "history".to_string()]
                .into_iter()
                .collect(),
            Vec::new(),
            None,
        );
        assert_eq!(catalog["models"].as_array().unwrap().len(), 1);
        assert_eq!(catalog["models"][0]["provider"], "OpenAI");
        assert_eq!(
            catalog["models"][0]["sources"],
            json!(["config", "history"])
        );
        assert_eq!(
            catalog["models"][0]["reasoningEfforts"],
            json!(["low", "high"])
        );
    }
}

mod builtin {
    use super::*;

    fn catalog_with_fixture(target: &str, fixture: Value) -> Value {
        let home = temp_test_dir("builtin-overlay-home");
        // The parameter above shadows the module's `fixture()`, so the port is
        // named where it is built.
        model_catalog_for_target(
            &crate::port::fixtures::packaged(),
            target,
            None,
            &json!({
                "homeDir": display_path(home),
                "includeHistoryModelCatalog": false,
                "modelCatalogFixture": { target: fixture }
            }),
        )
    }

    fn model<'a>(catalog: &'a Value, name: &str) -> Option<&'a Value> {
        catalog["models"]
            .as_array()
            .unwrap()
            .iter()
            .find(|model| model["name"] == name)
    }

    #[test]
    fn builtin_catalog_json_is_well_formed() {
        let parsed: Value = serde_json::from_str(include_str!("builtin_catalog.json")).unwrap();
        let agents = parsed["agents"].as_object().unwrap();
        assert!(agents.contains_key("codex"));
        for (agent, rows) in agents {
            let mut names = BTreeSet::new();
            for row in rows["models"].as_array().unwrap() {
                let name = row["name"].as_str().unwrap();
                assert!(!name.trim().is_empty(), "empty model name in {agent}");
                assert!(
                    names.insert(name.to_ascii_lowercase()),
                    "duplicate {name} in {agent}"
                );
                assert!(
                    row["reasoningEfforts"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .all(|effort| effort
                            .as_str()
                            .is_some_and(|value| !value.trim().is_empty())),
                    "blank effort in {agent}/{name}"
                );
            }
        }
    }

    #[test]
    fn builtin_overlay_replaces_efforts_in_table_order_and_marks_sources() {
        let catalog = catalog_with_fixture(
            "codex",
            json!({
                "models": [
                    { "name": "gpt-5.6-sol", "reasoningEfforts": ["max"] },
                    { "name": "deepseek-v4-pro", "reasoningEfforts": ["high"] }
                ]
            }),
        );

        assert_eq!(
            model(&catalog, "gpt-5.6-sol").unwrap()["reasoningEfforts"],
            json!(["low", "medium", "high", "xhigh", "max", "ultra"])
        );
        assert!(
            model(&catalog, "gpt-5.6-sol").unwrap()["sources"]
                .as_array()
                .unwrap()
                .contains(&json!("builtin"))
        );
        assert_eq!(
            model(&catalog, "deepseek-v4-pro").unwrap()["reasoningEfforts"],
            json!(["high"])
        );
        assert!(
            catalog["sources"]
                .as_array()
                .unwrap()
                .contains(&json!("builtin"))
        );
    }

    #[test]
    fn builtin_overlay_never_injects_unscanned_models() {
        let catalog = catalog_with_fixture(
            "codex",
            json!({ "models": [{ "name": "gpt-5.2", "reasoningEfforts": ["high"] }] }),
        );

        assert!(model(&catalog, "gpt-5.6-terra").is_none());
        assert!(model(&catalog, "gpt-5.6-luna").is_none());
    }

    #[test]
    fn antigravity_never_exposes_a_separate_reasoning_effort() {
        let catalog = catalog_with_fixture(
            "antigravity",
            json!({
                "models": [
                    { "name": "gemini-3.5-flash", "reasoningEfforts": ["low", "high"] },
                    { "name": "Claude Opus 4.6 (Thinking)", "reasoningEfforts": ["high"] }
                ]
            }),
        );

        assert!(
            catalog["models"]
                .as_array()
                .unwrap()
                .iter()
                .all(|model| model["reasoningEfforts"].as_array().unwrap().is_empty())
        );
    }

    #[test]
    fn cursor_never_exposes_a_separate_reasoning_effort() {
        let catalog = catalog_with_fixture(
            "cursor",
            json!({
                "models": [
                    {
                        "name": "fable-5-1m-medium",
                        "displayName": "Fable 5 1M Medium",
                        "reasoningEfforts": ["low", "medium", "high"]
                    },
                    {
                        "name": "gpt-5.4",
                        "reasoningEfforts": ["low", "high"]
                    }
                ]
            }),
        );

        assert!(
            catalog["models"]
                .as_array()
                .unwrap()
                .iter()
                .all(|model| model["reasoningEfforts"].as_array().unwrap().is_empty())
        );
    }

    #[test]
    fn builtin_overlay_matches_provider_prefixed_names() {
        let catalog = catalog_with_fixture(
            "openclaw",
            json!({
                "models": [
                    { "name": "moonshot/kimi-k3", "reasoningEfforts": ["low", "high"] }
                ]
            }),
        );

        assert_eq!(
            model(&catalog, "moonshot/kimi-k3").unwrap()["reasoningEfforts"],
            json!(["max"])
        );
    }

    #[test]
    fn builtin_overlay_leaves_unlisted_agents_untouched() {
        let catalog = catalog_with_fixture(
            "hermes",
            json!({
                "models": [{ "name": "z-ai/glm-5.2", "reasoningEfforts": ["medium"] }]
            }),
        );

        assert_eq!(
            model(&catalog, "z-ai/glm-5.2").unwrap()["reasoningEfforts"],
            json!(["medium"])
        );
        assert!(
            !catalog["sources"]
                .as_array()
                .unwrap()
                .contains(&json!("builtin"))
        );
    }
}

mod claude_code {
    use super::*;

    #[test]
    fn catalog_lists_native_aliases_and_the_configured_current_model() {
        let home = temp_test_dir("claude-code-model-options");
        let settings_path = home.join("settings.json");
        fs::write(&settings_path, json!({
            "model": "sonnet",
            "env": {"ANTHROPIC_MODEL": "deepseek-v4-flash", "ANTHROPIC_DEFAULT_OPUS_MODEL": "claude-opus-4-8"}
        }).to_string()).unwrap();
        let catalog = model_catalog_for_target(
            &fixture(),
            "claude-code",
            Some(&settings_path),
            &json!({"homeDir": display_path(home)}),
        );
        let rows = catalog["models"].as_array().unwrap();
        for name in [
            "default",
            "opus",
            "opus[1m]",
            "sonnet",
            "sonnet[1m]",
            "haiku",
            "opusplan",
            "deepseek-v4-flash",
        ] {
            assert!(rows.iter().any(|row| row["name"] == name), "missing {name}");
        }
        let opus = rows.iter().find(|row| row["name"] == "opus").unwrap();
        assert_eq!(opus["provider"], "Anthropic");
        assert_eq!(opus["displayName"], "Claude Opus 4.8");
        assert_eq!(
            opus["reasoningEfforts"],
            json!(["low", "medium", "high", "xhigh", "max"])
        );
        assert_eq!(opus["providerInferred"], true);
        assert_eq!(catalog["defaultModel"], "deepseek-v4-flash");
        assert_eq!(catalog["sources"], json!(["claude-settings"]));
    }

    #[test]
    fn configured_fable_alias_uses_its_pinned_admitted_model() {
        let home = temp_test_dir("claude-code-fable-alias");
        let path = home.join("settings.json");
        fs::write(&path, json!({"availableModels": ["fable"], "env": {"ANTHROPIC_DEFAULT_FABLE_MODEL": "claude-fable-5-1"}}).to_string()).unwrap();
        let catalog = model_catalog_for_target(
            &fixture(),
            "claude-code",
            Some(&path),
            &json!({"homeDir": display_path(home)}),
        );
        let rows = catalog["models"].as_array().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["name"], "fable");
        assert_eq!(rows[0]["displayName"], "Claude Fable 5.1");
        assert_eq!(
            rows[0]["reasoningEfforts"],
            json!(["low", "medium", "high", "xhigh", "max"])
        );
    }

    #[test]
    fn catalog_honors_available_models_and_always_keeps_default() {
        let home = temp_test_dir("claude-code-allowed-models");
        let settings_path = home.join("settings.json");
        fs::write(
            &settings_path,
            json!({"model": "opus", "availableModels": ["sonnet", "haiku"]}).to_string(),
        )
        .unwrap();
        let catalog = model_catalog_for_target(
            &fixture(),
            "claude-code",
            Some(&settings_path),
            &json!({"homeDir": display_path(home.clone())}),
        );
        let names = catalog["models"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["name"].as_str().unwrap())
            .collect::<BTreeSet<_>>();
        assert_eq!(names, BTreeSet::from(["default", "sonnet", "haiku"]));
        assert_eq!(catalog["defaultModel"], "default");
        fs::write(&settings_path, r#"{"availableModels":[]}"#).unwrap();
        let empty = model_catalog_for_target(
            &fixture(),
            "claude-code",
            Some(&settings_path),
            &json!({"homeDir": display_path(home)}),
        );
        assert_eq!(empty["models"].as_array().unwrap().len(), 1);
        assert_eq!(empty["models"][0]["name"], "default");
    }

    #[test]
    fn catalog_lists_native_aliases_without_a_configured_model() {
        let home = temp_test_dir("claude-code-default-model");
        let catalog = model_catalog_for_target(
            &fixture(),
            "claude-code",
            None,
            &json!({"homeDir": display_path(home)}),
        );
        assert_eq!(catalog["models"].as_array().unwrap().len(), 7);
        assert_eq!(catalog["defaultModel"], "default");
    }
}

mod opencode {
    use super::super::opencode::{
        collect_opencode_models_from_cli_output, collect_opencode_provider_catalog,
    };
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn opencode_provider_config_surfaces_qualified_models() {
        let home = temp_test_dir("opencode-provider-catalog");
        let config_path = home.join(".config").join("opencode").join("opencode.jsonc");
        fs::create_dir_all(config_path.parent().unwrap()).unwrap();
        fs::write(
            &config_path,
            json!({
                "provider": {
                    "anthropic": {
                        "name": "Anthropic",
                        "models": {
                            "claude-sonnet-4-5": { "name": "Claude Sonnet 4.5" },
                            "claude-opus-4-6": { "name": "Claude Opus 4.6" }
                        }
                    },
                    "openai": {
                        "models": ["gpt-5.4"]
                    }
                }
            })
            .to_string(),
        )
        .unwrap();

        let catalog = model_catalog_for_target(
            &fixture(),
            "opencode",
            Some(&config_path),
            &json!({
                "homeDir": display_path(home),
                "includeHistoryModelCatalog": false,
            }),
        );
        let names = catalog["models"]
            .as_array()
            .unwrap()
            .iter()
            .map(|model| model["name"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert!(names.contains(&"anthropic/claude-sonnet-4-5"));
        assert!(names.contains(&"anthropic/claude-opus-4-6"));
        assert!(names.contains(&"openai/gpt-5.4"));
        assert!(names.iter().all(|name| name.contains('/')));
        assert!(!names.contains(&"Claude Sonnet 4.5"));
        let anthropic = catalog["models"]
            .as_array()
            .unwrap()
            .iter()
            .find(|model| model["name"] == "anthropic/claude-sonnet-4-5")
            .unwrap();
        assert_eq!(anthropic["providerId"], json!("anthropic"));
        assert_eq!(anthropic["provider"], json!("Anthropic"));
    }

    #[test]
    fn opencode_cli_output_reads_provider_scoped_lines() {
        let mut entries = BTreeMap::<String, ModelCatalogEntry>::new();
        let added = collect_opencode_models_from_cli_output(
            "anthropic/claude-sonnet-4-5\nopenai/gpt-5.4 - GPT-5.4\n",
            "opencode-cli:models",
            &mut entries,
        );
        assert_eq!(added, 2);
        assert!(
            entries
                .values()
                .any(|entry| entry.name == "anthropic/claude-sonnet-4-5"
                    && entry.provider_id.as_deref() == Some("anthropic"))
        );
        assert!(entries.values().any(|entry| entry.name == "openai/gpt-5.4"
            && entry.provider_id.as_deref() == Some("openai")));
    }

    #[test]
    fn opencode_json_providers_are_not_an_empty_catalog() {
        let mut entries = BTreeMap::<String, ModelCatalogEntry>::new();
        collect_opencode_provider_catalog(
            &json!({
                "providers": [
                    {
                        "id": "anthropic",
                        "models": { "claude-haiku-4-5": { "name": "Haiku 4.5" } }
                    }
                ]
            }),
            "opencode-config",
            &mut entries,
        );
        assert!(
            entries
                .values()
                .any(|entry| entry.name == "anthropic/claude-haiku-4-5")
        );
    }

    #[test]
    fn opencode_cli_json_array_reads_provider_scoped_models() {
        let mut entries = BTreeMap::<String, ModelCatalogEntry>::new();
        let added = collect_opencode_models_from_cli_output(
            r#"["anthropic/claude-sonnet-4-5","openai/gpt-5.4"]"#,
            "opencode-cli:models",
            &mut entries,
        );
        assert_eq!(added, 2);
        assert!(
            entries
                .values()
                .any(|entry| entry.name == "anthropic/claude-sonnet-4-5")
        );
        assert!(entries.values().any(|entry| entry.name == "openai/gpt-5.4"));
    }

    #[cfg(unix)]
    #[test]
    fn opencode_cli_model_lookup_fills_provider_catalog() {
        let dir = temp_test_dir("opencode-cli-models");
        let executable = dir.join("opencode");
        fs::write(
            &executable,
            r#"#!/bin/sh
printf 'anthropic/claude-sonnet-4-5\nopenai/gpt-5.4\n'
"#,
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();

        let catalog = model_catalog_for_target(
            &fixture(),
            "opencode",
            None,
            &json!({
                "includeHistoryModelCatalog": false,
                "enableAgentCliModelLookup": true,
                "opencodeCliPath": display_path(executable),
            }),
        );
        let names = catalog["models"]
            .as_array()
            .unwrap()
            .iter()
            .map(|model| model["name"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert!(names.contains(&"anthropic/claude-sonnet-4-5"));
        assert!(names.contains(&"openai/gpt-5.4"));
        assert!(
            catalog["sources"]
                .as_array()
                .is_some_and(|sources| sources.contains(&json!("opencode-cli:models")))
        );
    }

    #[cfg(unix)]
    #[test]
    fn opencode_provider_without_nested_models_uses_cli_catalog() {
        let dir = temp_test_dir("opencode-provider-cli-fill");
        let home = dir.join("home");
        let config_path = home.join(".config").join("opencode").join("opencode.json");
        fs::create_dir_all(config_path.parent().unwrap()).unwrap();
        fs::write(
            &config_path,
            json!({
                "provider": {
                    "anthropic": { "npm": "@ai-sdk/anthropic" },
                    "openai": { "npm": "@ai-sdk/openai" }
                }
            })
            .to_string(),
        )
        .unwrap();
        let executable = dir.join("opencode");
        fs::write(
            &executable,
            r#"#!/bin/sh
printf 'anthropic/claude-sonnet-4-5\nopenai/gpt-5.4\n'
"#,
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();

        let catalog = model_catalog_for_target(
            &fixture(),
            "opencode",
            Some(&config_path),
            &json!({
                "homeDir": display_path(home),
                "includeHistoryModelCatalog": false,
                "enableAgentCliModelLookup": true,
                "opencodeCliPath": display_path(executable),
            }),
        );
        let names = catalog["models"]
            .as_array()
            .unwrap()
            .iter()
            .map(|model| model["name"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert!(names.contains(&"anthropic/claude-sonnet-4-5"));
        assert!(names.contains(&"openai/gpt-5.4"));
        assert!(!names.is_empty());
    }
}

fn display_path(path: PathBuf) -> String {
    path.to_string_lossy().to_string()
}

fn temp_test_dir(name: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let dir = std::env::temp_dir().join(format!(
        "lico-target-model-catalog-{name}-{}-{}",
        now.as_secs(),
        now.subsec_nanos(),
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

mod picker_revision {
    use super::*;

    fn fixture_catalog(target: &str, models: Value) -> Value {
        let home = temp_test_dir("picker-catalog");
        model_catalog_for_target(
            &fixture(),
            target,
            None,
            &json!({
                "homeDir": display_path(home),
                "includeHistoryModelCatalog": false,
                "disableAntigravityCliModelLookup": true,
                "modelCatalogFixture": {target: {"models": models}}
            }),
        )
    }

    #[test]
    fn codex_official_models_precede_custom_and_versions_sort_numerically() {
        let catalog = fixture_catalog(
            "codex",
            json!([
                {"name": "gateway/gpt-8", "providerId": "gateway", "provider": "Gateway"},
                {"name": "gpt-5.9"},
                {"name": "gpt-6-astra"},
                {"name": "gpt-5.10"},
                {"name": "gpt-5.6-sol"},
                {"name": "gpt-5.6-luna"},
                {"name": "gpt-5.6-terra"},
                {"name": "gateway/gpt-7", "providerId": "gateway", "provider": "Gateway"}
            ]),
        );
        let rows = catalog["models"].as_array().unwrap();
        let names = rows
            .iter()
            .map(|row| row["name"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            [
                "gpt-6-astra",
                "gpt-5.10",
                "gpt-5.9",
                "gpt-5.6-sol",
                "gpt-5.6-terra",
                "gpt-5.6-luna",
                "gateway/gpt-8",
                "gateway/gpt-7"
            ]
        );
        assert_eq!(rows[0]["displayName"], "GPT-6 Astra");
        assert_eq!(rows[0]["provider"], "OpenAI");
    }

    #[test]
    fn antigravity_groups_existing_models_by_their_vendor() {
        let catalog = fixture_catalog(
            "antigravity",
            json!([
                "gpt-oss-120b",
                "claude-opus-4-8",
                "gemini-3.6-flash",
                "gemini-3.7-flash"
            ]),
        );
        let rows = catalog["models"].as_array().unwrap();
        let providers = rows
            .iter()
            .map(|row| row["provider"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(providers, ["Google", "Google", "Anthropic", "OpenAI"]);
        assert_eq!(rows[0]["name"], "gemini-3.7-flash");
        assert!(
            rows.iter()
                .all(|row| row["reasoningEfforts"].as_array().unwrap().is_empty())
        );
    }

    #[test]
    fn scanned_efforts_keep_supported_values_in_increasing_order() {
        let catalog = fixture_catalog(
            "codex",
            json!([{"name": "custom-new-model", "reasoningEfforts": ["max", "high", "xhigh", "low", "medium"]}]),
        );
        assert_eq!(
            catalog["models"][0]["reasoningEfforts"],
            json!(["low", "medium", "high", "xhigh", "max"])
        );
    }

    #[test]
    fn kimi_config_preserves_selector_and_provider_and_per_model_efforts() {
        let home = temp_test_dir("kimi-provider-models");
        let path = home.join("config.toml");
        fs::write(
            &path,
            r#"
default_model = "kimi-code/kimi-k3"
[providers.kimi-code]
type = "kimi"
[providers.gateway]
type = "openai"
name = "Gateway"
[models."gateway/gemini-3.6-flash"]
provider = "gateway"
model = "gemini-3.6-flash"
support_efforts = ["high", "low"]
[models."gateway/gemini-3.7-flash"]
provider = "gateway"
model = "gemini-3.7-flash"
support_efforts = []
[models."kimi-code/kimi-k3"]
provider = "kimi-code"
model = "kimi-k3"
support_efforts = ["max"]
"#,
        )
        .unwrap();
        let catalog = model_catalog_for_target(
            &fixture(),
            "kimi-code",
            Some(&path),
            &json!({"homeDir": display_path(home), "includeHistoryModelCatalog": false}),
        );
        let rows = catalog["models"].as_array().unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0]["name"], "kimi-code/kimi-k3");
        assert_eq!(rows[0]["provider"], "Kimi Code");
        assert_eq!(rows[0]["displayName"], "Kimi K3");
        assert_eq!(rows[1]["name"], "gateway/gemini-3.7-flash");
        assert_eq!(rows[1]["provider"], "Gateway");
        assert_eq!(rows[1]["reasoningEfforts"], json!([]));
        assert_eq!(rows[2]["reasoningEfforts"], json!(["low", "high"]));
        assert_eq!(catalog["defaultModel"], "kimi-code/kimi-k3");
    }
}

mod deepseek {
    use super::super::deepseek::SOURCE;
    use super::*;

    #[test]
    fn disabled_lookup_does_not_publish_an_installed_catalogue() {
        let catalog = model_catalog_for_target(
            &fixture(),
            "deepseek-harness",
            None,
            &json!({ "enableAgentCliModelLookup": false }),
        );
        assert!(catalog["models"].as_array().unwrap().is_empty());
        assert!(
            catalog["diagnostics"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| { item["source"] == SOURCE && item["status"] == "disabled" })
        );
    }

    // The catalogue is the adapter package's own data, so reading it starts no
    // process, resolves nothing on disk and needs no installed Harness — which
    // is exactly what this test asserts by passing no paths at all.
    #[test]
    fn installed_catalog_preserves_native_ids_provider_and_supported_efforts() {
        let catalog = model_catalog_for_target(
            &fixture(),
            "deepseek-harness",
            None,
            &json!({ "enableAgentCliModelLookup": true }),
        );
        let models = catalog["models"].as_array().unwrap();
        assert_eq!(models.len(), 2, "the provider advertises two advisory rows");
        let flash = models
            .iter()
            .find(|model| model["name"] == "deepseek-flash")
            .unwrap();
        assert_eq!(flash["displayName"], "DeepSeek V41 Flash");
        assert_eq!(flash["providerId"], "deepseek-official");
        assert_eq!(flash["provider"], "DeepSeek");
        assert_eq!(
            flash["reasoningEfforts"],
            json!(["off"]),
            "the provider advertises this row as not reasoning"
        );
        let pro = models
            .iter()
            .find(|model| model["name"] == "deepseek-v4-pro")
            .unwrap();
        assert_eq!(pro["displayName"], "DeepSeek V4 Pro");
        assert_eq!(
            pro["reasoningEfforts"],
            json!(["off", "low", "high", "max"]),
            "the reasoning row advertises the provider's own efforts"
        );
        assert!(
            catalog["sources"]
                .as_array()
                .unwrap()
                .contains(&json!(SOURCE))
        );
        assert_eq!(catalog["defaultModel"], "");
    }

    // A harness path is still a scan fact: naming one that cannot execute does
    // not make the catalogue disappear, because the catalogue is data.
    #[test]
    fn a_named_but_unextractable_harness_still_answers_its_own_catalogue() {
        let catalog = model_catalog_for_target(
            &fixture(),
            "deepseek-harness",
            None,
            &json!({
                "enableAgentCliModelLookup": true,
                "deepseekHarnessCliPath": "not-a-real-harness",
                "deepseekHarnessNodePath": "not-a-real-node",
            }),
        );
        assert_eq!(catalog["models"].as_array().unwrap().len(), 2);
        assert!(
            catalog["diagnostics"]
                .as_array()
                .unwrap()
                .iter()
                .all(|item| item["status"] != "execution-denied"),
            "no process is started, so nothing can be denied execution"
        );
    }
}
