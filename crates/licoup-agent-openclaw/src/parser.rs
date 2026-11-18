pub mod codec;
pub mod events;
pub mod protocol;

use licoup_agent_adapter_sdk::adapters::AdapterContract;
use licoup_agent_adapter_sdk::{LifecycleStage, Transition, TransitionReducer};
pub const CONTRACT: AdapterContract = AdapterContract::new("openclaw", "gateway-jsonrpc-acp");

pub fn completed_transitions(output: &str) -> Vec<Transition> {
    let mut reducer = TransitionReducer::default();
    let mut transitions = reducer.advance(LifecycleStage::Accepted);
    transitions.extend(reducer.advance(LifecycleStage::Processing));
    transitions.extend(reducer.advance(LifecycleStage::Responding));
    transitions.push(Transition::Text {
        unit_id: "openclaw:reply".to_owned(),
        text: output.to_owned(),
    });
    transitions.extend(reducer.advance(LifecycleStage::Completed));
    transitions
}

pub fn failed_transitions(code: &str, stage: &str, message: &str) -> Vec<Transition> {
    let mut reducer = TransitionReducer::default();
    let mut transitions = reducer.advance(LifecycleStage::Accepted);
    if let Some(failure) = reducer.fail(code, stage, message) {
        transitions.push(failure);
    }
    transitions
}
