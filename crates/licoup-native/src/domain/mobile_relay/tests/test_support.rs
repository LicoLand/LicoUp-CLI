pub(super) use super::super::{
    command_sync::*, config::*, endpoint_trust::*, key_transparency::*, pairing::*,
    pairwise_session::*, relay_operations::*, secret_custody::*, support::*,
};
pub(super) use crate::core::secure_mesh_capability::{
    CapabilityEvidenceKind, CapabilityFact, capability_catalog, mandatory_protocol_facts,
};
pub(super) use crate::platform::badtower_station::BadTowerStationOperation;
pub(super) use crate::platform::secure_mesh_secret_store::{EphemeralSecretStore, SecretBytes};
pub(super) use licoup_foundation::platform::paths::set_portable_data_dir_override;
pub(super) use std::env;
pub(super) use std::io::{BufRead, BufReader, Read, Write};
pub(super) use std::net::{TcpListener, TcpStream};
pub(super) use std::sync::{Arc, Mutex};
pub(super) use std::thread;
pub(super) use std::time::{Duration, Instant};

pub(super) fn secure_envelope_fixture() -> Value {
    let mailbox = licoup_protocol_bindings::licoarc_relay::SecureMeshMailboxToken::from_base64url(
        general_purpose::URL_SAFE_NO_PAD.encode([2u8; 32]),
    )
    .unwrap();
    let envelope = licoup_protocol_bindings::licoarc_relay::LicoArcRelayEnvelopeDraft::begin(
        &mailbox,
        "2099-01-01T00:10:00Z",
        licoup_protocol_bindings::MIN_PADDING_BUCKET_BYTES,
    )
    .and_then(|draft| {
        draft.finish(
            &[7u8; licoup_protocol_bindings::licoarc_relay::LICOARC_ENCRYPTED_HEADER_BYTES],
            &[9u8; licoup_protocol_bindings::MIN_PADDING_BUCKET_BYTES],
        )
    })
    .unwrap();
    serde_json::from_str(&envelope.to_json().unwrap()).unwrap()
}

pub(super) fn append_test_directory_state(
    descriptor: &mut Value,
    directory_state: &str,
) -> Result<()> {
    ensure!(
        matches!(directory_state, "active" | "revoked"),
        "test directory state is unsupported"
    );
    let response_value = descriptor
        .get("preKeyBundle")
        .and_then(|bundle| bundle.get("keyTransparency"))
        .cloned()
        .ok_or_else(|| anyhow!("test descriptor KT response is missing"))?;
    let mut response: UntrustedDirectoryResponse = serde_json::from_value(response_value)?;
    let previous_tree_size = response.inclusion.signed_tree_head.tree_size;
    response.claim.endpoint.directory_state = directory_state.to_string();
    response.claim.endpoint.updated_at = now_iso();
    response.claim.directory_version = response
        .claim
        .directory_version
        .checked_add(1)
        .ok_or_else(|| anyhow!("test directory version overflow"))?;
    let now_epoch_seconds = mobile_relay_trust_record_now_epoch()?;
    response = with_mobile_relay_test_kt_log(|log| {
        let index = log.append_hashed_directory_leaf(
            &response.claim.stable_label(),
            response.claim.version(),
            response.claim.revoked(),
            response.claim.leaf_hash()?,
        )?;
        Ok(UntrustedDirectoryResponse {
            claim: response.claim.clone(),
            inclusion: log.inclusion_proof_at(index, now_epoch_seconds)?,
            latest_map: log.map_proof_at(&response.claim.stable_label(), now_epoch_seconds)?,
            consistency: (previous_tree_size < log.tree_size())
                .then(|| log.consistency_proof_at(previous_tree_size, now_epoch_seconds))
                .transpose()?,
        })
    })?;
    descriptor["preKeyBundle"]["keyTransparency"] = serde_json::to_value(response)?;
    Ok(())
}

