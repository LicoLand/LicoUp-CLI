use super::{AdmittedCommand, CliExecution};
use anyhow::{Result, anyhow};
use serde_json::Value;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use crate::domain::client_conversation::ConversationService;
use licoup_foundation::platform::paths::portable_data_dir;

const MAX_CONVERSATION_SERVICE_ROOTS: usize = 4;
static CONVERSATION_SERVICES: OnceLock<Mutex<VecDeque<(PathBuf, ConversationService)>>> =
    OnceLock::new();

pub(super) fn handle_conversation_execute(mut command: AdmittedCommand) -> Result<CliExecution> {
    let require_running_host = command.option_flag("require-running-host");
    let input = match command.take_option_json("stdin-json") {
        Some(Value::Object(input)) => Value::Object(input),
        Some(_) => return Err(anyhow!("conversation_request_invalid")),
        None => return Err(anyhow!("conversation_request_required")),
    };
    if require_running_host {
        return Ok(CliExecution::Json(
            crate::platform::conversation_host_client::execute_existing(
                "client.conversation.execute",
                &input,
            )?,
        ));
    }
    // Dispatch-type work needs the persistent host runtime. A one-shot
    // process would open a turn no observer can attach, so it fails closed
    // with the typed transport rejection and performs no Agent work.
    if input.get("action").and_then(Value::as_str) == Some("conversation.dispatch.after-post") {
        return Err(anyhow!(
            crate::domain::client_conversation::PERSISTENT_TRANSPORT_REQUIRED
        ));
    }
    let root = portable_data_dir()?;
    let service = conversation_service(&root)?;
    Ok(CliExecution::Json(serde_json::json!({
        "ok": true,
        "result": service.execute(input)?,
    })))
}

#[cfg(test)]
mod tests {
    use super::super::execute_cli;
    use licoup_foundation::platform::paths::set_portable_data_dir_override;

    #[test]
    fn required_host_mode_never_opens_a_local_conversation_store() {
        let root = std::env::temp_dir().join(format!(
            "licoup-required-conversation-host-{}",
            uuid::Uuid::new_v4()
        ));
        let previous = set_portable_data_dir_override(Some(root.clone()));
        let error = execute_cli(vec![
            "conversation".into(),
            "execute".into(),
            "--require-running-host".into(),
            "--stdin-json".into(),
            r#"{"action":"conversation.list"}"#.into(),
        ])
        .expect_err("a missing persistent host must fail closed");
        set_portable_data_dir_override(previous);

        assert_eq!(
            error.to_string(),
            "persistent_conversation_transport_required"
        );
        assert!(!root.exists());
    }
}

fn conversation_service(root: &Path) -> Result<ConversationService> {
    let services = CONVERSATION_SERVICES.get_or_init(|| Mutex::new(VecDeque::new()));
    let mut services = services
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(position) = services.iter().position(|(candidate, _)| candidate == root) {
        let entry = services
            .remove(position)
            .expect("conversation service position exists");
        let service = entry.1.clone();
        services.push_back(entry);
        return Ok(service);
    }
    let service = ConversationService::open(root)?;
    if services.len() == MAX_CONVERSATION_SERVICE_ROOTS {
        services.pop_front();
    }
    services.push_back((root.to_owned(), service.clone()));
    Ok(service)
}
