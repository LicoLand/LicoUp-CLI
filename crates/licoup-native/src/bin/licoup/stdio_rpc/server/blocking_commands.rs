use super::super::*;
use super::conversation;
use licoup_native::ffi::commands::{AdmittedCommand, CliExecution, admit_cli_command};

/// Only these admitted commands may leave the ordered CLI lane. Target probes
/// can wait for model discovery; migration can wait for direct OS approval.
/// Neither wait may prevent this same host from receiving another RPC frame.
pub(super) fn admit(args: &[String]) -> Result<Option<AdmittedCommand>> {
    let candidate = matches!(args, [family, operation, ..]
        if family == "targets" && operation == "scan")
        || matches!(args, [family, scope, operation, ..]
            if family == "llm-gateway" && scope == "credentials" && operation == "migrate");
    if !candidate {
        return Ok(None);
    }
    let command = admit_cli_command(args.to_vec())?;
    Ok(matches!(
        command.path(),
        ["targets", "scan"] | ["llm-gateway", "credentials", "migrate"]
    )
    .then_some(command))
}

type Executor = dyn Fn(AdmittedCommand) -> Result<CliExecution> + Send + Sync;

pub(super) struct Workers {
    handles: Vec<std::thread::JoinHandle<()>>,
    execute: Arc<Executor>,
}

impl Default for Workers {
    fn default() -> Self {
        Self {
            handles: Vec::new(),
            execute: Arc::new(AdmittedCommand::execute),
        }
    }
}

impl Workers {
    pub(super) fn reap_finished(&mut self) {
        conversation::reap_finished(&mut self.handles);
    }

    pub(super) fn join_until_completion(&mut self) {
        conversation::join_until_completion(&mut self.handles);
    }

    pub(super) fn spawn<W: Write + Send + 'static>(
        &mut self,
        writer: Arc<StdioRpcWriter<W>>,
        request_id: String,
        workflow_id: String,
        command: AdmittedCommand,
        portable_data_dir: Option<PathBuf>,
    ) -> io::Result<()> {
        if !conversation::has_capacity(&self.handles) {
            return Err(io::Error::other("RPC command capacity exhausted"));
        }
        let execute = Arc::clone(&self.execute);
        let worker = std::thread::Builder::new()
            .name("rpc-blocking-command".to_owned())
            .spawn(move || {
                let result = catch_unwind(AssertUnwindSafe(|| {
                    let _guard = PortableDataDirOverrideGuard::set(portable_data_dir);
                    execute(command)
                }));
                let _ = write_result(
                    &writer,
                    StdioRpcFrameLane::Stream,
                    &request_id,
                    &workflow_id,
                    result,
                );
            })?;
        self.handles.push(worker);
        Ok(())
    }
}

pub(super) fn write_result<W: Write>(
    writer: &Arc<StdioRpcWriter<W>>,
    lane: StdioRpcFrameLane,
    request_id: &str,
    workflow_id: &str,
    execution: std::thread::Result<Result<CliExecution>>,
) -> Result<()> {
    let target = writer.lane_target(lane);
    match execution {
        Ok(Ok(CliExecution::Json(value))) => {
            write_stdio_rpc_success_shared(target, request_id, workflow_id, value)?
        }
        Ok(Ok(CliExecution::Usage)) => write_stdio_rpc_error_shared(
            target,
            Some(request_id),
            Some(workflow_id),
            "command_usage",
        )?,
        Ok(Ok(CliExecution::Streamed)) => write_stdio_rpc_error_shared(
            target,
            Some(request_id),
            Some(workflow_id),
            "streaming_command_unsupported",
        )?,
        Ok(Err(error)) => write_stdio_rpc_client_error_shared(
            target,
            Some(request_id),
            Some(workflow_id),
            &stdio_rpc_command_error(&error),
        )?,
        Err(_) => write_stdio_rpc_error_shared(
            target,
            Some(request_id),
            Some(workflow_id),
            "command_panicked",
        )?,
    }
    Ok(())
}

#[cfg(test)]
#[path = "blocking_commands_tests.rs"]
mod tests;
