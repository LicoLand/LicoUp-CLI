use super::*;
use licoup_native::ffi::generated::client_error::ClientError;
use serde_json::Value;

#[path = "error/cases.rs"]
mod cases;

#[test]
fn typed_client_error_metadata_survives_command_and_terminal_rpc_frames() {
    for (index, expected) in cases::expected_errors().into_iter().enumerate() {
        let error: ClientError = serde_json::from_value(expected.clone()).unwrap();
        let request_id = format!("request-{index}");

        let mut command_writer = Vec::new();
        write_stdio_rpc_error(
            &mut command_writer,
            Some(&request_id),
            Some("workflow-1"),
            &error,
        )
        .unwrap();
        let command: Value = serde_json::from_slice(&command_writer).unwrap();
        assert_eq!(command["error"], expected);

        let terminal_writer = Arc::new(StdioRpcWriter::new(Vec::new()));
        write_stdio_rpc_terminal_error(
            terminal_writer.stream(),
            &request_id,
            "workflow-1",
            1,
            &error,
        )
        .unwrap();
        let terminal: Value =
            serde_json::from_slice(&recover_stdio_rpc_writer(terminal_writer).unwrap()).unwrap();
        assert_eq!(terminal["kind"], "terminal");
        assert_eq!(terminal["sequence"], 1);
        assert_eq!(terminal["error"], expected);
    }
}
