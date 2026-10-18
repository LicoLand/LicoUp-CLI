//! Goal lifecycle used by the production store commit path.
//!
//! Transitions follow `blueprints/02-STATE-AND-RECOVERY.md`. Tests drive this
//! table through `ConversationStore`, not a copied in-test reducer.

use super::error::continuity_failure;
use super::generated::{
    ContinuityFailure, ContinuityFailureCode, ContinuityFailureStage, ContinuityGoalControl,
    ContinuityGoalLifecycle,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContinuityGoalEvent {
    AcceptCommitment,
    NamedWait,
    RelevantWake,
    CandidateCompletion,
    CriteriaFailed,
    CriteriaSatisfied,
    Pause,
    Resume,
    CancelRequest,
    CancelSettled,
    Supersede,
    LateEvidence,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContinuityGoalState {
    pub lifecycle: ContinuityGoalLifecycle,
    pub control: ContinuityGoalControl,
}

pub fn is_terminal(lifecycle: ContinuityGoalLifecycle) -> bool {
    matches!(
        lifecycle,
        ContinuityGoalLifecycle::Achieved
            | ContinuityGoalLifecycle::Cancelled
            | ContinuityGoalLifecycle::Superseded
    )
}

pub fn apply_goal_event(
    current: ContinuityGoalState,
    event: ContinuityGoalEvent,
) -> Result<ContinuityGoalState, ContinuityFailure> {
    if is_terminal(current.lifecycle) {
        return match event {
            ContinuityGoalEvent::LateEvidence => Ok(current),
            _ => Err(continuity_failure(
                ContinuityFailureCode::InvalidRequest,
                ContinuityFailureStage::ContinuityAdmission,
            )),
        };
    }
    match event {
        ContinuityGoalEvent::AcceptCommitment => Ok(ContinuityGoalState {
            lifecycle: ContinuityGoalLifecycle::Active,
            control: ContinuityGoalControl::Enabled,
        }),
        ContinuityGoalEvent::NamedWait => Ok(ContinuityGoalState {
            lifecycle: ContinuityGoalLifecycle::Waiting,
            control: current.control,
        }),
        ContinuityGoalEvent::RelevantWake => Ok(ContinuityGoalState {
            lifecycle: ContinuityGoalLifecycle::Active,
            control: current.control,
        }),
        ContinuityGoalEvent::CandidateCompletion => Ok(ContinuityGoalState {
            lifecycle: ContinuityGoalLifecycle::Verifying,
            control: current.control,
        }),
        ContinuityGoalEvent::CriteriaFailed => Ok(ContinuityGoalState {
            lifecycle: ContinuityGoalLifecycle::Active,
            control: current.control,
        }),
        ContinuityGoalEvent::CriteriaSatisfied => Ok(ContinuityGoalState {
            lifecycle: ContinuityGoalLifecycle::Achieved,
            control: ContinuityGoalControl::Enabled,
        }),
        ContinuityGoalEvent::Pause => {
            if current.control == ContinuityGoalControl::CancelRequested {
                return Err(continuity_failure(
                    ContinuityFailureCode::InvalidRequest,
                    ContinuityFailureStage::ContinuityAdmission,
                ));
            }
            Ok(ContinuityGoalState {
                lifecycle: current.lifecycle,
                control: ContinuityGoalControl::Paused,
            })
        }
        ContinuityGoalEvent::Resume => {
            if current.control != ContinuityGoalControl::Paused {
                return Err(continuity_failure(
                    ContinuityFailureCode::InvalidRequest,
                    ContinuityFailureStage::ContinuityAdmission,
                ));
            }
            Ok(ContinuityGoalState {
                lifecycle: current.lifecycle,
                control: ContinuityGoalControl::Enabled,
            })
        }
        ContinuityGoalEvent::CancelRequest => Ok(ContinuityGoalState {
            lifecycle: current.lifecycle,
            control: ContinuityGoalControl::CancelRequested,
        }),
        ContinuityGoalEvent::CancelSettled => {
            if current.control != ContinuityGoalControl::CancelRequested {
                return Err(continuity_failure(
                    ContinuityFailureCode::InvalidRequest,
                    ContinuityFailureStage::ContinuityAdmission,
                ));
            }
            Ok(ContinuityGoalState {
                lifecycle: ContinuityGoalLifecycle::Cancelled,
                control: ContinuityGoalControl::Enabled,
            })
        }
        ContinuityGoalEvent::Supersede => Ok(ContinuityGoalState {
            lifecycle: ContinuityGoalLifecycle::Superseded,
            control: ContinuityGoalControl::Enabled,
        }),
        ContinuityGoalEvent::LateEvidence => Ok(current),
    }
}

pub fn suppresses_new_work(control: ContinuityGoalControl) -> bool {
    matches!(
        control,
        ContinuityGoalControl::Paused | ContinuityGoalControl::CancelRequested
    )
}
