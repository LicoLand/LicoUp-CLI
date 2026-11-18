//! The registration this package publishes to the adapter SDK's ports.
//!
//! The SDK owns the port ([`licoup_agent_adapter_sdk::port::AdapterParserSet`]);
//! this package owns the answer, because it owns the parser. Composition reads
//! [`parser_set`] and hands it to the SDK's registry, replay harness and host
//! queries, so adding this Agent's crate is one entry rather than four lists
//! that can drift.
//!
//! Both protocol-agnostic queries the SDK declares are answered here, from this
//! Agent's own evidence, rather than left fail-closed:
//!
//! - [`execution_transitions`] turns one Kilo execution outcome into the shared
//!   transition vocabulary.
//! - [`valid_identity`] answers whether a durable native session identity is a
//!   real Kilo session, judged from the identity this Agent's own endpoint would
//!   answer with.
//!
//! The adapter id and framing are this Agent's declaration, and the framing is
//! the same string its fixtures record, so a corpus cannot pass against another
//! channel.

use licoup_agent_adapter_sdk::adapters::AdapterContract;
use licoup_agent_adapter_sdk::port::{
    AdapterParserSet, DurableIdentityRequest, ExecutionOutcome, ParserRegistration,
};
use licoup_agent_adapter_sdk::{Transition, registry};

use crate::parser;

/// The one adapter this package carries.
pub const ADAPTER_ID: &str = parser::ID;

/// The framing its parser really speaks, and the channel its fixtures record.
pub const FRAMING: &str = parser::FRAMING;

/// This Agent's adapter declaration, as composition and the corpus check read it.
pub const CONTRACT: AdapterContract = parser::CONTRACT;

/// This Agent's registration, as composition reads it.
///
/// It is a `const` rather than only an element of [`registrations`] because a
/// composing program builds its own parser list at compile time and needs a
/// constant expression to put here.
pub const REGISTRATION: ParserRegistration =
    ParserRegistration::new(CONTRACT, execution_transitions, valid_identity);

/// The registrations this package injects into the adapter SDK.
static REGISTRATIONS: [ParserRegistration; 1] = [REGISTRATION];

/// The parser registrations this package publishes.
pub fn registrations() -> &'static [ParserRegistration] {
    &REGISTRATIONS
}

/// The parser set composition injects: one Agent parser, and the replay arm this
/// package builds exactly as its own driver does.
pub const fn parser_set() -> AdapterParserSet {
    AdapterParserSet {
        registrations,
        replay: crate::replay::replay_arm,
    }
}

/// The adapter declaration of this package's parser, read through the SDK's own
/// registry lookup rather than from the constant, so the lookup and the set
/// cannot disagree.
pub fn contract() -> Option<AdapterContract> {
    registry::parser_for(&parser_set(), ADAPTER_ID)
}

/// This Agent's normalized transitions for one execution outcome.
///
/// A completed execution becomes this Agent's reply transitions at the terminal
/// stage; a failed one becomes the shared failure transition with the protocol's
/// own code, stage and redacted message. The projection is a field copy: the
/// facts already arrived on the outcome.
pub fn execution_transitions(outcome: &ExecutionOutcome<'_>) -> Vec<Transition> {
    match outcome.failure {
        None => parser::completed_transitions(outcome.output),
        Some(failure) => parser::failure_transitions(failure.code, failure.stage, failure.message),
    }
}

/// Whether one durable native session identity is a real Kilo session.
///
/// Kilo Code persists no rollout record this package can read — the endpoint
/// owns its sessions and answers about them over HTTP — so the binding carries
/// no location to open and the identity is judged on its own shape: non-empty,
/// bounded and free of control characters. That is exactly what this host's
/// exact-identity resolution does for an Agent that keeps no on-disk record, and
/// it never authorizes a resume by file name, because there is no file name.
pub fn valid_identity(request: &DurableIdentityRequest<'_>) -> bool {
    match request.location {
        Some(location) => {
            valid_session_identity(request.session_id)
                && location
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name == request.session_id)
        }
        None => valid_session_identity(request.session_id),
    }
}

fn valid_session_identity(value: &str) -> bool {
    !value.is_empty() && value.len() <= 512 && !value.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use super::*;
    use licoup_agent_adapter_sdk::port::ExecutionFailure;
    use licoup_agent_adapter_sdk::{LifecycleStage, Transition};

    #[test]
    fn the_registration_answers_both_queries_from_this_agents_own_parser() {
        let set = parser_set();
        assert_eq!(set.registered_ids(), vec!["kilo-code"]);
        assert_eq!(set.framing("kilo-code").unwrap(), "http-sse");
        assert_eq!(contract().unwrap().id, "kilo-code");

        let completed = set
            .execution_transitions(
                "kilo-code",
                &ExecutionOutcome {
                    output: "answer",
                    failure: None,
                },
            )
            .unwrap();
        assert!(matches!(
            completed.last(),
            Some(Transition::Lifecycle(LifecycleStage::Completed))
        ));
        assert!(completed.iter().any(|transition| matches!(
            transition,
            Transition::Text { text, .. } if text == "answer"
        )));

        let failed = set
            .execution_transitions(
                "kilo-code",
                &ExecutionOutcome {
                    output: "",
                    failure: Some(ExecutionFailure {
                        code: "kilo_code_serve_sse_closed",
                        stage: "serve/sse",
                        message: "safe",
                    }),
                },
            )
            .unwrap();
        assert!(matches!(
            failed.last(),
            Some(Transition::Failed { code, .. }) if code == "kilo_code_serve_sse_closed"
        ));
    }

    #[test]
    fn an_absent_location_is_judged_on_shape_and_a_named_one_must_match_it() {
        let set = parser_set();
        let request = DurableIdentityRequest {
            session_id: "kilo-1",
            location: None,
        };
        assert_eq!(set.valid_identity("kilo-code", &request), Some(true));
        assert_eq!(
            set.valid_identity(
                "kilo-code",
                &DurableIdentityRequest {
                    session_id: "",
                    location: None,
                }
            ),
            Some(false)
        );
        assert_eq!(
            set.valid_identity(
                "kilo-code",
                &DurableIdentityRequest {
                    session_id: "kilo-1",
                    location: Some(std::path::Path::new("/fixtures/sessions/kilo-1")),
                }
            ),
            Some(true)
        );
        // A locator that names something else never authorizes this identity.
        assert_eq!(
            set.valid_identity(
                "kilo-code",
                &DurableIdentityRequest {
                    session_id: "kilo-1",
                    location: Some(std::path::Path::new("/fixtures/sessions/other")),
                }
            ),
            Some(false)
        );
        // Control characters are never a session identity.
        assert_eq!(
            set.valid_identity(
                "kilo-code",
                &DurableIdentityRequest {
                    session_id: "kilo\u{0}1",
                    location: None,
                }
            ),
            Some(false)
        );
    }

    #[test]
    fn an_unregistered_adapter_answers_nothing_rather_than_defaulting() {
        let set = parser_set();
        assert!(set.registration("codex").is_none());
        assert!(set.framing("codex").is_err());
        assert!(set.replay_for("codex").is_err());
    }
}
