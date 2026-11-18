use anyhow::Result;
use serde_json::{Value, json};

use crate::ffi::generated::client_state::{
    ClientStateActivity, ClientStateDocument, ClientStateGetRequest, ClientStateGetResult,
    ClientStateSetRequest, ClientStateSetResult,
};

use super::collections::ClientStateStore;
use licoup_client_state::ActivityLog;
use licoup_client_state::SnapshotStore;

pub fn state_get(request: ClientStateGetRequest) -> Result<ClientStateGetResult> {
    let store = ClientStateStore::portable()?;
    let collection = request.collection;
    let document = ClientStateDocument::from_value(store.read_collection(collection.as_str())?)
        .map_err(anyhow::Error::msg)?;
    anyhow::ensure!(
        document.collection == collection,
        "client state document collection mismatch"
    );
    Ok(ClientStateGetResult {
        ok: true,
        collection,
        document,
    })
}

pub fn state_set(request: ClientStateSetRequest) -> Result<ClientStateSetResult> {
    let store = ClientStateStore::portable()?;
    let collection = request.collection;
    let document = ClientStateDocument::from_value(
        store.write_collection(collection.as_str(), request.document.into_value())?,
    )
    .map_err(anyhow::Error::msg)?;
    let activity = serde_json::from_value::<ClientStateActivity>(store.activity_log().append(
        "state.collection.saved",
        json!({
            "collection": collection.as_str(),
            "target": collection.as_str()
        }),
    )?)?;
    Ok(ClientStateSetResult {
        ok: true,
        collection,
        document,
        activity,
    })
}

pub fn activity_list(params: &Value) -> Result<Value> {
    ActivityLog::portable()?.list(params)
}

pub fn snapshots_list(params: &Value) -> Result<Value> {
    SnapshotStore::portable()?.list(params)
}

pub fn snapshots_restore(snapshot_id: &str) -> Result<Value> {
    let store = ClientStateStore::portable()?;
    let result = store.snapshot_store().restore(snapshot_id)?;
    let activity = store.activity_log().append(
        "snapshot.restored",
        json!({
            "target": "snapshot",
            "snapshotId": snapshot_id
        }),
    )?;
    Ok(json!({
        "ok": true,
        "restore": result,
        "activity": activity
    }))
}
