use serde_json::{Value, json};

pub use crate::state_machines::parser_lifecycle::State as LifecycleStage;
use crate::state_machines::parser_lifecycle::{self, Event};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Transition {
    Lifecycle(LifecycleStage),
    Text {
        unit_id: String,
        text: String,
    },
    #[allow(dead_code)]
    Control {
        method: String,
        summary: String,
    },
    Failed {
        code: String,
        stage: String,
        message: String,
    },
}

impl Transition {
    pub fn to_json(&self) -> Value {
        match self {
            Self::Lifecycle(stage) => json!({
                "kind": "lifecycle",
                "stage": stage.as_str(),
            }),
            Self::Text { unit_id, text } => json!({
                "kind": "text",
                "unitId": unit_id,
                "text": text,
            }),
            Self::Control { method, summary } => json!({
                "kind": "control",
                "method": method,
                "summary": summary,
            }),
            Self::Failed {
                code,
                stage,
                message,
            } => json!({
                "kind": "failed",
                "code": code,
                "stage": stage,
                "message": message,
            }),
        }
    }
}

/// Arrival-ordered lifecycle and terminal reduction. Stages are prefix closed;
/// the first exact native failure is write-once.
#[derive(Default)]
pub struct TransitionReducer {
    highest: Option<LifecycleStage>,
    failure: Option<Transition>,
}

impl TransitionReducer {
    pub fn advance(&mut self, stage: LifecycleStage) -> Vec<Transition> {
        if self.failure.is_some() || self.highest.is_some_and(|current| current >= stage) {
            return Vec::new();
        }

        let mut emitted = Vec::new();
        let mut current = match self.highest {
            Some(current) => current,
            None => {
                let initial = parser_lifecycle::INITIAL;
                emitted.push(Transition::Lifecycle(initial));
                initial
            }
        };
        while current != stage {
            let next = parser_lifecycle::transition(current, Event::Advance)
                .expect("parser lifecycle must reach every later configured stage");
            current = next;
            emitted.push(Transition::Lifecycle(current));
        }
        self.highest = Some(current);
        emitted
    }

    pub fn fail(
        &mut self,
        code: impl Into<String>,
        stage: impl Into<String>,
        message: impl Into<String>,
    ) -> Option<Transition> {
        if self.failure.is_some() || self.highest.is_some_and(parser_lifecycle::terminal) {
            return None;
        }
        let failure = Transition::Failed {
            code: code.into(),
            stage: stage.into(),
            message: message.into(),
        };
        self.failure = Some(failure.clone());
        Some(failure)
    }
}
