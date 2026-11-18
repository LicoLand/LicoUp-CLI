//! The execution contract this crate publishes.
//!
//! The graph driver, the node facade and the capability adapters are the
//! engine's. `licoup-workflow-runtime` owns the LicoUp service, ports and
//! evolution seams layered over them; it must not grow a second scheduling
//! shell. These checks unify the published paths with the engine's own items.

use licoup_workflow_runtime::{adapter, driver, node};

/// Unify two spellings of one type. Rust has no structural typing, so the
/// call compiles only while both paths name the very same item.
fn one_type<T>(value: T) -> T {
    value
}

#[test]
fn the_graph_driver_is_the_engine_driver() {
    let _: Option<acativa::runtime::driver::ContinuousNodeDriver> =
        one_type(None::<driver::ContinuousNodeDriver>);
    let _: Option<acativa::runtime::driver::DriverStepOutcome> =
        one_type(None::<driver::DriverStepOutcome>);
    let _: Option<acativa::runtime::driver::GraphPauseReport> =
        one_type(None::<driver::GraphPauseReport>);
    let _: Option<acativa::runtime::driver::PerNodePauseOutcome> =
        one_type(None::<driver::PerNodePauseOutcome>);
}

#[test]
fn the_node_facade_and_its_receipts_are_the_engine_own_types() {
    let _: Option<acativa::runtime::node::NodeFacade> = one_type(None::<node::NodeFacade>);
    let _: Option<acativa::runtime::node::NodeExecutionReceipt> =
        one_type(None::<node::NodeExecutionReceipt>);
    let _: Option<acativa::runtime::node::NodeObservation> =
        one_type(None::<node::NodeObservation>);
    let _: Option<acativa::runtime::node::SteerResult> = one_type(None::<node::SteerResult>);
}

#[test]
fn the_capability_adapters_are_the_engine_own_types() {
    let _: Option<acativa::runtime::adapter::NodeInvocation> =
        one_type(None::<adapter::NodeInvocation>);
    let _: Option<acativa::runtime::adapter::AdapterError> =
        one_type(None::<adapter::AdapterError>);
    let _: Option<acativa::runtime::adapter::AdapterExecutionStatus> =
        one_type(None::<adapter::AdapterExecutionStatus>);
    let _: Option<acativa::runtime::adapter::CooperativeDrainAdapter> =
        one_type(None::<adapter::CooperativeDrainAdapter>);
    let _: Option<acativa::runtime::adapter::SingleWriterSessionRegistry> =
        one_type(None::<adapter::SingleWriterSessionRegistry>);
    let _: Option<acativa::runtime::adapter::SyntheticCapabilityAdapter> =
        one_type(None::<adapter::SyntheticCapabilityAdapter>);
}

#[test]
fn the_turn_service_vocabulary_is_the_engine_own_types() {
    let _: Option<acativa::runtime::service::ActorTurnPort> =
        one_type(None::<licoup_workflow_runtime::ActorTurnPort>);
    let _: Option<acativa::runtime::service::TurnCancelDisposition> =
        one_type(None::<licoup_workflow_runtime::TurnCancelDisposition>);
    let _: Option<acativa::runtime::service::RunWakePort> =
        one_type(None::<licoup_workflow_runtime::AssistantWakePort>);
}
