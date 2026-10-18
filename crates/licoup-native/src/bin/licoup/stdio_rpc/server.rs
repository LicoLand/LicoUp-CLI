use super::*;
use licoup_native::domain::client_conversation::ConversationService;
use licoup_native::platform::runtime_adapters::RuntimeAdapterError;
use std::collections::VecDeque;

const MAX_CONVERSATION_SERVICE_ROOTS: usize = 4;

#[derive(Default)]
struct ConversationServices {
    entries: VecDeque<(Option<PathBuf>, ConversationService)>,
}

#[path = "server/client_conversation.rs"]
mod client_conversation;
#[path = "server/conversation.rs"]
mod conversation;
pub(crate) use conversation::PersistentConversationRuntime;
#[path = "server/blocking_commands.rs"]
mod blocking_commands;
#[cfg(test)]
#[path = "server/data_home_tests.rs"]
mod data_home_tests;
#[path = "server/state.rs"]
mod state;
#[cfg(test)]
#[path = "server/work_control_routing_tests.rs"]
mod work_control_routing_tests;

pub(crate) fn serve_stdio_rpc<R, W, F>(reader: R, writer: W, execute: F) -> Result<W>
where
    R: BufRead,
    W: Write + Send + 'static,
    F: FnMut(Vec<String>, Option<PathBuf>) -> Result<licoup_native::ffi::commands::CliExecution>,
{
    serve_stdio_rpc_inner(
        reader,
        writer,
        execute,
        None,
        None,
        false,
        blocking_commands::Workers::default(),
    )
}

/// One-shot bridge reserved for the data-home relocation owner. Its process
/// deliberately has no shared data-home lease, so only this operation may
/// acquire the exclusive relocation lease.
pub(crate) fn serve_data_home_stdio_rpc<R, W, F>(reader: R, writer: W, execute: F) -> Result<W>
where
    R: BufRead,
    W: Write + Send + 'static,
    F: FnMut(Vec<String>, Option<PathBuf>) -> Result<licoup_native::ffi::commands::CliExecution>,
{
    serve_stdio_rpc_inner(
        reader,
        writer,
        execute,
        None,
        None,
        true,
        blocking_commands::Workers::default(),
    )
}

#[cfg(test)]
pub(crate) fn serve_stdio_rpc_with_runtime<R, W, F>(
    reader: R,
    writer: W,
    execute: F,
    conversation_runtime: PersistentConversationRuntime,
) -> Result<W>
where
    R: BufRead,
    W: Write + Send + 'static,
    F: FnMut(Vec<String>, Option<PathBuf>) -> Result<licoup_native::ffi::commands::CliExecution>,
{
    serve_stdio_rpc_inner(
        reader,
        writer,
        execute,
        Some(conversation_runtime),
        None,
        false,
        blocking_commands::Workers::default(),
    )
}

pub(crate) fn serve_stdio_rpc_with_persistent_conversation<R, W, F>(
    reader: R,
    writer: W,
    execute: F,
    conversation_runtime: PersistentConversationRuntime,
    conversation_service: ConversationService,
) -> Result<W>
where
    R: BufRead,
    W: Write + Send + 'static,
    F: FnMut(Vec<String>, Option<PathBuf>) -> Result<licoup_native::ffi::commands::CliExecution>,
{
    serve_stdio_rpc_inner(
        reader,
        writer,
        execute,
        Some(conversation_runtime),
        Some(conversation_service),
        false,
        blocking_commands::Workers::default(),
    )
}

