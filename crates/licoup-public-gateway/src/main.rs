mod connector;
use std::process::ExitCode;
fn main() -> ExitCode {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    // A data-home coordinator closes admission before asking the existing MCP
    // service to stop. This stop-only child bypasses the shared lease so it
    // can deliver control and prove drain while every ordinary MCP process is
    // correctly refused by the active barrier.
    let data_home_transition_stop =
        args.as_slice() == ["service", "stop", "--data-home-transition"];
    let _data_home_access = if data_home_transition_stop {
        None
    } else {
        match licoup_foundation::platform::data_home_access::acquire_process_data_home_access() {
            Ok(lease) => Some(lease),
            Err(_) => {
                eprintln!("lico-subagent-mcp: data root is unavailable");
                return ExitCode::FAILURE;
            }
        }
    };
    let service_action = match args.as_slice() {
        [service, action] if service == "service" => Some(action.as_str()),
        [service, action, transition]
            if service == "service"
                && action == "stop"
                && transition == "--data-home-transition" =>
        {
            Some("stop")
        }
        _ => None,
    };
    if let Some(action) = service_action {
        return match licoup_public_gateway::lifecycle::execute(action) {
            Ok(result) => {
                println!("{result}");
                ExitCode::SUCCESS
            }
            Err(_) => {
                eprintln!("lico-subagent-mcp: service operation failed");
                ExitCode::FAILURE
            }
        };
    }
    connector::main()
}
