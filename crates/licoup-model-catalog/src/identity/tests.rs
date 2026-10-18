use super::*;
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};

/// The Agent declaration labels production composition supplies. The registry
/// strips a label that names a source before it matches a model, so a fixture
/// states the declarations its expectations depend on instead of inheriting a
/// host's. The list is the packaged Agent ids and labels the inventory composes.
const FIXTURE_AGENT_LABELS: &[&str] = &[
    "openclaw",
    "claude-code",
    "Claude Code",
    "codex",
    "Codex",
    "antigravity",
    "Antigravity",
    "opencode",
    "OpenCode",
    "copilot",
    "Copilot",
    "kilo-code",
    "Kilo Code",
    "cursor",
    "Cursor",
    "cursor-cli",
    "Cursor CLI",
    "hermes",
    "Hermes",
    "kimi-code",
    "Kimi Code",
    "pi",
    "Pi",
    "lico-agent",
    "Lico Agent",
    "deepseek-harness",
    "DeepSeek Harness",
];

fn fixture_registry(value: Value) -> Result<RegistrySnapshot> {
    RegistrySnapshot::from_catalog_with_wrappers(value, FIXTURE_AGENT_LABELS)
}

fn fixture_document(catalog: &CatalogDocument) -> Result<RegistrySnapshot> {
    RegistrySnapshot::from_document_with(catalog, FIXTURE_AGENT_LABELS)
}

fn catalog() -> Value {
    json!({
        "models": {
            "moonshotai/kimi-k3": {"name": "Kimi K3", "family": "kimi"},
            "moonshotai/kimi-k2.7-code": {"name": "Kimi K2.7 Code"},
            "xai/grok-4.6": {"name": "Grok 4.6"},
            "xai/grok-4.6-fast": {"name": "Grok 4.6 Fast"},
            "example/vision-2": {"name": "Example Vision 2"},
            "example/vision-3": {"name": "Example Vision 3"}
        },
        "providers": {
            "kimi-for-coding": {"name": "Kimi Code", "models": {
                "k3": {"name": "Kimi K3", "base_model": "moonshotai/kimi-k3"},
                "k3-256k": {"name": "Kimi Code K3 256K", "base_model": "moonshotai/kimi-k3"},
                "kimi-for-coding": {"name": "Kimi Code", "base_model": "moonshotai/kimi-k2.7-code"}
            }},
            "opencode-go": {"name": "OpenCode Go", "models": {
                "kimi-k3": {"name": "Kimi K3", "base_model": "moonshotai/kimi-k3"}
            }},
            "xai": {"name": "xAI", "models": {
                "grok-4.6": {"name": "Grok 4.6", "base_model": "xai/grok-4.6"}
            }}
        }
    })
}

fn downloaded(value: Value) -> Result<source::DownloadedCatalog> {
    Ok(source::DownloadedCatalog {
        catalog: serde_json::from_value(value)?,
        source: source::REPOSITORY_SOURCE.to_owned(),
        skipped_entries: 2,
    })
}

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "licoup-model-registry-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn explicit_provider_links_merge_context_agent_and_effort_wrappers() {
    let snapshot = fixture_registry(catalog()).unwrap();
    for name in [
        "k3",
        "kimi-k3",
        "kimi-code-k3",
        "kimi-code-k3-256k",
        "kimi-for-coding/k3",
        "opencode-go/kimi-k3",
        "Cursor Kimi K3 Extra High Fast",
        "Kimi Code K3.256k",
        "Kimi K3 (256k) High",
        "Kimi K3 [256k]",
    ] {
        assert_eq!(
            snapshot
                .resolve(name, Some("cursor"))
                .map(|model| model.id.as_str()),
            Some("moonshotai/kimi-k3"),
            "{name}"
        );
    }
    assert!(snapshot.resolve("kimi-for-coding", None).is_none());
    assert!(snapshot.is_provider_label("kimi-for-coding"));
    assert!(snapshot.is_provider_label("Cursor"));
    assert!(snapshot.is_provider_label("xAI"));
    assert!(!snapshot.is_provider_label("kimi-for-coding/k3"));
    assert_eq!(
        snapshot
            .resolve("kimi-for-coding/kimi-for-coding", None)
            .unwrap()
            .id,
        "moonshotai/kimi-k2.7-code"
    );
    assert_eq!(
        snapshot
            .resolve_with_provider("kimi-for-coding", Some("kimi-for-coding"), Some("opencode"))
            .unwrap()
            .id,
        "moonshotai/kimi-k2.7-code"
    );
    assert!(
        snapshot
            .resolve_with_provider("kimi-for-coding", Some("opencode-go"), Some("opencode"))
            .is_none()
    );
    assert!(
        snapshot
            .resolve_with_provider("Cursor", Some("kimi-for-coding"), Some("cursor"))
            .is_none()
    );
    assert!(snapshot.resolve("Cursor", Some("cursor")).is_none());
    assert!(snapshot.resolve("Kimi K4", None).is_none());
}