fn serve_stdio_rpc_inner<R, W, F>(
    mut reader: R,
    writer: W,
    mut execute: F,
    conversation_runtime: Option<PersistentConversationRuntime>,
    initial_conversation_service: Option<ConversationService>,
    data_home_rpc_enabled: bool,
    mut blocking_commands: blocking_commands::Workers,
) -> Result<W>
where
    R: BufRead,
    W: Write + Send + 'static,
    F: FnMut(Vec<String>, Option<PathBuf>) -> Result<licoup_native::ffi::commands::CliExecution>,
{
    let writer = Arc::new(StdioRpcWriter::new(writer));
    let mut bound_workflow_id: Option<String> = None;
    let mut conversation_workers = Vec::new();
    let mut conversation_services = ConversationServices::default();
    if let Some(service) = initial_conversation_service {
        // The host (or test) already bound this instance. Re-binding here
        // would replace a typed admitted/held cognition port with the
        // generic public binder and hide the actual listener owner.
        conversation_services.entries.push_back((None, service));
    }
    loop {
        conversation::reap_finished(&mut conversation_workers);
        blocking_commands.reap_finished();
        let line = read_stdio_rpc_line(&mut reader, STDIO_RPC_MAX_REQUEST_BYTES)?;
        let bytes = match line {
            StdioRpcLine::Eof => {
                // The observer disappeared; the dispatched work did not. Keep
                // this native host alive until every Agent reaches a terminal
                // state. Only the explicit conversation cancel operation may
                // interrupt a running turn.
                conversation::join_until_completion(&mut conversation_workers);
                blocking_commands.join_until_completion();
                return recover_stdio_rpc_writer(writer);
            }
            StdioRpcLine::TooLarge => {
                write_stdio_rpc_error_shared(
                    writer.ordered(),
                    None,
                    bound_workflow_id.as_deref(),
                    "request_too_large",
                )?;
                continue;
            }
            StdioRpcLine::Request(bytes) => bytes,
        };
        let request = match parse_stdio_rpc_request(&bytes) {
            Ok(request) => request,
            Err(error) => {
                write_stdio_rpc_error_shared(
                    writer.ordered(),
                    error.id.as_deref(),
                    error.workflow_id.as_deref(),
                    error.code,
                )?;
                continue;
            }
        };
        if bound_workflow_id
            .as_deref()
            .is_some_and(|workflow_id| workflow_id != request.workflow_id.as_str())
        {
            write_stdio_rpc_error_shared(
                writer.ordered(),
                Some(&request.id),
                Some(&request.workflow_id),
                "workflow_mismatch",
            )?;
            continue;
        }
        if bound_workflow_id.is_none() {
            bound_workflow_id = Some(request.workflow_id.clone());
        }
        let data_home_mutation = matches!(
            &request.method,
            StdioRpcMethod::DataHomeRelocate { .. }
                | StdioRpcMethod::DataHomeRecover { .. }
                | StdioRpcMethod::DataHomeCleanup { .. }
        );
        let data_home_status = matches!(&request.method, StdioRpcMethod::DataHomeStatus);
        if (data_home_rpc_enabled && (!data_home_mutation || data_home_status))
            || (!data_home_rpc_enabled && data_home_mutation)
        {
            write_stdio_rpc_error_shared(
                writer.ordered(),
                Some(&request.id),
                Some(&request.workflow_id),
                "data_home_operation_requires_dedicated_process",
            )?;
            if data_home_rpc_enabled {
                return recover_stdio_rpc_writer(writer);
            }
            continue;
        }

        // One host-facing boundary: the whole per-request dispatch is
        // unwind-safe. A panic in any arm (including arms without their own
        // catch-unwind guard) becomes a structured error delta and the frame
        // loop keeps serving; only a response-write failure or the explicit
        // shutdown acknowledgment may end the loop.
        let dispatch = catch_unwind(AssertUnwindSafe(|| -> Result<bool> {
            match request.method {
                StdioRpcMethod::StateGet {
                    request: state_request,
                    portable_data_dir,
                } => {
                    state::get(
                        &writer,
                        request.lane,
                        &request.id,
                        &request.workflow_id,
                        state_request,
                        portable_data_dir,
                    )?;
                }
                StdioRpcMethod::StateSet {
                    request: state_request,
                    portable_data_dir,
                } => {
                    state::set(
                        &writer,
                        request.lane,
                        &request.id,
                        &request.workflow_id,
                        state_request,
                        portable_data_dir,
                    )?;
                }
                StdioRpcMethod::Shutdown { params } => {
                    let stop_host = params
                        .get("host")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    if stop_host {
                        if let Some(runtime) = conversation_runtime.as_ref() {
                            runtime.request_host_stop();
                        }
                    }
                    write_stdio_rpc_success_shared(
                        writer.lane_target(request.lane),
                        &request.id,
                        &request.workflow_id,
                        json!({"status": "shutdown", "host_stop_requested": stop_host}),
                    )?;
                    // Shutdown closes the RPC session, not the Agent turns it has
                    // already accepted. Acknowledge first so the client can leave.
                    conversation::join_until_completion(&mut conversation_workers);
                    blocking_commands.join_until_completion();
                    return Ok(true);
                }
                StdioRpcMethod::DataHomeRelocate { params } => {
                    dispatch_data_home_result(
                        &writer,
                        request.lane,
                        &request.id,
                        &request.workflow_id,
                        || licoup_native::platform::data_home_relocation::relocate(&params),
                    )?;
                    return Ok(true);
                }
                StdioRpcMethod::DataHomeStatus => dispatch_data_home_result(
                    &writer,
                    request.lane,
                    &request.id,
                    &request.workflow_id,
                    licoup_native::platform::data_home_relocation::status,
                )?,
                StdioRpcMethod::DataHomeRecover { params } => {
                    dispatch_data_home_result(
                        &writer,
                        request.lane,
                        &request.id,
                        &request.workflow_id,
                        || licoup_native::platform::data_home_relocation::recover(&params),
                    )?;
                    return Ok(true);
                }
                StdioRpcMethod::DataHomeCleanup { params } => {
                    dispatch_data_home_result(
                        &writer,
                        request.lane,
                        &request.id,
                        &request.workflow_id,
                        || licoup_native::platform::data_home_relocation::cleanup_previous(&params),
                    )?;
                    return Ok(true);
                }
                StdioRpcMethod::Conversation {
                    operation,
                    params,
                    portable_data_dir,
                } => {
                    let persistent_operation = matches!(
                        operation.as_str(),
                        "send"
                            | "dispatch"
                            | "stream"
                            | "steer"
                            | "cancel"
                            | "active"
                            | "attach"
                            | "execution"
                            | "execution.detach"
                    );
                    if persistent_operation && conversation_runtime.is_none() {
                        let rejection = persistent_runtime_rejection();
                        if matches!(
                            operation.as_str(),
                            "send" | "stream" | "attach" | "execution"
                        ) {
                            write_stdio_rpc_terminal_success(
                                writer.lane_target(request.lane),
                                &request.id,
                                &request.workflow_id,
                                1,
                                rejection,
                            )?;
                        } else {
                            write_stdio_rpc_success_shared(
                                writer.lane_target(request.lane),
                                &request.id,
                                &request.workflow_id,
                                rejection,
                            )?;
                        }
                        return Ok(false);
                    }
                    if operation == "dispatch" {
                        let runtime = conversation_runtime
                            .as_ref()
                            .expect("persistent operation validated");
                        match runtime.start_background(&params, portable_data_dir) {
                            Ok(value) => write_stdio_rpc_success_shared(
                                writer.lane_target(request.lane),
                                &request.id,
                                &request.workflow_id,
                                value,
                            )?,
                            Err(error) => write_stdio_rpc_client_error_shared(
                                writer.lane_target(request.lane),
                                Some(&request.id),
                                Some(&request.workflow_id),
                                &licoup_native::platform::runtime_adapters::client_error::client_error(&error),
                            )?,
                        }
                    } else if operation == "send" {
                        let runtime = conversation_runtime
                            .as_ref()
                            .expect("persistent operation validated");
                        if !conversation::has_capacity(&conversation_workers) {
                            write_stdio_rpc_terminal_error(
                                writer.lane_target(request.lane),
                                &request.id,
                                &request.workflow_id,
                                1,
                                &stdio_rpc_client_error("conversation_capacity_exhausted"),
                            )?;
                            return Ok(false);
                        }
                        match conversation::spawn_send(
                            Arc::clone(&writer),
                            request.id.clone(),
                            request.workflow_id.clone(),
                            params,
                            portable_data_dir,
                            runtime.clone(),
                        ) {
                            Ok(worker) => conversation_workers.push(worker),
                            Err(error) => write_stdio_rpc_terminal_error(
                                writer.lane_target(request.lane),
                                &request.id,
                                &request.workflow_id,
                                1,
                                &error,
                            )?,
                        }
                    } else if operation == "execution.detach" {
                        let runtime = conversation_runtime
                            .as_ref()
                            .expect("persistent operation validated");
                        match runtime.detach_execution(&params) {
                            Ok(value) => write_stdio_rpc_success_shared(
                                writer.lane_target(request.lane),
                                &request.id,
                                &request.workflow_id,
                                value,
                            )?,
                            Err(error) => write_stdio_rpc_client_error_shared(
                                writer.lane_target(request.lane),
                                Some(&request.id),
                                Some(&request.workflow_id),
                                &error,
                            )?,
                        }
                    } else if operation == "execution" {
                        let runtime = conversation_runtime
                            .as_ref()
                            .expect("persistent operation validated");
                        match conversation::spawn_execution(
                            Arc::clone(&writer),
                            request.id.clone(),
                            request.workflow_id.clone(),
                            params,
                            runtime.clone(),
                        ) {
                            Ok(worker) => conversation_workers.push(worker),
                            Err(error) => write_stdio_rpc_terminal_error(
                                writer.lane_target(request.lane),
                                &request.id,
                                &request.workflow_id,
                                1,
                                &error,
                            )?,
                        }
                    } else if operation == "attach" {
                        let runtime = conversation_runtime
                            .as_ref()
                            .expect("persistent operation validated");
                        match conversation::spawn_attach(
                            Arc::clone(&writer),
                            request.id.clone(),
                            request.workflow_id.clone(),
                            params,
                            runtime.clone(),
                        ) {
                            Ok(worker) => conversation_workers.push(worker),
                            Err(error) => write_stdio_rpc_terminal_error(
                                writer.lane_target(request.lane),
                                &request.id,
                                &request.workflow_id,
                                1,
                                &error,
                            )?,
                        }
                    } else if operation == "active" {
                        let runtime = conversation_runtime
                            .as_ref()
                            .expect("persistent operation validated");
                        write_stdio_rpc_success_shared(
                            writer.lane_target(request.lane),
                            &request.id,
                            &request.workflow_id,
                            runtime.active(&params),
                        )?;
                    } else if matches!(
                        operation.as_str(),
                        "stop" | "force.preview" | "force.confirm"
                    ) {
                        // Manual stop and owned-process force stop are control
                        // operations: they never fabricate a business success,
                        // and a force-stop confirm terminates only the
                        // re-verified owned process group.
                        let execution = catch_unwind(AssertUnwindSafe(|| {
                            let _guard =
                                PortableDataDirOverrideGuard::set(portable_data_dir.clone());
                            match operation.as_str() {
                                "stop" => conversation::stop_task(
                                    &params,
                                    conversation_runtime.as_ref(),
                                    portable_data_dir,
                                ),
                                "force.preview" => {
                                    licoup_native::platform::stop_control::force_stop_preview(
                                        &params,
                                    )
                                }
                                _ => licoup_native::platform::stop_control::force_stop_confirm(
                                    &params,
                                ),
                            }
                        }));
                        match execution {
                            Ok(value) => write_stdio_rpc_success_shared(
                                writer.lane_target(request.lane),
                                &request.id,
                                &request.workflow_id,
                                value,
                            )?,
                            Err(_) => write_stdio_rpc_error_shared(
                                writer.lane_target(request.lane),
                                Some(&request.id),
                                Some(&request.workflow_id),
                                "command_panicked",
                            )?,
                        }
                    } else if operation == "cancel" {
                        let runtime = conversation_runtime
                            .as_ref()
                            .expect("persistent operation validated");
                        match runtime.request_cancel(&params) {
                            Ok(value) => write_stdio_rpc_success_shared(
                                writer.lane_target(request.lane),
                                &request.id,
                                &request.workflow_id,
                                value,
                            )?,
                            Err(error) => write_stdio_rpc_terminal_error(
                                writer.lane_target(request.lane),
                                &request.id,
                                &request.workflow_id,
                                1,
                                &error,
                            )?,
                        }
                    } else {
                        let params = if operation == "steer" {
                            let runtime = conversation_runtime
                                .as_ref()
                                .expect("persistent operation validated");
                            match runtime.scoped_control_params(&params) {
                                Ok(params) => params,
                                Err(error) => {
                                    write_stdio_rpc_terminal_error(
                                        writer.lane_target(request.lane),
                                        &request.id,
                                        &request.workflow_id,
                                        1,
                                        &error,
                                    )?;
                                    return Ok(false);
                                }
                            }
                        } else {
                            params
                        };
                        conversation::execute(
                            &writer,
                            request.lane,
                            &request.id,
                            &request.workflow_id,
                            &operation,
                            params,
                            portable_data_dir,
                            false,
                            None,
                        )?;
                    }
                }
                StdioRpcMethod::ClientConversation {
                    params,
                    portable_data_dir,
                } => {
                    if client_conversation::requires_worker(&params)
                        && conversation_runtime.is_none()
                    {
                        write_stdio_rpc_success_shared(
                            writer.lane_target(request.lane),
                            &request.id,
                            &request.workflow_id,
                            persistent_runtime_rejection(),
                        )?;
                        return Ok(false);
                    }
                    let service = match conversation_service(
                        &mut conversation_services,
                        portable_data_dir.clone(),
                        conversation_runtime.as_ref(),
                    ) {
                        Ok(service) => service,
                        Err(error) => {
                            write_stdio_rpc_client_error_shared(
                                writer.lane_target(request.lane),
                                Some(&request.id),
                                Some(&request.workflow_id),
                                &stdio_rpc_command_error(&error),
                            )?;
                            return Ok(false);
                        }
                    };
                    if client_conversation::requires_worker(&params) {
                        if !conversation::has_capacity(&conversation_workers) {
                            write_stdio_rpc_client_error_shared(
                                writer.lane_target(request.lane),
                                Some(&request.id),
                                Some(&request.workflow_id),
                                &stdio_rpc_client_error("conversation_capacity_exhausted"),
                            )?;
                            return Ok(false);
                        }
                        match client_conversation::spawn_execute(
                            Arc::clone(&writer),
                            request.id.clone(),
                            request.workflow_id.clone(),
                            params,
                            service,
                            portable_data_dir,
                        ) {
                            Ok(worker) => conversation_workers.push(worker),
                            Err(_) => write_stdio_rpc_success_shared(
                                writer.lane_target(request.lane),
                                &request.id,
                                &request.workflow_id,
                                json!({
                                    "ok": false,
                                    "error": {
                                        "code": "conversation_dispatch_failed",
                                        "stage": "conversation/dispatch",
                                    }
                                }),
                            )?,
                        }
                    } else {
                        client_conversation::execute(
                            &writer,
                            request.lane,
                            &request.id,
                            &request.workflow_id,
                            params,
                            service,
                            portable_data_dir,
                        )?;
                    }
                }
                StdioRpcMethod::StrategyExecute {
                    params,
                    portable_data_dir,
                } => {
                    if strategy_requires_persistent_runtime(&params)
                        && conversation_runtime.is_none()
                    {
                        write_stdio_rpc_success_shared(
                            writer.lane_target(request.lane),
                            &request.id,
                            &request.workflow_id,
                            persistent_runtime_rejection(),
                        )?;
                        return Ok(false);
                    }
                    let runtime = conversation_runtime.clone();
                    let execution = catch_unwind(AssertUnwindSafe(|| {
                        let _guard = PortableDataDirOverrideGuard::set(portable_data_dir.clone());
                        let root = licoup_foundation::platform::paths::portable_data_dir()?;
                        let service =
                            licoup_native::domain::workflow_runtime::StrategyService::open(&root)?;
                        let service = if let Some(runtime) = runtime {
                            service
                                .with_actor_turn_port(conversation::strategy_turn_port(
                                    runtime.clone(),
                                    portable_data_dir.clone(),
                                ))
                                .with_assistant_wake_port(conversation::assistant_wake_port(
                                    runtime,
                                    portable_data_dir.clone(),
                                ))
                        } else {
                            service
                        };
                        service.execute(params)
                    }));
                    match execution {
                        Ok(Ok(value)) => write_stdio_rpc_success_shared(
                            writer.lane_target(request.lane),
                            &request.id,
                            &request.workflow_id,
                            value,
                        )?,
                        Ok(Err(error)) => write_stdio_rpc_client_error_shared(
                            writer.lane_target(request.lane),
                            Some(&request.id),
                            Some(&request.workflow_id),
                            &stdio_rpc_command_error(&error),
                        )?,
                        Err(_) => write_stdio_rpc_error_shared(
                            writer.lane_target(request.lane),
                            Some(&request.id),
                            Some(&request.workflow_id),
                            "command_panicked",
                        )?,
                    }
                }
                StdioRpcMethod::Selection {
                    request: selection_request,
                    portable_data_dir,
                } => {
                    // The selection facts and the durable policy both live in the
                    // data home this frame names, and the transitions write through
                    // the one store their owner opens. Nothing is composed here.
                    let execution = catch_unwind(AssertUnwindSafe(|| {
                        let _guard = PortableDataDirOverrideGuard::set(portable_data_dir);
                        selection_request.dispatch()
                    }));
                    match execution {
                        Ok(Ok(value)) => write_stdio_rpc_success_shared(
                            writer.lane_target(request.lane),
                            &request.id,
                            &request.workflow_id,
                            value,
                        )?,
                        Ok(Err(error)) => write_stdio_rpc_client_error_shared(
                            writer.lane_target(request.lane),
                            Some(&request.id),
                            Some(&request.workflow_id),
                            &error,
                        )?,
                        Err(_) => write_stdio_rpc_error_shared(
                            writer.lane_target(request.lane),
                            Some(&request.id),
                            Some(&request.workflow_id),
                            "command_panicked",
                        )?,
                    }
                }
                StdioRpcMethod::Catalog {
                    operation,
                    params,
                    portable_data_dir,
                } => {
                    let execution = catch_unwind(AssertUnwindSafe(|| {
                        let _guard = PortableDataDirOverrideGuard::set(portable_data_dir);
                        licoup_native::domain::catalog_convergence::dispatch(
                            &["catalog".to_string(), operation],
                            &params,
                        )
                    }));
                    match execution {
                        Ok(Ok(value)) => write_stdio_rpc_success_shared(
                            writer.lane_target(request.lane),
                            &request.id,
                            &request.workflow_id,
                            value,
                        )?,
                        Ok(Err(error)) => write_stdio_rpc_client_error_shared(
                            writer.lane_target(request.lane),
                            Some(&request.id),
                            Some(&request.workflow_id),
                            &stdio_rpc_command_error(&error),
                        )?,
                        Err(_) => write_stdio_rpc_error_shared(
                            writer.lane_target(request.lane),
                            Some(&request.id),
                            Some(&request.workflow_id),
                            "command_panicked",
                        )?,
                    }
                }
                StdioRpcMethod::Execute {
                    args,
                    portable_data_dir,
                } => {
                    if rpc_args_dispatch_conversation(&args) {
                        match licoup_native::ffi::commands::admit_cli_command(args) {
                            Ok(_) => write_stdio_rpc_success_shared(
                                writer.lane_target(request.lane),
                                &request.id,
                                &request.workflow_id,
                                persistent_runtime_rejection(),
                            )?,
                            Err(error) => write_stdio_rpc_client_error_shared(
                                writer.lane_target(request.lane),
                                Some(&request.id),
                                Some(&request.workflow_id),
                                &stdio_rpc_command_error(&error),
                            )?,
                        }
                        return Ok(false);
                    }
                    if rpc_command_reads_external_stdin(&args) {
                        write_stdio_rpc_error_shared(
                            writer.lane_target(request.lane),
                            Some(&request.id),
                            Some(&request.workflow_id),
                            "private_input_transport_required",
                        )?;
                        return Ok(false);
                    }
                    if rpc_command_writes_external_stdout(&args) {
                        write_stdio_rpc_error_shared(
                            writer.lane_target(request.lane),
                            Some(&request.id),
                            Some(&request.workflow_id),
                            "streaming_command_unsupported",
                        )?;
                        return Ok(false);
                    }
                    let admitted = match blocking_commands::admit(&args) {
                        Ok(command) => command,
                        Err(error) => {
                            write_stdio_rpc_client_error_shared(
                                writer.lane_target(request.lane),
                                Some(&request.id),
                                Some(&request.workflow_id),
                                &stdio_rpc_command_error(&error),
                            )?;
                            return Ok(false);
                        }
                    };
                    if let Some(command) = admitted {
                        if let Err(error) = blocking_commands.spawn(
                            Arc::clone(&writer),
                            request.id.clone(),
                            request.workflow_id.clone(),
                            command,
                            portable_data_dir,
                        ) {
                            write_stdio_rpc_client_error_shared(
                                writer.lane_target(request.lane),
                                Some(&request.id),
                                Some(&request.workflow_id),
                                &stdio_rpc_command_error(&error.into()),
                            )?;
                        }
                        return Ok(false);
                    }
                    blocking_commands::write_result(
                        &writer,
                        request.lane,
                        &request.id,
                        &request.workflow_id,
                        catch_unwind(AssertUnwindSafe(|| execute(args, portable_data_dir))),
                    )?;
                }
            }
            Ok(false)
        }));
        match dispatch {
            Ok(Ok(true)) => return recover_stdio_rpc_writer(writer),
            Ok(Ok(false)) => {}
            Ok(Err(error)) => return Err(error),
            Err(_) => {
                write_stdio_rpc_error_shared(
                    writer.lane_target(request.lane),
                    Some(&request.id),
                    Some(&request.workflow_id),
                    "command_panicked",
                )?;
                continue;
            }
        }
    }
}

