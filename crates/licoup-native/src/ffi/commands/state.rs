use super::{AdmittedCommand, CliExecution, admitted_params};
use crate::ffi::generated::client_state::{
    ClientStateCollection, ClientStateDocument, ClientStateGetRequest, ClientStateSetRequest,
};
use anyhow::Result;
use std::path::Path;

// Public CLI adapter: user-facing state commands immediately construct the
// generated DTOs and do not define a second state contract.
pub(super) fn handle_state_get(command: AdmittedCommand) -> Result<CliExecution> {
    let collection = serde_json::from_value::<ClientStateCollection>(serde_json::json!(
        command.required_text("collection")
    ))?;
    let result = crate::platform::client_state::state_get(ClientStateGetRequest { collection })?;
    Ok(CliExecution::Json(serde_json::to_value(result)?))
}

pub(super) fn handle_state_set(command: AdmittedCommand) -> Result<CliExecution> {
    let collection = serde_json::from_value::<ClientStateCollection>(serde_json::json!(
        command.required_text("collection")
    ))?;
    let document =
        ClientStateDocument::for_collection(collection, command.required_json("payload").clone())
            .map_err(anyhow::Error::msg)?;
    let result = crate::platform::client_state::state_set(ClientStateSetRequest {
        collection,
        document,
    })?;
    Ok(CliExecution::Json(serde_json::to_value(result)?))
}

pub(super) fn handle_state_admit(command: AdmittedCommand) -> Result<CliExecution> {
    let data_root = command.required_text("data-root");
    Ok(CliExecution::Json(
        crate::domain::client_state_migration::admission_json(Path::new(data_root))?,
    ))
}

pub(super) fn handle_activity_list(command: AdmittedCommand) -> Result<CliExecution> {
    let params = admitted_params(
        &[
            ("type", command.option_text("type")),
            ("target", command.option_text("target")),
            ("limit", command.option_text("limit")),
        ],
        &[],
        &[],
    );
    Ok(CliExecution::Json(
        crate::platform::client_state::activity_list(&params)?,
    ))
}