#[test]
fn composer_speed_tiers_share_one_identity_across_catalogs_and_agents() {
    let mut value = catalog();
    value["models"]["cursor/composer-2.5"] = json!({"name":"Composer 2.5"});
    value["models"]["cursor/composer-2.5-fast"] = json!({"name":"Composer 2.5 Fast"});
    value["providers"]["relay"] = json!({"models": {
        "composer-2.5-fast": {"base_model":"cursor/composer-2.5-fast"}
    }});
    let snapshot = fixture_registry(value.clone()).unwrap();
    assert_eq!(
        snapshot.models.len(),
        catalog()["models"].as_object().unwrap().len() + 1
    );
    assert_eq!(
        snapshot
            .resolve("relay/composer-2.5-fast", None)
            .unwrap()
            .id,
        "cursor/composer-2.5"
    );
    value["models"]
        .as_object_mut()
        .unwrap()
        .remove("cursor/composer-2.5");
    let fast_only = fixture_registry(value).unwrap();
    for registry in [
        &snapshot,
        &fast_only,
        &RegistrySnapshot::empty_with_wrappers(FIXTURE_AGENT_LABELS),
    ] {
        for agent in [None, Some("cursor"), Some("opencode"), Some("codex")] {
            for raw in [
                "composer-2.5",
                "composer-2.5-fast",
                "cursor/composer-2.5-fast",
                "Composer 2.5 (Fast)",
                "Cursor Composer 2.5 Fast",
            ] {
                for model in [
                    registry.resolve_with_provider(raw, Some("cursor"), agent),
                    registry.resolve_historical(raw, Some("cursor"), agent),
                ] {
                    let model = model.unwrap();
                    assert_eq!(model.id, "cursor/composer-2.5", "{raw}");
                    assert_eq!(model.display_name, "Composer 2.5", "{raw}");
                }
            }
        }
        assert_eq!(
            registry
                .canonical_model("cursor/composer-2.5-fast")
                .unwrap()
                .id,
            "cursor/composer-2.5"
        );
        for raw in [
            "composer-2.6-fast",
            "composer-2.5-vision",
            "unrelated/composer-2.5-fast",
        ] {
            assert!(
                registry.resolve_historical(raw, None, None).is_none(),
                "{raw}"
            );
        }
    }
}

#[test]
fn actual_fast_versions_and_modalities_remain_distinct() {
    let snapshot = fixture_registry(catalog()).unwrap();
    assert_eq!(
        snapshot
            .resolve("Cursor Grok 4.6 Extra High Fast", Some("cursor"))
            .unwrap()
            .id,
        "xai/grok-4.6"
    );
    assert_eq!(
        snapshot.resolve("Grok 4.6 Fast", None).unwrap().id,
        "xai/grok-4.6-fast"
    );
    assert_ne!(
        snapshot.resolve("vision-2", None).unwrap().id,
        snapshot.resolve("vision-3", None).unwrap().id
    );
    for name in [
        "Kimi K3 Fast",
        "Kimi K3 Vision",
        "Grok 4.7",
        "Grok 46",
        "Grok 4.6 Thinking",
    ] {
        assert!(snapshot.resolve(name, None).is_none(), "{name}");
    }
}

#[test]
fn ambiguous_names_require_an_explicit_provider_identity() {
    let mut value = catalog();
    value["models"]["other/kimi-k3"] = json!({"name":"Kimi K3"});
    let snapshot = fixture_registry(value).unwrap();
    assert!(snapshot.resolve("Kimi K3", None).is_none());
    assert_eq!(
        snapshot.resolve("kimi-for-coding/k3", None).unwrap().id,
        "moonshotai/kimi-k3"
    );
    assert_eq!(
        snapshot.resolve("other/kimi-k3", None).unwrap().id,
        "other/kimi-k3"
    );
}