fn persistent_runtime_rejection() -> Value {
    json!({
        "ok": false,
        "error": {
            "code": licoup_native::domain::client_conversation::PERSISTENT_TRANSPORT_REQUIRED,
            "stage": "conversation/dispatch",
        }
    })
}

fn rpc_args_dispatch_conversation(args: &[String]) -> bool {
    matches!(
        args,
        [domain, conversation, operation, ..]
            if domain == "agent"
                && conversation == "conversation"
                && matches!(operation.as_str(), "send" | "stream" | "steer" | "cancel")
    ) || matches!(
        args,
        [conversation, execute, ..]
            if conversation == "conversation" && execute == "execute"
    ) || matches!(
        args,
        [strategy, execute, ..] if strategy == "strategy" && execute == "execute"
    )
}

pub(crate) fn strategy_requires_persistent_runtime(params: &Value) -> bool {
    params
        .get("action")
        .and_then(Value::as_str)
        .is_some_and(|action| {
            matches!(
                action,
                "strategy.run.start"
                    | "strategy.run.resume"
                    | "strategy.run.retry"
                    | "strategy.assistant.workflow.execute"
            )
        })
}

/// Open (once per portable data dir) the process-owned Conversation service.
/// The returned handle is cloned for every request and spawned worker, so all
/// sessions reuse one bounded SQLite pool instead of opening per-call
/// connections. The override guard resolves the root exactly like the legacy
/// per-request open did.
fn conversation_service(
    services: &mut ConversationServices,
    portable_data_dir: Option<PathBuf>,
    runtime: Option<&PersistentConversationRuntime>,
) -> Result<ConversationService> {
    if let Some(position) = services
        .entries
        .iter()
        .position(|(root, _)| root == &portable_data_dir)
    {
        let entry = services
            .entries
            .remove(position)
            .expect("conversation service position exists");
        let service = entry.1.clone();
        services.entries.push_back(entry);
        return Ok(service);
    }
    let _guard = PortableDataDirOverrideGuard::set(portable_data_dir.clone());
    let root = licoup_foundation::platform::paths::portable_data_dir()?;
    let service = ConversationService::open(&root)?;
    let service = match runtime {
        Some(runtime) => bind_conversation_runtime(service, runtime, portable_data_dir.clone()),
        None => service,
    };
    if services.entries.len() == MAX_CONVERSATION_SERVICE_ROOTS {
        services.entries.pop_front();
    }
    services
        .entries
        .push_back((portable_data_dir, service.clone()));
    Ok(service)
}

