use serde_json::json;

use crate::ffi::generated::client_state::{
    ClientStateCollection, ClientStateDocument, ClientStateGetRequest, ClientStateSetRequest,
};

use super::support::{PortableDataDirOverrideGuard, TestRoot};

#[test]
fn state_operations_close_over_only_the_requested_collection_and_activity() {
    let root = TestRoot::new("operations");
    let _override = PortableDataDirOverrideGuard::set(root.path().join("portable"));

    let collection = ClientStateCollection::Settings;
    let document =
        ClientStateDocument::for_collection(collection, json!({"items": [{"key": "theme"}]}))
            .unwrap();
    let saved = super::super::state_set(ClientStateSetRequest {
        collection,
        document,
    })
    .unwrap();
    let read = super::super::state_get(ClientStateGetRequest { collection }).unwrap();
    let activity = super::super::activity_list(&json!({
        "type": "state.collection.saved",
        "limit": 1
    }))
    .unwrap();

    assert_eq!(saved.document.content["items"][0]["key"], "theme");
    assert_eq!(read.document.content["items"][0]["key"], "theme");
    assert_eq!(activity["events"].as_array().unwrap().len(), 1);
    assert_eq!(activity["events"][0]["target"], "settings");
}