#[test]
fn numeric_versions_and_exact_ids_survive_loose_display_alias_collisions() {
    let snapshot = fixture_registry(json!({
        "models": {
            "lab/model-5.1": {"name":"Model 5.1"},
            "lab/model-51": {"name":"Model 51"},
            "lab/model-5.10": {"name":"Model 5.10"},
            "lab/model-51.0": {"name":"Model 51.0"},
            "other/conflicting-display": {"name":"lab/model-5.1"}
        },
        "providers": {"relay": {"name":"Relay", "models": {
            "native-5.1": {"base_model":"lab/model-5.1"},
            "native-51": {"base_model":"lab/model-51"}
        }}}
    }))
    .unwrap();
    for version in ["5.1", "51", "5.10", "51.0"] {
        let id = format!("lab/model-{version}");
        assert_eq!(snapshot.resolve(&id, None).unwrap().id, id);
        assert_eq!(
            snapshot
                .resolve(&format!("Model {version}"), None)
                .unwrap()
                .id,
            id
        );
    }
    assert_eq!(
        snapshot.resolve("relay/native-5.1", None).unwrap().id,
        "lab/model-5.1"
    );
    assert_eq!(
        snapshot.resolve("relay/native-51", None).unwrap().id,
        "lab/model-51"
    );
    assert!(snapshot.resolve("Model 510", None).is_none());
    assert!(snapshot.resolve("Model 5.11", None).is_none());
}

#[test]
fn canonical_history_ids_do_not_follow_floating_provider_aliases() {
    let snapshot = fixture_registry(json!({
        "models": {
            "deepseek/deepseek-v4-flash": {"name":"deepseek-v4-flash"},
            "deepseek/deepseek-v4-flash-0731": {"name":"DeepSeek-V4-Flash-0731"},
            "deepseek/deepseek-v4.1-flash": {"name":"DeepSeek-V4.1-Flash"},
            "deepseek/deepseek-v4-flash-vision-exp": {"name":"DeepSeek V4 Flash Vision Exp"}
        },
        "providers": {
            "deepseek": {"name":"DeepSeek", "models": {
                "deepseek-v4-flash": {"base_model":"deepseek/deepseek-v4.1-flash"},
                "deepseek-v4-flash-vision-exp": {"base_model":"deepseek/deepseek-v4.1-flash"},
                "serving-selector": {"base_model":"deepseek/deepseek-v4.1-flash"}
            }},
            "opencode": {"name":"OpenCode", "models": {
                "deepseek-v4-flash": {"base_model":"deepseek/deepseek-v4-flash-0731"}
            }},
            "openrouter": {"name":"OpenRouter", "models": {
                "deepseek/deepseek-v4-flash": {"base_model":"deepseek/deepseek-v4-flash"}
            }}
        }
    }))
    .unwrap();
    for name in [
        "deepseek-v4-flash",
        "DeepSeek V4 Flash",
        "deepseek/deepseek-v4-flash",
        "deepseek:deepseek-v4-flash",
    ] {
        let model = snapshot.resolve(name, Some("opencode")).unwrap();
        assert_eq!(model.id, "deepseek/deepseek-v4-flash", "{name}");
        assert_eq!(model.display_name, "DeepSeek V4 Flash");
    }
    assert_eq!(
        snapshot
            .resolve_with_provider("deepseek-v4-flash", Some("deepseek"), Some("codex"))
            .unwrap()
            .id,
        "deepseek/deepseek-v4-flash"
    );
    assert_eq!(
        snapshot
            .resolve_with_provider(
                "deepseek/deepseek-v4-flash",
                Some("openrouter"),
                Some("opencode")
            )
            .unwrap()
            .id,
        "deepseek/deepseek-v4-flash"
    );
    assert_eq!(
        snapshot
            .resolve_with_provider("serving-selector", Some("deepseek"), Some("codex"))
            .unwrap()
            .id,
        "deepseek/deepseek-v4.1-flash"
    );
    for suffix in ["v4-flash-0731", "v4.1-flash", "v4-flash-vision-exp"] {
        let id = format!("deepseek/deepseek-{suffix}");
        assert_eq!(snapshot.resolve(&id, None).unwrap().id, id);
    }
}