fn dispatch_data_home_result<W: Write>(
    writer: &Arc<StdioRpcWriter<W>>,
    lane: StdioRpcFrameLane,
    id: &str,
    workflow_id: &str,
    operation: impl FnOnce() -> anyhow::Result<Value>,
) -> io::Result<()> {
    let execution = catch_unwind(AssertUnwindSafe(operation));
    let target = writer.lane_target(lane);
    match execution {
        Ok(Ok(value)) => write_stdio_rpc_success_shared(target, id, workflow_id, value),
        Ok(Err(error)) => write_stdio_rpc_error_shared(
            target,
            Some(id),
            Some(workflow_id),
            data_home_rpc_error_code(&error),
        ),
        Err(_) => write_stdio_rpc_error_shared(
            target,
            Some(id),
            Some(workflow_id),
            "data_home_operation_failed",
        ),
    }
}

fn data_home_rpc_error_code(error: &anyhow::Error) -> &'static str {
    const SAFE_CODES: &[&str] = &[
        "data_home_confirmation_required",
        "data_home_copy_failed",
        "data_home_copy_unsupported_entry",
        "data_home_destination_exists",
        "data_home_destination_invalid",
        "data_home_destination_nested",
        "data_home_destination_unavailable",
        "data_home_environment_selected",
        "data_home_previous_root_cleanup_failed",
        "data_home_previous_root_marker_cleanup_failed",
        "data_home_previous_root_invalid",
        "data_home_previous_root_unavailable",
        "data_home_recovery_autostart_failed",
        "data_home_recovery_cleanup_failed",
        "data_home_recovery_not_available",
        "data_home_recovery_not_required",
        "data_home_relocation_recovery_required",
        "data_home_selection_changed",
        "data_home_source_unavailable",
    ];
    let candidate = error.to_string();
    SAFE_CODES
        .iter()
        .copied()
        .find(|code| *code == candidate)
        .unwrap_or("data_home_operation_failed")
}

