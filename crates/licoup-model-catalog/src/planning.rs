//! The planning inputs the catalogue owns.
//!
//! A planner compares `Agent + Model + Thinking` options. The facts it reads
//! from the catalogue are exactly two: the declared canonical identity of the
//! model a selector names, and the price facts recorded for that route. The
//! ranking, the comparison contract, the qualification policy and the learned
//! defaults are planning behaviour and stay with the planner; this module owns
//! only the inputs, so a planner never reaches into the pricing tables
//! themselves.
//!
//! Every input is optional and absence is reported as absence. A model with no
//! recorded route has no planning price; it is not priced at zero, and no price
//! is inferred from a similar name.

pub use crate::pricing::{
    ModelTokenPrice, PlanningModelPrice, agent_model_planning_price, agent_model_price,
    model_planning_price, model_price,
};

/// The recorded price facts for one model, as a planning option consumes them.
///
/// `None` is unknown, never free: a route the catalogue does not record has no
/// price and must not be ranked as the cheapest option.
pub fn planning_model_price(model_id: &str) -> Option<PlanningModelPrice> {
    model_planning_price(model_id)
}

/// The effective Agent route price for one model and Thinking setting.
///
/// An Agent route is only priced when the catalogue records one for the exact
/// Agent, model and thinking triple, and an empty thinking setting names no
/// route.
pub fn planning_agent_model_price(
    agent_id: &str,
    model_id: &str,
    thinking: &str,
) -> Option<PlanningModelPrice> {
    agent_model_planning_price(agent_id, model_id, thinking)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planning_reads_the_recorded_route_and_reports_absence_as_absence() {
        let raw = planning_model_price("gpt-5-6-sol").expect("the recorded route");
        assert_eq!(raw, model_planning_price("gpt-5.6-sol").unwrap());
        assert_eq!(raw.unit, "usd_per_million_tokens");
        assert!(raw.input.is_finite() && raw.input >= 0.0);
        assert!(raw.output.is_finite() && raw.output >= 0.0);

        let codex = planning_agent_model_price("codex", "gpt-5-6-sol", "max")
            .expect("the recorded Agent route");
        assert_eq!(codex.unit, "credits_per_million_tokens");

        assert_eq!(planning_model_price("not-a-recorded-model"), None);
        assert_eq!(
            planning_agent_model_price("codex", "gpt-5-6-sol", ""),
            None,
            "an empty thinking setting names no route"
        );
    }

    #[test]
    fn a_model_token_price_is_reported_only_for_a_recorded_route() {
        let price = model_price("deepseek-v4-flash").expect("the recorded route");
        assert!(price.input >= 0.0 && price.output >= 0.0);
        assert_eq!(model_price("not-a-recorded-model"), None);
    }
}