#[test]
fn recorded_provider_disambiguates_native_selectors_but_source_agent_does_not() {
    let snapshot = fixture_registry(json!({
        "models": {
            "first/shared-model": {"name":"Shared Model"},
            "second/shared-model": {"name":"Shared Model"},
            "lab/model-a": {"name":"Model A"},
            "lab/model-b": {"name":"Model B"}
        },
        "providers": {
            "relay": {"name":"Relay", "models": {
                "shared-model": {"base_model":"second/shared-model"}
            }},
            "relay-a": {"name":"Relay A", "models": {
                "private-slot": {"base_model":"lab/model-a"}
            }},
            "relay-b": {"name":"Relay B", "models": {
                "other-slot": {"base_model":"lab/model-b"}
            }}
        }
    }))
    .unwrap();
    assert!(snapshot.resolve("shared-model", Some("relay")).is_none());
    assert_eq!(
        snapshot
            .resolve_with_provider("shared-model", Some("relay"), Some("opencode"))
            .unwrap()
            .id,
        "second/shared-model"
    );
    assert_eq!(
        snapshot.resolve("private-slot", None).unwrap().id,
        "lab/model-a"
    );
    assert_eq!(
        snapshot
            .resolve_with_provider("private-slot", Some("relay-a"), Some("opencode"))
            .unwrap()
            .id,
        "lab/model-a"
    );
    for raw in [
        "private-slot",
        "private-slot-high",
        "Cursor private-slot High",
        "relay-a/private-slot",
    ] {
        assert!(
            snapshot
                .resolve_with_provider(raw, Some("relay-b"), Some("cursor"))
                .is_none(),
            "{raw}"
        );
    }
    for raw in ["lab/model-a", "model-a", "Model A"] {
        assert_eq!(
            snapshot
                .resolve_with_provider(raw, Some("relay-b"), Some("opencode"))
                .unwrap()
                .id,
            "lab/model-a",
            "{raw}"
        );
    }
}

#[test]
fn provider_ids_are_separate_from_display_names_and_preserve_punctuation() {
    let snapshot = fixture_registry(json!({
        "models": {
            "lab/model-a": {"name":"Model A"},
            "lab/model-b": {"name":"Model B"}
        },
        "providers": {
            "relay-a": {"name":"relay-b", "models": {
                "private-slot": {"base_model":"lab/model-a"}
            }},
            "relay-b": {"name":"Relay B", "models": {
                "other-slot": {"base_model":"lab/model-b"}
            }},
            "relaya": {"name":"Another Relay", "models": {
                "other-slot": {"base_model":"lab/model-b"}
            }},
            "duplicate-a": {"name":"Shared Relay", "models": {
                "private-slot": {"base_model":"lab/model-a"}
            }},
            "duplicate-b": {"name":"Shared Relay", "models": {
                "other-slot": {"base_model":"lab/model-b"}
            }}
        }
    }))
    .unwrap();
    for provider in ["relay-b", "relaya", "Shared Relay", "unknown-relay"] {
        assert!(
            snapshot
                .resolve_with_provider("private-slot", Some(provider), None)
                .is_none(),
            "{provider}"
        );
    }
    assert_eq!(
        snapshot
            .resolve_with_provider("private-slot", Some("relay-a"), None)
            .unwrap()
            .id,
        "lab/model-a"
    );
    assert_eq!(
        snapshot
            .resolve_with_provider("other-slot", Some("Another Relay"), None)
            .unwrap()
            .id,
        "lab/model-b"
    );
}

