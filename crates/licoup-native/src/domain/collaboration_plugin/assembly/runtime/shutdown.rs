use anyhow::{Result, anyhow, ensure};
use serde_json::json;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use super::super::model::LocalAssemblyRecord;

pub(super) fn request(record: &LocalAssemblyRecord) -> Result<()> {
    let runtime_instance_id = record
        .runtime_instance_id
        .as_deref()
        .ok_or_else(|| anyhow!("collaboration_local_server_runtime_instance_missing"))?;
    let body = serde_json::to_vec(&json!({
        "schemaVersion": "licoup.local-server-shutdown.v1",
        "deploymentId": record.deployment_id,
        "runtimeInstanceId": runtime_instance_id,
        "assemblyManifestDigestSha256": record.manifest_digest_sha256,
        "runtimeGeneration": record.runtime_generation
    }))?;
    let address = SocketAddr::from(([127, 0, 0, 1], record.port));
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_millis(500))
        .map_err(|_| anyhow!("collaboration_local_server_controlled_shutdown_unavailable"))?;
    stream.set_read_timeout(Some(Duration::from_millis(800)))?;
    stream.set_write_timeout(Some(Duration::from_millis(800)))?;
    let headers = format!(
        "POST /v1/shutdown HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream
        .write_all(headers.as_bytes())
        .and_then(|_| stream.write_all(&body))
        .map_err(|_| anyhow!("collaboration_local_server_controlled_shutdown_unavailable"))?;
    let mut response = Vec::new();
    stream
        .take(16 * 1024 + 1)
        .read_to_end(&mut response)
        .map_err(|_| anyhow!("collaboration_local_server_controlled_shutdown_unavailable"))?;
    ensure!(
        response.len() <= 16 * 1024
            && (response.starts_with(b"HTTP/1.1 200 OK\r\n")
                || response.starts_with(b"HTTP/1.1 202 Accepted\r\n")),
        "collaboration_local_server_controlled_shutdown_rejected"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::request;
    use crate::domain::collaboration_plugin::assembly::model::LocalServerLifecycle;
    use crate::domain::collaboration_plugin::assembly::tests::synthetic_record;
    use serde_json::Value;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;
    use uuid::Uuid;

    #[test]
    fn controlled_shutdown_binds_exact_runtime_generation_and_identity() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let mut record = synthetic_record(
            &std::env::temp_dir().join(Uuid::new_v4().to_string()),
            LocalServerLifecycle::Running,
        );
        record.port = port;
        let expected_deployment = record.deployment_id.clone();
        let expected_runtime = record.runtime_instance_id.clone().unwrap();
        let expected_manifest = record.manifest_digest_sha256.clone();
        let expected_generation = record.runtime_generation;
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request_bytes = Vec::new();
            let mut buffer = [0_u8; 1024];
            let header_end = loop {
                let read = stream.read(&mut buffer).unwrap();
                assert!(read > 0);
                request_bytes.extend_from_slice(&buffer[..read]);
                if let Some(index) = request_bytes
                    .windows(4)
                    .position(|value| value == b"\r\n\r\n")
                {
                    break index + 4;
                }
            };
            let headers = std::str::from_utf8(&request_bytes[..header_end]).unwrap();
            assert!(headers.starts_with("POST /v1/shutdown HTTP/1.1\r\n"));
            let content_length = headers
                .lines()
                .find_map(|line| line.strip_prefix("Content-Length: "))
                .unwrap()
                .parse::<usize>()
                .unwrap();
            while request_bytes.len() < header_end + content_length {
                let read = stream.read(&mut buffer).unwrap();
                assert!(read > 0);
                request_bytes.extend_from_slice(&buffer[..read]);
            }
            let body: Value =
                serde_json::from_slice(&request_bytes[header_end..header_end + content_length])
                    .unwrap();
            assert_eq!(body["schemaVersion"], "licoup.local-server-shutdown.v1");
            assert_eq!(body["deploymentId"], expected_deployment);
            assert_eq!(body["runtimeInstanceId"], expected_runtime);
            assert_eq!(body["assemblyManifestDigestSha256"], expected_manifest);
            assert_eq!(body["runtimeGeneration"], expected_generation);
            stream
                .write_all(
                    b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .unwrap();
        });

        request(&record).unwrap();
        server.join().unwrap();
    }
}