pub(super) fn pair_mobile_relay_configs(pc_config: &mut Value, mobile_config: &mut Value) {
    let shared_delivery_secret = random_base64url(MOBILE_RELAY_KEY_BYTES);
    test_runtime_secret_material(stringify!(pc_config))
        .replace_e2ee_secret(
            MobileRelayE2eeSecretField::PairingSecret,
            SecretBytes::try_from_string(shared_delivery_secret.clone()).unwrap(),
        )
        .unwrap();
    test_runtime_secret_material(stringify!(mobile_config))
        .replace_e2ee_secret(
            MobileRelayE2eeSecretField::PairingSecret,
            SecretBytes::try_from_string(shared_delivery_secret).unwrap(),
        )
        .unwrap();
    let pc_descriptor = ensure_mobile_relay_endpoint_descriptor(
        pc_config,
        &mut test_runtime_secret_material(stringify!(pc_config)),
        "desktop_sidecar",
    )
    .unwrap();
    ensure_mobile_relay_endpoint_descriptor(
        mobile_config,
        &mut test_runtime_secret_material(stringify!(mobile_config)),
        "mobile",
    )
    .unwrap();
    apply_peer_secure_mesh_descriptor(
        mobile_config,
        &mut test_runtime_secret_material(stringify!(mobile_config)),
        &pc_descriptor,
        true,
    )
    .unwrap();
    let mobile_descriptor = ensure_mobile_relay_endpoint_descriptor(
        mobile_config,
        &mut test_runtime_secret_material(stringify!(mobile_config)),
        "mobile",
    )
    .unwrap();
    apply_peer_secure_mesh_descriptor(
        pc_config,
        &mut test_runtime_secret_material(stringify!(pc_config)),
        &mobile_descriptor,
        true,
    )
    .unwrap();
    let pc_accepted_descriptor = ensure_mobile_relay_endpoint_descriptor(
        pc_config,
        &mut test_runtime_secret_material(stringify!(pc_config)),
        "desktop_sidecar",
    )
    .unwrap();
    apply_peer_secure_mesh_descriptor(
        mobile_config,
        &mut test_runtime_secret_material(stringify!(mobile_config)),
        &pc_accepted_descriptor,
        true,
    )
    .unwrap();
    let mobile_finished_descriptor = ensure_mobile_relay_endpoint_descriptor(
        mobile_config,
        &mut test_runtime_secret_material(stringify!(mobile_config)),
        "mobile",
    )
    .unwrap();
    assert!(mobile_finished_descriptor["pairwiseFinished"].is_object());
    apply_peer_secure_mesh_descriptor(
        pc_config,
        &mut test_runtime_secret_material(stringify!(pc_config)),
        &mobile_finished_descriptor,
        true,
    )
    .unwrap();
    let protected_payload = seal_mobile_relay_payload(
        mobile_config,
        &mut test_runtime_secret_material(stringify!(mobile_config)),
        crate::core::secure_mesh_crypto::SecureMeshPayloadKind::ServiceAction,
        &json!({"action": "pairwise_finished_confirmed"}),
    )
    .unwrap();
    let opened = open_mobile_relay_payload(
        pc_config,
        &mut test_runtime_secret_material(stringify!(pc_config)),
        &protected_payload,
        crate::core::secure_mesh_crypto::SecureMeshPayloadKind::ServiceAction,
    )
    .unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&opened).unwrap()["action"],
        "pairwise_finished_confirmed"
    );
}