/// Attach every host-only execution port to one service instance. The
/// pre-opened host service and lazily opened portable-root services must be
/// indistinguishable; otherwise requests without an explicit portable root
/// can persist a message but fail before reaching the selected Agent.
pub(crate) fn bind_conversation_runtime(
    service: ConversationService,
    runtime: &PersistentConversationRuntime,
    portable_data_dir: Option<PathBuf>,
) -> ConversationService {
    let send_runtime = runtime.clone();
    let send_dir = portable_data_dir.clone();
    let active_runtime = runtime.clone();
    let steer_runtime = runtime.clone();
    let cancel_runtime = runtime.clone();
    let inspect_runtime = runtime.clone();
    let complete_runtime = runtime.clone();
    let complete_dir = portable_data_dir.clone();
    let actor_runtime = runtime.clone();
    let actor_dir = portable_data_dir;
    let strategy_root = service
        .store()
        .db_path()
        .parent()
        .expect("Conversation database always has a parent")
        .to_path_buf();
    let bound = service.bind_conversation_runtime(
        licoup_native::domain::client_conversation::PersistentRuntimePorts::new(
            move |params| send_runtime.start_admitted_background(params, send_dir.clone()),
            move |conversation_id| {
                active_runtime.active(&json!({ "conversationId": conversation_id }))
            },
            move |params| steer_runtime.steer_sync(params),
            move |params| {
                let handle = complete_runtime.open_admitted_turn(params)?;
                complete_runtime.run_open_turn(&handle, params, complete_dir.clone())
            },
            move |request| {
                let port =
                    conversation::strategy_turn_port(actor_runtime.clone(), actor_dir.clone());
                licoup_native::domain::workflow_runtime::StrategyService::open(&strategy_root)?
                    .with_actor_turn_port(port)
                    .with_assistant_wake_port(conversation::assistant_wake_port(
                        actor_runtime.clone(),
                        actor_dir.clone(),
                    ))
                    .execute(request)
            },
        )
        .with_cancel(move |params| {
            cancel_runtime
                .request_cancel(params)
                .map_err(|_| RuntimeAdapterError::ConversationDispatchFailed)
        }),
    );
    if let Some(host) = bound.continuity().cloned() {
        let observer_host = host.clone();
        runtime.set_live_turn_observer(
            move |conversation_id, membership_id, dispatch_id, native_turn_id| {
                observer_host.update_live_native_turn(
                    conversation_id,
                    membership_id,
                    dispatch_id,
                    native_turn_id,
                );
            },
        );
        host.bind_work_turn_inspect(std::sync::Arc::new(move |handle| {
            inspect_runtime.inspect_turn(handle)
        }));
    }
    bound
}
