use super::authority::key_transparency_configure_authority;
use super::gossip::key_transparency_gossip;
use super::provision::key_transparency_provision;
use super::publication::key_transparency_publication_request;
use super::revocation::key_transparency_revocation_request;
use super::self_monitor::key_transparency_self_monitor;
use super::status::key_transparency_status;
use anyhow::{Result, anyhow};
use serde_json::Value;

pub fn dispatch_key_transparency_action(action: &str, params: &Value) -> Result<Value> {
    match action {
        "secure_mesh.kt.configureAuthority" => key_transparency_configure_authority(params),
        "secure_mesh.kt.publicationRequest" => key_transparency_publication_request(params),
        "secure_mesh.kt.revocationRequest" => key_transparency_revocation_request(params),
        "secure_mesh.kt.provision" => key_transparency_provision(params),
        "secure_mesh.kt.gossip" => key_transparency_gossip(params),
        "secure_mesh.kt.selfMonitor" => key_transparency_self_monitor(params),
        "secure_mesh.kt.status" => key_transparency_status(params),
        _ => Err(anyhow!("secure mesh KT native action is unsupported")),
    }
}