pub(super) fn test_runtime_e2ee_secret(
    variable: &str,
    field: MobileRelayE2eeSecretField,
) -> String {
    let material = test_runtime_secret_material(variable);
    let bytes = material
        .e2ee_secret(field)
        .unwrap_or_else(|| {
            panic!(
                "test runtime secret material is missing {}",
                field.config_field()
            )
        })
        .expose_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

pub(super) fn persist_test_runtime_secret_material(variable: &str) -> Result<()> {
    let mut material = test_runtime_secret_material(variable);
    let mut batch = MobileRelaySecretStoreAuthBatch::default();
    persist_runtime_secret_material_to_native_store_with_batch(&mut material, &mut batch)
}

pub(super) fn take_test_runtime_secret_context(variable: &str) -> RuntimeSecretContext {
    let mut context = RuntimeSecretContext::default();
    if let Some(bundle) = test_runtime_secret_material(variable).take_e2ee_bundle() {
        context.material.merge_e2ee_bundle(bundle);
    }
    context
}

pub(super) fn restore_test_runtime_secret_context(
    variable: &str,
    mut context: RuntimeSecretContext,
) {
    if let Some(bundle) = context.material.take_e2ee_bundle() {
        test_runtime_secret_material(variable).merge_e2ee_bundle(bundle);
    }
}

pub(super) fn apply_test_out_of_band_pairing_response(
    config: &mut Value,
    variable: &str,
    response: &Value,
) -> Result<()> {
    let mut context = take_test_runtime_secret_context(variable);
    let result =
        apply_out_of_band_pairing_response_with_context(config, response, Some(&mut context));
    restore_test_runtime_secret_context(variable, context);
    result
}

pub(super) fn save_test_config_with_runtime_secret_context(
    config: &mut Value,
    variable: &str,
) -> Result<()> {
    let mut context = take_test_runtime_secret_context(variable);
    let result = save_config_with_runtime_secret_context(config, &mut context);
    restore_test_runtime_secret_context(variable, context);
    result
}

pub(super) fn paired_command_envelope_fixture() -> (Value, Value, Value) {
    let mut pc_config = default_config();
    let mut mobile_config = default_config();
    pair_mobile_relay_configs(&mut pc_config, &mut mobile_config);
    let mobile_material = test_runtime_secret_material(stringify!(&mobile_config));
    let pc_material = test_runtime_secret_material(stringify!(&pc_config));
    let mobile_endpoint = local_endpoint_state(&mobile_config, &mobile_material).unwrap();
    let pc_endpoint = local_endpoint_state(&pc_config, &pc_material).unwrap();
    let command_payload = json!({
        "schema": crate::core::secure_mesh::SECURE_MESH_COMMAND_PROTOCOL_VERSION,
        "commandId": "cmd_mobile_relay_replay_fixture",
        "commandKind": "agent.sessions.list",
        "senderIdentity": {
            "endpointId": mobile_endpoint.endpoint_id,
            "identityFingerprint": mobile_endpoint.fingerprint,
            "trustState": "verified",
            "endpointKind": mobile_endpoint.endpoint_kind
        },
        "targetBinding": {
            "targetEndpointId": pc_endpoint.endpoint_id,
            "targetAgentId": "codex",
            "workspaceId": "default"
        },
        "riskClass": "read_only",
        "requiresUserConfirmation": false,
        "idempotencyKey": "idem_mobile_relay_replay_fixture",
        "createdAt": now_iso(),
        "expiresAt": timestamp_after_seconds(MOBILE_RELAY_COMMAND_TTL_SECONDS).unwrap(),
        "body": {
            "limit": 1
        }
    });
    drop(mobile_material);
    drop(pc_material);
    let envelope = seal_mobile_relay_payload(
        &mobile_config,
        &mut test_runtime_secret_material(stringify!(&mobile_config)),
        crate::core::secure_mesh_crypto::SecureMeshPayloadKind::Command,
        &command_payload,
    )
    .unwrap();
    (pc_config, mobile_config, envelope)
}

pub(super) fn opened_result_payload(mobile_config: &Value, envelope: &Value) -> Value {
    let opened = open_mobile_relay_payload(
        mobile_config,
        &mut test_runtime_secret_material(stringify!(mobile_config)),
        envelope,
        crate::core::secure_mesh_crypto::SecureMeshPayloadKind::ResultPayload,
    )
    .unwrap();
    serde_json::from_slice::<Value>(&opened).unwrap()
}

pub(super) struct TestTempDir {
    path: PathBuf,
    cleanup_pending: bool,
}

impl TestTempDir {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            cleanup_pending: true,
        }
    }

    pub(super) fn close(mut self) -> std::io::Result<()> {
        let result = self.cleanup();
        if result.is_ok() {
            self.cleanup_pending = false;
        }
        result
    }

    fn cleanup(&self) -> std::io::Result<()> {
        let Some(metadata) = fs::symlink_metadata(&self.path)
            .map(Some)
            .or_else(|error| {
                (error.kind() == std::io::ErrorKind::NotFound)
                    .then_some(None)
                    .ok_or(error)
            })?
        else {
            return Ok(());
        };
        if metadata.file_type().is_symlink() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "test temporary root was replaced by a symlink",
            ));
        }
        fs::remove_dir_all(&self.path)
    }
}

impl std::ops::Deref for TestTempDir {
    type Target = PathBuf;

    fn deref(&self) -> &Self::Target {
        &self.path
    }
}

impl AsRef<std::path::Path> for TestTempDir {
    fn as_ref(&self) -> &std::path::Path {
        &self.path
    }
}

