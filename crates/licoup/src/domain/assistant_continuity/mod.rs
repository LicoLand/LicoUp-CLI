//! M1 cognition and context leaves plus the M2 host that attaches them.

pub mod adoption;
pub mod cognition;
pub mod collection;
pub mod context;
pub mod execution;
pub mod host;
pub mod live;
pub mod observation;

pub use adoption::{AdoptionPolicy, AdoptionStage};
pub use cognition::{UnavailableInterpretationService, interpretation_port};
pub use context::{UnavailableContextCompositionService, context_composition_port};
pub use host::{
    ChildControlDisposition, ChildWorkFault, ContinuityHost, set_child_assembly_recheck_failures,
    set_child_work_fault,
};
pub use observation::{
    ObservationFact, ObservationFactKind, ObservationIndex, ObservationPage, ObservationQuery,
    ObservationRecordOutcome, ObservationScope, ObservationState, ObservationStore,
    list_observations, load_observation, load_observation_index, record_observation,
};
