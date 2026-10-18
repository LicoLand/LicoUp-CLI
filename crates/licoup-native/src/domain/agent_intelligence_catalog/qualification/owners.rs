//! Read current facts from existing catalog owners. This module does not store
//! a second model, price, Skill, or route table.

use crate::domain::agent_intelligence_catalog;
use crate::domain::provider_model_pricing::{self, ModelTokenPrice};
use serde_json::Value;

/// Current token price from `provider_model_pricing`. Missing is unknown, not zero.
pub fn model_token_price(model_id: &str) -> Option<ModelTokenPrice> {
    provider_model_pricing::model_price(model_id)
}

/// Agent-route token price from the same pricing owner.
pub fn agent_model_token_price(
    agent_id: &str,
    model_id: &str,
    thinking: &str,
) -> Option<ModelTokenPrice> {
    provider_model_pricing::agent_model_price(agent_id, model_id, thinking)
}

/// Current catalog projection. Presence is a fact, never a brand eligibility list.
pub fn catalog_model_projection(model_id: &str) -> Option<Value> {
    agent_intelligence_catalog::project_allowlisted_model(model_id)
}

/// Read-only target inspection from the existing `targets` owner.
pub fn target_read_only(target_id: &str) -> Result<Value, anyhow::Error> {
    crate::domain::targets::inspect_target_read_only(
        &crate::target_port::agent_target_port(),
        target_id,
    )
}

/// Skill list from the existing `skill_hub` owner. Callers supply params; this
/// leaf does not copy the Skill catalog.
pub fn skill_hub_list(params: &Value) -> Result<Value, anyhow::Error> {
    crate::domain::skill_hub::skill_list(params)
}

/// Convert token counts through the current owner price. Unknown price stays
/// unknown; it is never inferred as free or cheaper.
pub fn token_cost_from_owner(
    model_id: Option<&str>,
    agent_id: Option<&str>,
    thinking: Option<&str>,
    input_tokens: u64,
    output_tokens: u64,
) -> Option<f64> {
    let price = match (model_id, agent_id, thinking) {
        (Some(model), Some(agent), Some(effort)) if !agent.is_empty() && !effort.is_empty() => {
            agent_model_token_price(agent, model, effort).or_else(|| model_token_price(model))
        }
        (Some(model), _, _) => model_token_price(model),
        _ => None,
    }?;
    Some(
        (input_tokens as f64) * price.input / 1_000_000.0
            + (output_tokens as f64) * price.output / 1_000_000.0,
    )
}
