//! The native command surface for the composed extension runtime.
//!
//! One route, one operation: run a single `agent-execution` call through the
//! production host. The command owns nothing the platform composition does not
//! already own — it resolves the caller's managed root, opens
//! [`ExtensionRuntime`] over it, and reports what the host's own lifecycle
//! produced. Nothing here decides a package's facts, a generation, an admission
//! or a settlement.

use super::{AdmittedCommand, CliExecution};
use anyhow::{Result, anyhow};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::time::Duration;

use licoup_application::{ApplicationFailure, EffectCertainty};

use crate::platform::extension_host::isolation::IsolationMode;
use crate::platform::extension_host::{AgentExecutionCall, ExtensionRuntime};

/// The report one `extension-host serve` call publishes.
const SERVE_SCHEMA: &str = "licoup.extension-host.serve.v1";

/// The refusal envelope this surface publishes.
const ERROR_SCHEMA: &str = "licoup.extension-host.error.v1";

/// How long a call waits for settlement when the caller names no bound.
const DEFAULT_WAIT_MS: u64 = 30_000;

/// The longest a caller may wait for one call to settle.
const MAX_WAIT_MS: u64 = 10 * 60 * 1_000;

/// Run one `agent-execution` call through the host composed over a managed root.
pub(super) fn handle_serve(command: AdmittedCommand) -> Result<CliExecution> {
    let root = PathBuf::from(command.required_text("data-root"));
    let package_id = command.required_text("package-id").to_owned();
    let version = command.required_text("version").to_owned();
    let mode = isolation_mode(command.option_text("mode"))?;
    let wait = millis_option(command.option_text("await-ms"), "await-ms")?
        .map(Duration::from_millis)
        .unwrap_or(Duration::from_millis(DEFAULT_WAIT_MS));
    let cancel_after = millis_option(command.option_text("cancel-after-ms"), "cancel-after-ms")?
        .map(Duration::from_millis);
    let request = command
        .option_json("request")
        .cloned()
        .unwrap_or(Value::Null);
    let runtime = match ExtensionRuntime::open(&root, mode) {
        Ok(runtime) => runtime,
        Err(failure) => return Ok(CliExecution::Json(failure_envelope(&failure))),
    };
    let mut call = AgentExecutionCall::new(request).with_wait(wait);
    if let Some(capability) = command.option_text("capability") {
        call = call.with_capability(capability);
    }
    if let Some(cancel_after) = cancel_after {
        call = call.with_cancel_after(cancel_after);
    }
    match runtime.serve_agent_execution(&package_id, &version, &call) {
        Ok(mut report) => {
            if let Some(object) = report.as_object_mut() {
                object.insert("schemaVersion".to_owned(), json!(SERVE_SCHEMA));
                object.insert("isError".to_owned(), json!(false));
            }
            Ok(CliExecution::Json(report))
        }
        Err(failure) => Ok(CliExecution::Json(failure_envelope(&failure))),
    }
}

/// The envelope this surface publishes for a refusal.
///
/// The whole neutral failure travels: its code, its stage, whether a retry is
/// meaningful, the recovery action this interface projects, and whether an effect
/// may already have happened. A caller that only saw the code would have to
/// guess the last two. The recovery is the CLI projection, so "install or retry
/// the runtime" stays distinguishable from an ordinary retry.
fn failure_envelope(failure: &ApplicationFailure) -> Value {
    json!({
        "schemaVersion": ERROR_SCHEMA,
        "reasonCode": &failure.code,
        "stage": &failure.stage,
        "retryable": failure.retryable,
        "recovery": failure.recovery.cli_wire(),
        "effect": effect_name(failure.effect),
        "component": failure.component.as_ref(),
        "isError": true,
    })
}

const fn effect_name(effect: EffectCertainty) -> &'static str {
    match effect {
        EffectCertainty::NotAttempted => "not-attempted",
        EffectCertainty::Uncertain => "uncertain",
        EffectCertainty::Applied => "applied",
    }
}

/// The isolation mode a caller selected, with the least privileged default.
fn isolation_mode(raw: Option<&str>) -> Result<IsolationMode> {
    match raw {
        None | Some("restricted") => Ok(IsolationMode::Restricted),
        Some("trusted-local") => Ok(IsolationMode::TrustedLocal),
        Some(_) => Err(anyhow!("extension_isolation_mode_unknown")),
    }
}

/// One bounded millisecond option.
fn millis_option(raw: Option<&str>, name: &str) -> Result<Option<u64>> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let value: u64 = raw
        .parse::<u64>()
        .map_err(|_| anyhow!("extension_host_{name}_invalid"))?;
    if value > MAX_WAIT_MS {
        return Err(anyhow!("extension_host_{name}_out_of_range"));
    }
    Ok(Some(value))
}

#[cfg(test)]
mod tests {
    use super::super::{CliExecution, execute_cli};
    use serde_json::Value;
    use std::path::Path;

    /// An unknown isolation mode is refused before any root is opened, so a
    /// typo cannot silently become the unconfined run.
    #[test]
    fn an_unknown_isolation_mode_is_refused() {
        assert_eq!(
            super::isolation_mode(Some("unconfined"))
                .expect_err("an unknown mode is not a mode")
                .to_string(),
            "extension_isolation_mode_unknown"
        );
        assert_eq!(
            super::isolation_mode(None).expect("the default is restricted"),
            crate::platform::extension_host::isolation::IsolationMode::Restricted
        );
    }

    /// The bounded wait options are bounds, not suggestions.
    #[test]
    fn a_wait_option_beyond_the_bound_is_refused() {
        assert!(super::millis_option(Some("0"), "await-ms").is_ok());
        assert_eq!(
            super::millis_option(Some("not-a-number"), "await-ms")
                .expect_err("a non-numeric bound is refused")
                .to_string(),
            "extension_host_await-ms_invalid"
        );
        assert!(super::millis_option(Some("600001"), "await-ms").is_err());
    }

    /// A package that was never installed is refused by the store's own record
    /// before anything is derived from the request.
    #[test]
    fn a_package_that_is_not_installed_is_refused_without_starting_anything() {
        let root =
            std::env::temp_dir().join(format!("licoup-cli-extension-host-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let root_text = root.display().to_string();
        let execution = execute_cli(vec![
            "extension-host".to_owned(),
            "serve".to_owned(),
            root_text.clone(),
            "example.specialist.absent".to_owned(),
            "1.0.0".to_owned(),
        ])
        .expect("the route is admitted");
        let CliExecution::Json(report) = execution else {
            panic!("serve publishes a JSON report");
        };
        assert_eq!(report["isError"], Value::Bool(true));
        assert_eq!(report["reasonCode"], "extension_program_unavailable");
        assert_eq!(report["component"], "extension_host");
        assert_eq!(report["recovery"], "install_or_retry_runtime");
        assert!(Path::new(&root_text).exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}
