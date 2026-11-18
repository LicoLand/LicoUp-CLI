use licoup_conversation::{
    ALL_SEND_EVENTS, ALL_SEND_STATES, ALL_TURN_EVENTS, ALL_TURN_STATES, SEND_TRANSITIONS,
    SendEvent, SendState, TURN_TRANSITIONS, TurnEvent, TurnState,
};
use proptest::prelude::*;

fn turn_event() -> impl Strategy<Value = TurnEvent> {
    prop_oneof![
        Just(TurnEvent::Claim),
        Just(TurnEvent::Start),
        Just(TurnEvent::WaitForHuman),
        Just(TurnEvent::Resume),
        Just(TurnEvent::Succeed),
        Just(TurnEvent::Fail),
        Just(TurnEvent::Interrupt),
        Just(TurnEvent::Cancel),
    ]
}

fn send_event() -> impl Strategy<Value = SendEvent> {
    prop_oneof![Just(SendEvent::Deliver), Just(SendEvent::Fail)]
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 64,
        max_shrink_iters: 2_048,
        .. ProptestConfig::default()
    })]

    #[test]
    fn random_ten_thousand_event_turn_sequences_never_escape_terminal_states(
        events in proptest::collection::vec(turn_event(), 10_000)
    ) {
        let mut state = TurnState::Pending;
        for event in events {
            let before = state;
            if let Ok(next) = state.transition(event) {
                state = next;
            }
            if before.is_terminal() {
                prop_assert_eq!(state, before);
            }
        }
    }

    #[test]
    fn random_ten_thousand_event_send_sequences_never_escape_terminal_states(
        events in proptest::collection::vec(send_event(), 10_000)
    ) {
        let mut state = SendState::Sending;
        for event in events {
            let before = state;
            state = state.transition(event).expect("every send signal is defined");
            if before.is_terminal() {
                prop_assert_eq!(state, before);
            }
        }
    }
}

#[test]
fn published_turn_table_exactly_matches_transition_function() {
    for &state in ALL_TURN_STATES {
        for &event in ALL_TURN_EVENTS {
            let table_result = TURN_TRANSITIONS
                .iter()
                .find(|transition| transition.from == state && transition.event == event)
                .map(|transition| transition.to);
            assert_eq!(state.transition(event).ok(), table_result);
        }
    }
}

#[test]
fn published_send_table_exactly_matches_transition_function() {
    for &state in ALL_SEND_STATES {
        for &event in ALL_SEND_EVENTS {
            let table_result = SEND_TRANSITIONS
                .iter()
                .find(|transition| transition.from == state && transition.event == event)
                .map(|transition| transition.to);
            assert_eq!(state.transition(event).ok(), table_result);
        }
    }
}