#[test]
fn historical_models_keep_concrete_versions_without_following_current_routes() {
    let mut value = catalog();
    value["providers"]["relay"] = json!({"name":"Relay", "models": {
        "private-slot": {"name":"Kimi K3", "base_model":"moonshotai/kimi-k3"},
        "slot3": {"name":"Kimi K3", "base_model":"moonshotai/kimi-k3"},
        "default": {"base_model":"moonshotai/kimi-k3"}
    }});
    let before = fixture_registry(value.clone()).unwrap();
    value["providers"]["relay"]["models"]["private-slot"]["base_model"] =
        json!("moonshotai/kimi-k2.7-code");
    value["providers"]["kimi-for-coding"]["models"]["kimi-for-coding"]["base_model"] =
        json!("moonshotai/kimi-k3");
    let after = fixture_registry(value).unwrap();
    assert_ne!(
        before
            .resolve_with_provider("private-slot", Some("relay"), None)
            .unwrap()
            .id,
        after
            .resolve_with_provider("private-slot", Some("relay"), None)
            .unwrap()
            .id
    );
    for snapshot in [&before, &after] {
        for raw in [
            "k3",
            "kimi-k3",
            "kimi-code-k3",
            "kimi-code-k3-256k",
            "Kimi Code K3.256k",
            "Cursor Kimi K3 Extra High Fast",
            "opencode-go/kimi-k3",
        ] {
            assert_eq!(
                snapshot
                    .resolve_historical(raw, Some("kimi-for-coding"), Some("cursor"))
                    .unwrap()
                    .id,
                "moonshotai/kimi-k3",
                "{raw}"
            );
        }
        for (raw, provider) in [
            ("private-slot", "relay"),
            ("slot3", "relay"),
            ("default", "relay"),
            ("kimi-for-coding", "kimi-for-coding"),
            ("kimi-for-coding/kimi-for-coding", "kimi-for-coding"),
        ] {
            assert!(
                snapshot
                    .resolve_historical(raw, Some(provider), None)
                    .is_none(),
                "{raw}"
            );
        }
    }
    let mismatched_versions = fixture_registry(json!({
        "models": {
            "lab/model-5.1-0731-vision": {"name":"Model 5.1 0731 Vision"},
            "lab/other-3": {"name":"Other 3"}
        },
        "providers": {"relay": {"name":"Relay", "models": {
            "model-5.1": {"base_model":"lab/model-5.1-0731-vision"},
            "1-0731-vision": {"base_model":"lab/model-5.1-0731-vision"},
            "slot3": {"name":"Other 3", "base_model":"lab/other-3"}
        }}}
    }))
    .unwrap();
    for raw in ["model-5.1", "1-0731-vision", "slot3"] {
        assert!(
            mismatched_versions
                .resolve_historical(raw, Some("relay"), None)
                .is_none(),
            "{raw}"
        );
    }
}

#[test]
fn cursor_thinking_and_effort_wrappers_preserve_the_actual_model_version() {
    let snapshot = fixture_registry(json!({
        "models": {
            "anthropic/claude-fable-5": {"name":"Claude Fable 5"},
            "anthropic/claude-fable-5-1": {"name":"claude-fable-5-1"},
            "example/separate-thinking": {"name":"Separate Thinking"},
            "example/separate": {"name":"Separate"}
        },
        "providers": {}
    }))
    .unwrap();
    for suffix in ["max", "high"] {
        let model = snapshot
            .resolve(
                &format!("claude-fable-5-1-thinking-{suffix}"),
                Some("cursor"),
            )
            .unwrap();
        assert_eq!(model.id, "anthropic/claude-fable-5-1");
        assert_eq!(model.display_name, "Claude Fable 5.1");
    }
    assert_eq!(
        snapshot
            .resolve("claude-fable-5-thinking-max", Some("cursor"))
            .unwrap()
            .id,
        "anthropic/claude-fable-5"
    );
    assert_eq!(
        snapshot
            .resolve("separate-thinking", Some("cursor"))
            .unwrap()
            .id,
        "example/separate-thinking"
    );
    assert!(
        snapshot
            .resolve("claude-fable-5-1-thinking-max", Some("other-agent"))
            .is_none()
    );
    assert!(
        snapshot
            .resolve("claude-fable-5-2-thinking-max", Some("cursor"))
            .is_none()
    );
}

