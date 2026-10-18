use super::super::*;
use licoup_native::ffi::generated::client_state::{
    ClientStateFailure, ClientStateFailureCode, ClientStateGetRequest, ClientStateSetRequest,
};

pub(super) fn get<W: Write>(
    writer: &Arc<StdioRpcWriter<W>>,
    lane: StdioRpcFrameLane,
    request_id: &str,
    workflow_id: &str,
    state_request: ClientStateGetRequest,
    portable_data_dir: Option<PathBuf>,
) -> Result<()> {
    let execution = catch_unwind(AssertUnwindSafe(|| {
        let _guard = PortableDataDirOverrideGuard::set(portable_data_dir);
        licoup_native::platform::client_state::state_get(state_request)
    }));
    write_result(writer, lane, request_id, workflow_id, execution)
}

pub(super) fn set<W: Write>(
    writer: &Arc<StdioRpcWriter<W>>,
    lane: StdioRpcFrameLane,
    request_id: &str,
    workflow_id: &str,
    state_request: ClientStateSetRequest,
    portable_data_dir: Option<PathBuf>,
) -> Result<()> {
    let execution = catch_unwind(AssertUnwindSafe(|| {
        let _guard = PortableDataDirOverrideGuard::set(portable_data_dir);
        licoup_native::platform::client_state::state_set(state_request)
    }));
    write_result(writer, lane, request_id, workflow_id, execution)
}

fn write_result<W: Write, T: serde::Serialize>(
    writer: &Arc<StdioRpcWriter<W>>,
    lane: StdioRpcFrameLane,
    request_id: &str,
    workflow_id: &str,
    execution: std::thread::Result<std::result::Result<T, anyhow::Error>>,
) -> Result<()> {
    let target = writer.lane_target(lane);
    match execution {
        Ok(Ok(result)) => Ok(write_stdio_rpc_success_shared(
            target,
            request_id,
            workflow_id,
            serde_json::to_value(result)?,
        )?),
        Ok(Err(_)) | Err(_) => Ok(write_stdio_rpc_client_error_shared(
            target,
            Some(request_id),
            Some(workflow_id),
            &stdio_rpc_state_failure(ClientStateFailure::new(
                ClientStateFailureCode::StateOperationFailed,
            )),
        )?),
    }
}