impl Drop for TestTempDir {
    fn drop(&mut self) {
        if self.cleanup_pending {
            let _ = self.cleanup();
        }
    }
}

pub(super) fn temp_dir(name: &str) -> TestTempDir {
    let dir = env::temp_dir().join(format!("licoup-{}-{}", name, Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    TestTempDir::new(dir)
}

#[test]
fn pairing_test_temp_dir_cleans_on_normal_return_and_unwind() {
    let normal_path = {
        let dir = temp_dir("mobile-relay-guard-normal");
        let path = dir.to_path_buf();
        assert!(path.is_dir());
        path
    };
    assert!(!normal_path.exists());

    let observed = std::cell::RefCell::new(None);
    let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let dir = temp_dir("mobile-relay-guard-unwind");
        observed.replace(Some(dir.to_path_buf()));
        panic!("synthetic guard unwind");
    }));
    assert!(unwind.is_err());
    assert!(!observed.into_inner().unwrap().exists());
}

#[cfg(unix)]
#[test]
fn pairing_test_temp_dir_refuses_symlink_replacement() {
    use std::os::unix::fs::symlink;

    let external = temp_dir("mobile-relay-guard-symlink-external");
    let guarded = temp_dir("mobile-relay-guard-symlink-root");
    let guarded_path = guarded.to_path_buf();
    fs::remove_dir(&guarded_path).unwrap();
    symlink(external.as_ref(), &guarded_path).unwrap();
    let error = guarded.close().unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    assert!(external.is_dir());
    fs::remove_file(guarded_path).unwrap();
}

#[derive(Clone, Debug)]
pub(super) struct CapturedHttpRequest {
    pub(super) method: String,
    pub(super) path: String,
    pub(super) body: String,
}

pub(super) struct CanonicalStation {
    pub(super) address: String,
    pub(super) captured: Arc<Mutex<Vec<CapturedHttpRequest>>>,
    pub(super) handle: thread::JoinHandle<()>,
}

impl CanonicalStation {
    pub(super) fn start(expected_requests: usize, receive_envelopes: Vec<Value>) -> Self {
        Self::start_with_send_response_drop(expected_requests, receive_envelopes, false)
    }

    pub(super) fn start_with_first_send_response_dropped(expected_requests: usize) -> Self {
        Self::start_with_send_response_drop(expected_requests, Vec::new(), true)
    }

    pub(super) fn start_with_envelopes_and_first_send_response_dropped(
        expected_requests: usize,
        receive_envelopes: Vec<Value>,
    ) -> Self {
        Self::start_with_send_response_drop(expected_requests, receive_envelopes, true)
    }

    fn start_with_send_response_drop(
        expected_requests: usize,
        receive_envelopes: Vec<Value>,
        drop_first_send_response: bool,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let captured = Arc::new(Mutex::new(Vec::<CapturedHttpRequest>::new()));
        let thread_captured = Arc::clone(&captured);
        let handle = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(15);
            let mut first_send_response_dropped = false;
            let mut available_envelopes = receive_envelopes;
            while thread_captured.lock().unwrap().len() < expected_requests
                && Instant::now() < deadline
            {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream.set_nonblocking(false).unwrap();
                        let request = read_http_request(&mut stream);
                        let operation = station_operation(&request);
                        let mut response =
                            canonical_station_response(&request, &available_envelopes);
                        if operation == BadTowerStationOperation::DeleteEnvelope {
                            let mailbox_id = mailbox_id_from_path(&request.path).to_string();
                            let envelope_id = request
                                .path
                                .rsplit('/')
                                .next()
                                .unwrap_or_default()
                                .to_string();
                            available_envelopes.retain(|envelope| {
                                envelope.get("mailboxId").and_then(Value::as_str)
                                    != Some(mailbox_id.as_str())
                                    || envelope.get("envelopeId").and_then(Value::as_str)
                                        != Some(envelope_id.as_str())
                            });
                        }
                        thread_captured.lock().unwrap().push(request);
                        if drop_first_send_response
                            && operation == BadTowerStationOperation::SendEnvelope
                            && !first_send_response_dropped
                        {
                            first_send_response_dropped = true;
                            continue;
                        }
                        if first_send_response_dropped
                            && operation == BadTowerStationOperation::SendEnvelope
                        {
                            response.body["duplicate"] = json!(true);
                        }
                        write_http_json_response(&mut stream, response.status, &response.body);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                    }
                    Err(error) => panic!("canonical station accept failed: {error}"),
                }
            }
        });
        Self {
            address,
            captured,
            handle,
        }
    }

    pub(super) fn url(&self) -> String {
        format!("http://{}", self.address)
    }

    pub(super) fn request_body(&self, index: usize) -> String {
        self.captured
            .lock()
            .unwrap()
            .get(index)
            .map(|request| request.body.clone())
            .unwrap_or_default()
    }

    pub(super) fn request_path(&self, index: usize) -> String {
        self.captured
            .lock()
            .unwrap()
            .get(index)
            .map(|request| request.path.clone())
            .unwrap_or_default()
    }

    pub(super) fn operations(&self) -> Vec<BadTowerStationOperation> {
        self.captured
            .lock()
            .unwrap()
            .iter()
            .map(station_operation)
            .collect()
    }

    pub(super) fn assert_operations(&self, operations: &[BadTowerStationOperation]) {
        assert_eq!(self.operations(), operations);
    }

    pub(super) fn join(self) {
        self.handle.join().unwrap();
    }
}