#[test]
fn model_display_names_format_catalog_and_unknown_ids_without_aliasing() {
    for (raw, expected) in [
        ("deepseek-v4-flash", "DeepSeek V4 Flash"),
        ("deepseek/deepseek-v4-pro", "DeepSeek V4 Pro"),
        ("deepseek:deepseek-v4-flash", "DeepSeek V4 Flash"),
        ("DeepSeek-V4-Flash-0731", "DeepSeek V4 Flash 0731"),
        (
            "deepseek-v4-flash-vision-exp",
            "DeepSeek V4 Flash Vision Exp",
        ),
        ("anthropic/claude-fable-5-1", "Claude Fable 5.1"),
        ("openai/gpt-6-astra", "GPT-6 Astra"),
        ("GPT-5.6 Sol", "GPT-5.6 Sol"),
        ("OpenAI GPT-6 Astra", "OpenAI GPT-6 Astra"),
        ("MiniMax M2.5", "MiniMax M2.5"),
        ("minimax-m2.5", "MiniMax M2.5"),
        ("ChatGPT Latest", "ChatGPT Latest"),
        ("chatgpt-latest", "ChatGPT Latest"),
        ("QwQ 32B", "QwQ 32B"),
        ("CodeGeeX 4", "CodeGeeX 4"),
        ("LFM2.5 1.2B", "LFM2.5 1.2B"),
        ("gemini-3.5-flash (Medium)", "Gemini 3.5 Flash (Medium)"),
        ("custom/gpt-reserve", "GPT Reserve"),
        ("custom/grok-bot", "Grok Bot"),
        ("custom/grok-bot-default", "Grok Bot"),
        ("default", "Default"),
        ("vendor/my_private-model", "My Private Model"),
        ("unknown/model-5.10", "Model 5.10"),
        ("unknown/model-51.0", "Model 51.0"),
    ] {
        assert_eq!(model_display_name(raw), expected, "{raw}");
    }
    let snapshot = fixture_registry(catalog()).unwrap();
    assert!(snapshot.resolve("custom/gpt-reserve", None).is_none());
    assert!(snapshot.resolve("custom/grok-bot", None).is_none());
    assert!(snapshot.resolve("custom/grok-bot-default", None).is_none());
    let branded = fixture_registry(json!({
        "models": {
            "minimax/minimax-m2.5": {"name":"MiniMax M2.5"},
            "openai/chatgpt-latest": {"name":"ChatGPT Latest"},
            "qwen/qwq-32b": {"name":"QwQ 32B"}
        },
        "providers": {}
    }))
    .unwrap();
    for (id, expected) in [
        ("minimax/minimax-m2.5", "MiniMax M2.5"),
        ("openai/chatgpt-latest", "ChatGPT Latest"),
        ("qwen/qwq-32b", "QwQ 32B"),
    ] {
        assert_eq!(branded.resolve(id, None).unwrap().display_name, expected);
    }
}

#[test]
fn unique_public_names_link_missing_base_references_without_model_guessing() {
    let mut value = catalog();
    value["providers"]["relay"] = json!({"name":"Relay", "models": {
        "vendor-k3": {"name":"Kimi K3"},
        "novel-7": {"name":"Novel 7"}
    }});
    value["providers"]["different"] = json!({"name":"Different", "models": {
        "novel-7": {"name":"Novel 7"}
    }});
    let snapshot = fixture_registry(value).unwrap();
    assert_eq!(
        snapshot.resolve("relay/vendor-k3", None).unwrap().id,
        "moonshotai/kimi-k3"
    );
    assert_eq!(
        snapshot.resolve("relay/novel-7", None).unwrap().id,
        "relay/novel-7"
    );
    assert_eq!(
        snapshot.resolve("different/novel-7", None).unwrap().id,
        "different/novel-7"
    );
    assert!(snapshot.resolve("Novel 7", None).is_none());
}

#[test]
fn cache_refresh_is_atomic_and_other_process_observes_new_revision() {
    let directory = TestDirectory::new();
    let path = directory.0.join("model-registry/catalog.json");
    let writer = RegistryRuntime::default();
    let reader = RegistryRuntime::default();
    assert!(writer.refresh_at(&path, || downloaded(catalog())).unwrap());
    reader.reload_at(&path).unwrap();
    let old = reader.snapshot();
    assert_eq!(writer.snapshot().revision(), old.revision());
    assert_eq!(reader.summary(true, "ready", None)["skippedEntries"], 2);
    assert!(!writer.refresh_at(&path, || downloaded(catalog())).unwrap());
    let mut next = catalog();
    next["models"]["moonshotai/kimi-k4"] = json!({"name":"Kimi K4"});
    writer.refresh_at(&path, || downloaded(next)).unwrap();
    reader.reload_at(&path).unwrap();
    assert_ne!(reader.snapshot().revision(), old.revision());
    assert!(old.resolve("kimi-k4", None).is_none());
    assert_eq!(
        reader.snapshot().resolve("kimi-k4", None).unwrap().id,
        "moonshotai/kimi-k4"
    );
}

