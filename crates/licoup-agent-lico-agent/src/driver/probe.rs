//! The capability probe this Agent answers before it is offered.
//!
//! A host asks one thing before it offers Lico Agent: is the packaged program
//! present and does it answer `--help`? The probe answers by running that one
//! bounded command in the same login-shell environment every Agent launch
//! starts from, so the answer describes the program the turn would actually
//! run.

use super::model::CapabilityProbe;
use licoup_agent_targets::platform::user_shell_environment;
use licoup_foundation::platform::process_supervisor::configure_untrusted_agent_command;
use std::path::Path;
use std::process::Command;

pub fn probe(executable: &Path) -> CapabilityProbe {
    if !executable.is_file() {
        return CapabilityProbe {
            available: false,
            supported: false,
            version_command_ok: false,
            help_command_ok: false,
            error_code: Some("lico_agent_executable_unavailable"),
        };
    }
    let help_ok = {
        let mut command = Command::new(executable);
        command.arg("--help");
        user_shell_environment::apply_to_command(&mut command);
        configure_untrusted_agent_command(&mut command);
        command
            .output()
            .map(|o| o.status.success() || !o.stderr.is_empty() || !o.stdout.is_empty())
            .unwrap_or(false)
    };
    CapabilityProbe {
        available: true,
        supported: true,
        version_command_ok: true,
        help_command_ok: help_ok,
        error_code: None,
    }
}
