use super::super::model_catalog::project_model_list_response;
use super::super::{io::TransportEvent, model_catalog::wait_for_catalog_responses};
use crate::app_server::reserve::reserve_model_available;
use serde_json::json;
use std::sync::mpsc;
use std::time::{Duration, Instant};

#[test]
fn projects_visible_native_models_without_collapsing_duplicate_labels() {
    let result = project_model_list_response(&json!({
        "data": [
            {"model":"first","displayName":"Same","hidden":false,"isDefault":true,
             "defaultReasoningEffort":"medium",
             "supportedReasoningEfforts":[{"reasoningEffort":"low"},{"reasoningEffort":"medium"}]},
            {"model":"second","displayName":"Same","hidden":false,"isDefault":false,
             "supportedReasoningEfforts":[{"reasoningEffort":"high"}]},
            {"model":"hidden","displayName":"Hidden","hidden":true}
        ]
    }), false)
    .unwrap();
    assert_eq!(result["defaultModel"], "first");
    assert_eq!(result["models"].as_array().unwrap().len(), 2);
    assert_eq!(result["models"][0]["displayName"], "Same");
    assert_eq!(result["models"][0]["defaultReasoningEffort"], "medium");
    assert_eq!(result["models"][1]["displayName"], "Same");
    assert!(result["models"][1].get("defaultReasoningEffort").is_none());
}

#[test]
fn projects_gpt_reserve_only_for_an_authorized_exhausted_account() {
    let response = json!({
        "data": [
            {"model":"gpt-6-luna","displayName":"GPT-6 Luna","hidden":false},
            {"model":"gpt-reserve","displayName":"GPT Reserve","hidden":false,
             "supportedReasoningEfforts":[{"reasoningEffort":"low"}]}
        ]
    });

    let ordinary = project_model_list_response(&response, false).unwrap();
    assert!(
        !ordinary["models"]
            .as_array()
            .unwrap()
            .iter()
            .any(|model| model["name"] == "gpt-reserve")
    );

    let exhausted = project_model_list_response(&response, true).unwrap();
    let reserve = exhausted["models"]
        .as_array()
        .unwrap()
        .iter()
        .find(|model| model["name"] == "gpt-reserve")
        .unwrap();
    assert_eq!(reserve["displayName"], "GPT Reserve");
    assert_eq!(reserve["reasoningEfforts"], json!(["low"]));
    assert_eq!(reserve["isDefault"], false);
    assert_eq!(reserve["ephemeral"], true);
}

#[test]
fn reserve_visibility_requires_the_backend_capability_and_bucket() {
    let eligible = json!({
        "ordinaryUsageAllowed": false,
        "rateLimitsByLimitId": {
            "base_model_inference": {"limitName":"gpt-reserve"}
        },
        "rateLimitUpsell": {"banner_type":"luna_reserve"}
    });
    assert!(reserve_model_available(&eligible));

    let ordinary = json!({
        "ordinaryUsageAllowed": true,
        "rateLimitsByLimitId": {
            "base_model_inference": {"limitName":"gpt-reserve"}
        },
        "rateLimitUpsell": {"banner_type":"luna_reserve"}
    });
    assert!(!reserve_model_available(&ordinary));

    let missing_banner = json!({
        "ordinaryUsageAllowed": false,
        "rateLimitsByLimitId": {
            "base_model_inference": {"limitName":"gpt-reserve"}
        }
    });
    assert!(!reserve_model_available(&missing_banner));

    let missing_bucket = json!({
        "ordinaryUsageAllowed": false,
        "rateLimitUpsell": {"banner_type":"luna_reserve"}
    });
    assert!(!reserve_model_available(&missing_bucket));
}

#[test]
fn catalog_waits_for_rate_limits_when_model_list_arrives_first() {
    let (sender, receiver) = mpsc::channel();
    sender
        .send(TransportEvent::Line(
            br#"{"id":91002,"result":{"data":[{"model":"gpt-6-luna"}]}}"#.to_vec(),
        ))
        .unwrap();
    sender
        .send(TransportEvent::Line(
            br#"{"id":91003,"result":{"ordinaryUsageAllowed":false,"rateLimitUpsell":{"banner_type":"luna_reserve"},"rateLimitsByLimitId":{"base_model_inference":{"limitName":"gpt-reserve"}}}}"#.to_vec(),
        ))
        .unwrap();

    let (model_list, reserve_available) =
        wait_for_catalog_responses(&receiver, Instant::now() + Duration::from_secs(1)).unwrap();
    assert_eq!(model_list["data"][0]["model"], "gpt-6-luna");
    assert!(reserve_available);
}

#[test]
fn malformed_model_list_response_is_rejected_without_a_reserve_row() {
    assert!(project_model_list_response(&json!({"data": "invalid"}), true).is_err());
}