#[test]
fn failed_refresh_and_invalid_cache_keep_last_valid_snapshot() {
    let directory = TestDirectory::new();
    let path = directory.0.join("catalog.json");
    let runtime = RegistryRuntime::default();
    runtime.refresh_at(&path, || downloaded(catalog())).unwrap();
    let old = runtime.snapshot();
    let bytes = fs::read(&path).unwrap();
    assert!(
        runtime
            .refresh_at(&path, || Err(anyhow!("offline")))
            .is_err()
    );
    assert!(
        runtime
            .refresh_at(&path, || downloaded(json!({"models":{},"providers":{}})))
            .is_err()
    );
    assert_eq!(fs::read(&path).unwrap(), bytes);
    fs::write(&path, "not a catalog").unwrap();
    assert!(runtime.reload_at(&path).is_err());
    assert_eq!(runtime.snapshot().revision(), old.revision());
}

#[test]
fn cache_write_failure_does_not_publish_an_unpersisted_catalog() {
    let directory = TestDirectory::new();
    let obstruction = directory.0.join("file");
    fs::write(&obstruction, "fixture").unwrap();
    let runtime = RegistryRuntime::default();
    assert!(
        runtime
            .refresh_at(&obstruction.join("catalog.json"), || downloaded(catalog()))
            .is_err()
    );
    assert_eq!(runtime.summary(true, "ready", None)["status"], "empty");
}

#[test]
fn test_snapshot_is_scoped_and_never_reads_host_catalog() {
    assert!(snapshot().revision().is_empty());
    let result = std::panic::catch_unwind(|| {
        with_test_snapshot(fixture_registry(catalog()).unwrap(), || {
            assert!(!refresh_cached_snapshot().revision().is_empty());
            panic!("fixture panic");
        })
    });
    assert!(result.is_err());
    assert!(snapshot().revision().is_empty());
}

fn archive(entries: &[(&str, &str)], links: &[(&str, &str)]) -> Vec<u8> {
    let mut tar = tar::Builder::new(Vec::new());
    for (path, text) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_size(text.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append_data(&mut header, format!("catalog/{path}"), text.as_bytes())
            .unwrap();
    }
    for (path, link) in links {
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_size(0);
        header.set_mode(0o644);
        tar.append_link(&mut header, format!("catalog/{path}"), link)
            .unwrap();
    }
    let bytes = tar.into_inner().unwrap();
    let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gzip.write_all(&bytes).unwrap();
    gzip.finish().unwrap()
}

#[test]
fn public_archive_preserves_explicit_links_and_reports_dangling_entries() {
    let bytes = archive(
        &[
            ("models/lab/model-3.toml", "name = 'Model 3'"),
            ("providers/relay/provider.toml", "name = 'Relay'"),
            (
                "providers/relay/models/native.toml",
                "name = 'Native Model'\nbase_model = 'lab/model-3'",
            ),
        ],
        &[
            ("providers/relay/models/alias.toml", "native.toml"),
            ("providers/relay/models/dangling.toml", "missing.toml"),
            ("providers/relay/models/cycle.toml", "cycle.toml"),
        ],
    );
    let downloaded = source::from_archive(&bytes).unwrap();
    assert_eq!(downloaded.source, source::REPOSITORY_SOURCE);
    assert_eq!(downloaded.skipped_entries, 2);
    let snapshot = fixture_document(&downloaded.catalog).unwrap();
    assert_eq!(
        snapshot.resolve("relay/alias", None).unwrap().id,
        "lab/model-3"
    );
    assert!(snapshot.resolve("dangling", None).is_none());
}

#[test]
fn malformed_public_model_rejects_the_entire_refresh() {
    let bytes = archive(
        &[
            ("models/lab/model-3.toml", "name = 'Model 3'"),
            ("providers/relay/provider.toml", "name = 'Relay'"),
            ("providers/relay/models/native.toml", "name = ["),
        ],
        &[],
    );
    assert!(source::from_archive(&bytes).is_err());
}
