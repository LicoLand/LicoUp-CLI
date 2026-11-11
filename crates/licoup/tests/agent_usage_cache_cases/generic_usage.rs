use super::support::*;

#[test]
fn generic_usage_extractor_keeps_cached_input_as_a_subset() {
    let history_root = temp_dir("generic-usage-cached-subset");
    fs::write(
        history_root.join("session.json"),
        json!({
            "id": "cached-session",
            "messages": [
                {
                    "role": "user",
                    "content": "question"
                },
                {
                    "role": "assistant",
                    "content": "done",
                    "usage": {
                        "input_tokens": 100,
                        "cached_input_tokens": 40,
                        "output_tokens": 10
                    }
                }
            ]
        })
        .to_string(),
    )
    .unwrap();

    let listed = conversations::conversation_list(&json!({
        "agent": "opencode",
        "root": history_root.to_string_lossy(),
        "limit": 10
    }))
    .unwrap();
    let usage = find_explicit_usage(&listed).expect("explicit usage projection");
    assert_eq!(usage["promptTokens"], 100);
    assert_eq!(usage["cachedInputTokens"], 40);
    assert_eq!(usage["completionTokens"], 10);
    assert_eq!(usage["totalTokens"], 110);
}

#[test]
fn generic_usage_extractor_projects_parent_usage_once_for_content_blocks() {
    let history_root = temp_dir("generic-usage-content-blocks");
    fs::write(
        history_root.join("session.json"),
        json!({
            "id": "content-block-session",
            "messages": [
                {"role": "user", "content": "question"},
                {
                    "role": "assistant",
                    "content": [
                        {"type": "output_text", "text": "first block"},
                        {"type": "output_text", "text": "second block"}
                    ],
                    "usage": {
                        "input_tokens": 100,
                        "cached_input_tokens": 40,
                        "output_tokens": 10
                    }
                }
            ]
        })
        .to_string(),
    )
    .unwrap();

    let listed = conversations::conversation_list(&json!({
        "agent": "opencode",
        "root": history_root.to_string_lossy(),
        "limit": 10
    }))
    .unwrap();
    let usages = explicit_usages(&listed);
    assert_eq!(usages.len(), 1, "parent usage must be projected once");
    assert_eq!(usages[0]["promptTokens"], 100);
    assert_eq!(usages[0]["cachedInputTokens"], 40);
    assert_eq!(usages[0]["completionTokens"], 10);
    assert_eq!(
        usages
            .iter()
            .filter_map(|usage| usage["totalTokens"].as_u64())
            .sum::<u64>(),
        110
    );
}

#[test]
fn generic_usage_extractor_handles_normalized_opencode_cache_and_reasoning() {
    let history_root = temp_dir("generic-usage-opencode-normalized");
    fs::write(
        history_root.join("session.json"),
        json!({
            "id": "normalized-session",
            "messages": [
                {"role": "user", "content": "question"},
                {
                    "role": "assistant",
                    "content": "done",
                    "usage": {
                        "tokens": {
                            "input": 60,
                            "output": 5,
                            "reasoning": 2,
                            "cache": {"read": 30, "write": 10}
                        }
                    }
                }
            ]
        })
        .to_string(),
    )
    .unwrap();

    let listed = conversations::conversation_list(&json!({
        "agent": "opencode",
        "root": history_root.to_string_lossy(),
        "limit": 10
    }))
    .unwrap();
    let usage = find_explicit_usage(&listed).expect("normalized usage projection");
    assert_eq!(usage["promptTokens"], 100);
    assert_eq!(usage["cachedInputTokens"], 30);
    assert_eq!(usage["completionTokens"], 7);
    assert_eq!(usage["totalTokens"], 107);
}