pub(super) fn read_http_request(stream: &mut TcpStream) -> CapturedHttpRequest {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut request_line = String::new();
    reader.read_line(&mut request_line).unwrap();
    let mut request_parts = request_line.split_whitespace();
    let method = request_parts.next().unwrap_or_default().to_string();
    let path = request_parts.next().unwrap_or("/").to_string();
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            break;
        }
        if let Some((name, value)) = trimmed.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse::<usize>().unwrap_or(0);
            }
        }
    }
    let mut body = vec![0u8; content_length];
    reader.read_exact(&mut body).unwrap();
    CapturedHttpRequest {
        method,
        path,
        body: String::from_utf8(body).unwrap(),
    }
}

pub(super) fn with_station_params(params: Value) -> Value {
    params
}

struct StationHttpResponse {
    status: u16,
    body: Value,
}

fn canonical_station_response(
    request: &CapturedHttpRequest,
    receive_envelopes: &[Value],
) -> StationHttpResponse {
    match station_operation(request) {
        BadTowerStationOperation::LeaseMailbox => StationHttpResponse {
            status: 200,
            body: json!({
                "mailboxId": mailbox_id_from_path(&request.path),
                "leaseExpiresAt": "2099-01-01T00:01:00Z"
            }),
        },
        BadTowerStationOperation::SendEnvelope => StationHttpResponse {
            status: 202,
            body: json!({
                "accepted": true,
                "duplicate": false
            }),
        },
        BadTowerStationOperation::ReceiveEnvelopes => StationHttpResponse {
            status: 200,
            body: json!({
                "envelopes": receive_envelopes
                    .iter()
                    .filter(|envelope| {
                        envelope.get("mailboxId").and_then(Value::as_str)
                            == Some(mailbox_id_from_path(&request.path))
                    })
                    .cloned()
                    .collect::<Vec<_>>()
            }),
        },
        BadTowerStationOperation::DeleteEnvelope => StationHttpResponse {
            status: 200,
            body: json!({
                "acknowledged": true
            }),
        },
        BadTowerStationOperation::ConfigureTransport => {
            panic!("transport construction has no HTTP operation")
        }
    }
}

fn station_operation(request: &CapturedHttpRequest) -> BadTowerStationOperation {
    match request.method.as_str() {
        "POST" if request.path == "/v1/envelopes" => BadTowerStationOperation::SendEnvelope,
        "POST" if request.path.ends_with("/lease") => BadTowerStationOperation::LeaseMailbox,
        "GET" if request.path.contains("/envelopes?limit=") => {
            BadTowerStationOperation::ReceiveEnvelopes
        }
        "DELETE" if request.path.contains("/envelopes/") => {
            BadTowerStationOperation::DeleteEnvelope
        }
        _ => panic!("unexpected station request"),
    }
}

fn mailbox_id_from_path(path: &str) -> &str {
    path.split('/').nth(3).unwrap_or_default()
}

pub(super) fn write_http_json_response(stream: &mut TcpStream, status: u16, body: &Value) {
    let serialized = serde_json::to_string(body).unwrap();
    let reason = if status == 202 { "Accepted" } else { "OK" };
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        serialized.len(),
        serialized
    )
    .unwrap();
}
