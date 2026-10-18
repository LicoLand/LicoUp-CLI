// OpenClaw-specific JSON-RPC/ACP framing and response classification.
use licoup_foundation::core::acp;
use serde_json::Value;
use std::io;

pub const INITIALIZE_REQUEST_ID: i64 = 1;
pub const SESSION_REQUEST_ID: i64 = 2;
pub const MODE_REQUEST_ID: i64 = 3;
pub const PROMPT_REQUEST_ID: i64 = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DecodeFailure {
    Invalid,
    TooLarge,
}

pub fn encode_message(message: &Value) -> io::Result<Vec<u8>> {
    acp::encode_json_line(message).map_err(io::Error::other)
}

pub fn decode_message(line: &[u8]) -> Result<Value, DecodeFailure> {
    acp::decode_json_line(line).map_err(|error| match error {
        acp::AcpError::MessageTooLarge => DecodeFailure::TooLarge,
        _ => DecodeFailure::Invalid,
    })
}

pub fn response_is_error(message: &Value) -> bool {
    message.get("error").is_some()
}

pub fn request_id_matches(message: &Value, expected: i64) -> bool {
    message.get("id").is_some_and(|id| {
        id.as_i64() == Some(expected)
            || id
                .as_str()
                .is_some_and(|value| value == expected.to_string())
    })
}
