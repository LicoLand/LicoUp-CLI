//! This Agent's driver declaration, as the host composition reads it.

use super::super::{DRIVER, DRIVER_ID, ERROR_PREFIX, RUNTIME_PROTOCOL};

#[test]
fn the_driver_declaration_is_read_from_the_package_that_owns_it() {
    assert_eq!(RUNTIME_PROTOCOL, "kilo-code-serve-http-v1");
    assert_eq!(DRIVER.runtime_protocol, RUNTIME_PROTOCOL);
    assert_eq!(DRIVER.launch_args, &["serve"]);
    assert_eq!(DRIVER.agent_id, "kilo-code-serve");
    assert_eq!(DRIVER.agent_id, DRIVER_ID);
    assert_eq!(DRIVER.error_prefix, "kilo_code_serve");
    assert_eq!(DRIVER.error_prefix, ERROR_PREFIX);
}

#[test]
fn the_launch_shape_starts_a_serve_endpoint_and_never_a_chat_continuation() {
    assert!(DRIVER.launch_args.iter().all(|argument| {
        *argument != "acp"
            && !argument.contains("continue")
            && !argument.contains("session")
            && !argument.contains("prompt")
    }));
}
