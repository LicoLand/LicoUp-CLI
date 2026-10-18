use serde_json::Value;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use super::super::model::LocalAssemblyRecord;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ProbeIdentity {
    Owned,
    Mismatched,
    Unavailable,
}

pub(super) fn endpoint_identity(record: &LocalAssemblyRecord) -> ProbeIdentity {
    let Some(expected_pid) = record.runtime_pid else {
        return ProbeIdentity::Mismatched;
    };
    let Some(runtime_instance_id) = record.runtime_instance_id.as_deref() else {
        return ProbeIdentity::Mismatched;
    };
    let health = match get_json(record.port, "/health") {
        Fetch::Unavailable => return ProbeIdentity::Unavailable,
        Fetch::Invalid => return ProbeIdentity::Mismatched,
        Fetch::Json(value) => value,
    };
    if !health_matches(&health, record, expected_pid, runtime_instance_id) {
        return ProbeIdentity::Mismatched;
    }
    let capabilities = match get_json(record.port, "/v1/capabilities") {
        Fetch::Unavailable => return ProbeIdentity::Unavailable,
        Fetch::Invalid => return ProbeIdentity::Mismatched,
        Fetch::Json(value) => value,
    };
    if capabilities_match(&capabilities, record, expected_pid, runtime_instance_id) {
        ProbeIdentity::Owned
    } else {
        ProbeIdentity::Mismatched
    }
}

fn health_matches(
    body: &Value,
    record: &LocalAssemblyRecord,
    expected_pid: u32,
    runtime_instance_id: &str,
) -> bool {
    body.get("schemaVersion").and_then(Value::as_str)
        == Some(record.health_contract_version.as_str())
        && body.get("ok").and_then(Value::as_bool) == Some(true)
        && body.get("status").and_then(Value::as_str) == Some("running")
        && body.get("deploymentId").and_then(Value::as_str) == Some(record.deployment_id.as_str())
        && body
            .get("assemblyManifestDigestSha256")
            .and_then(Value::as_str)
            == Some(record.manifest_digest_sha256.as_str())
        && body.get("serverVersion").and_then(Value::as_str) == Some(record.server_version.as_str())
        && body.get("runtimePid").and_then(Value::as_u64) == Some(u64::from(expected_pid))
        && body.get("runtimeInstanceId").and_then(Value::as_str) == Some(runtime_instance_id)
        && body.get("runnerContractVersion").and_then(Value::as_str)
            == Some(record.runner_contract_version.as_str())
}

fn capabilities_match(
    body: &Value,
    record: &LocalAssemblyRecord,
    expected_pid: u32,
    runtime_instance_id: &str,
) -> bool {
    body.get("schemaVersion").and_then(Value::as_str)
        == Some(record.capabilities_contract_version.as_str())
        && body.get("deploymentId").and_then(Value::as_str) == Some(record.deployment_id.as_str())
        && body
            .get("assemblyManifestDigestSha256")
            .and_then(Value::as_str)
            == Some(record.manifest_digest_sha256.as_str())
        && body.get("serverVersion").and_then(Value::as_str) == Some(record.server_version.as_str())
        && body.get("runtimePid").and_then(Value::as_u64) == Some(u64::from(expected_pid))
        && body.get("runtimeInstanceId").and_then(Value::as_str) == Some(runtime_instance_id)
        && body.get("runnerContractVersion").and_then(Value::as_str)
            == Some(record.runner_contract_version.as_str())
        && body.get("healthContractVersion").and_then(Value::as_str)
            == Some(record.health_contract_version.as_str())
        && body
            .get("capabilitiesContractVersion")
            .and_then(Value::as_str)
            == Some(record.capabilities_contract_version.as_str())
        && body.get("selectedComponentIds")
            == Some(&serde_json::json!(record.selected_component_ids))
        && body
            .get("selectedPayloadInventoryDigestSha256")
            .and_then(Value::as_str)
            == Some(record.selected_payload_inventory_digest_sha256.as_str())
}

enum Fetch {
    Json(Value),
    Invalid,
    Unavailable,
}

fn get_json(port: u16, path: &str) -> Fetch {
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    let Ok(mut stream) = TcpStream::connect_timeout(&address, Duration::from_millis(250)) else {
        return Fetch::Unavailable;
    };
    if stream
        .set_read_timeout(Some(Duration::from_millis(350)))
        .is_err()
        || stream
            .set_write_timeout(Some(Duration::from_millis(350)))
            .is_err()
    {
        return Fetch::Unavailable;
    }
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nAccept: application/json\r\nConnection: close\r\n\r\n"
    );
    if stream.write_all(request.as_bytes()).is_err() {
        return Fetch::Unavailable;
    }
    let mut response = Vec::new();
    if stream
        .take(64 * 1024 + 1)
        .read_to_end(&mut response)
        .is_err()
    {
        return Fetch::Unavailable;
    }
    if response.len() > 64 * 1024 || !response.starts_with(b"HTTP/1.1 200 OK\r\n") {
        return Fetch::Invalid;
    }
    let Some(body_offset) = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|position| position + 4)
    else {
        return Fetch::Invalid;
    };
    let Ok(headers) = std::str::from_utf8(&response[..body_offset]) else {
        return Fetch::Invalid;
    };
    let content_type_valid = headers.lines().any(|line| {
        line.split_once(':').is_some_and(|(name, value)| {
            name.eq_ignore_ascii_case("content-type")
                && value.trim().eq_ignore_ascii_case("application/json")
        })
    });
    let content_length = headers.lines().find_map(|line| {
        line.split_once(':').and_then(|(name, value)| {
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
    });
    let body = &response[body_offset..];
    if !content_type_valid || content_length != Some(body.len()) {
        return Fetch::Invalid;
    }
    serde_json::from_slice(body)
        .map(Fetch::Json)
        .unwrap_or(Fetch::Invalid)
}
