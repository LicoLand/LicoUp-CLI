//! OneDrive adapter for the trusted-history store port.
//!
//! The caller authorizes against OneDrive first and hands this adapter an
//! already-authorized bearer token. The adapter never logs, persists, refreshes
//! or otherwise owns that token: it borrows the credential for each request and
//! redacts it from its `Debug` output. Login and token refresh belong to the
//! caller (`task_ll_u12_auth`), not here.
//!
//! Traffic crosses a narrow [`OneDriveTransport`] port so the adapter can be
//! exercised without a network. [`UreqOneDriveTransport`] is the production
//! implementation; the module's tests use a synthetic server that speaks the
//! same port with HTTP-like statuses (404/410 for missing objects, 412 for
//! immutable conflicts, and paged listings).

use std::{collections::BTreeMap, fmt, io::Read, sync::Arc, time::Duration};

use serde::Deserialize;
use thiserror::Error;
use zeroize::Zeroizing;

use super::{
    policy::StoredHistoryObject,
    store::{
        AuthorizedHistoryStore, DeviceId, HistoryStoreError, InventoryPage, InventoryVersion,
        ManifestSegment, ObjectId, SegmentId,
    },
};

/// A validated OneDrive endpoint for retained history.
///
/// The base URL is the Microsoft Graph root (or a compatible deployment), the
/// drive identifies the OneDrive drive, and the container is the path under the
/// drive root that holds objects and manifest segments. Construct it with
/// [`OneDriveHistoryConfig::new`]; invalid endpoints are rejected before any
/// request is attempted.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OneDriveHistoryConfig {
    base_url: String,
    drive_id: String,
    container: String,
}

impl OneDriveHistoryConfig {
    /// Creates an endpoint configuration.
    ///
    /// `base_url` must be an absolute HTTPS root without a trailing slash,
    /// query or fragment. `drive_id` is a single path component. `container` is
    /// a relative path whose components may not be empty, `.` or `..`.
    pub fn new(
        base_url: impl Into<String>,
        drive_id: impl Into<String>,
        container: impl Into<String>,
    ) -> Result<Self, OneDriveConfigError> {
        let base_url = base_url.into();
        let drive_id = drive_id.into();
        let container = container.into();
        if !is_valid_base_url(&base_url)
            || !is_safe_path_component(&drive_id)
            || !is_safe_container(&container)
        {
            return Err(OneDriveConfigError::InvalidEndpoint);
        }
        Ok(Self {
            base_url,
            drive_id,
            container,
        })
    }

    /// The Microsoft Graph base URL, for example `https://graph.microsoft.com/v1.0`.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// The drive that retains history objects and manifest segments.
    pub fn drive_id(&self) -> &str {
        &self.drive_id
    }

    /// The drive-relative container path used by this adapter.
    pub fn container(&self) -> &str {
        &self.container
    }

    fn root_url(&self) -> String {
        format!(
            "{}/drives/{}/root:/{}",
            self.base_url,
            encode_component(&self.drive_id),
            self.container
                .split('/')
                .map(encode_component)
                .collect::<Vec<_>>()
                .join("/"),
        )
    }
}

/// Why a [`OneDriveHistoryConfig`] or adapter was rejected before use.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum OneDriveConfigError {
    #[error("the OneDrive endpoint configuration is invalid")]
    InvalidEndpoint,
    #[error("an authorized bearer token is required")]
    MissingAccessToken,
}

/// The HTTP method of a [`OneDriveRequest`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OneDriveMethod {
    Get,
    Put,
}

impl OneDriveMethod {
    /// The wire spelling of this method.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Put => "PUT",
        }
    }
}

/// One request handed to a [`OneDriveTransport`].
///
/// The bearer token is deliberately not a field: it is passed separately to the
/// transport, so request values can be recorded or formatted without exposing
/// credentials.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OneDriveRequest {
    pub method: OneDriveMethod,
    pub url: String,
    /// `If-None-Match`, used to make a write conditional and immutable.
    pub if_none_match: Option<String>,
    pub body: Option<Vec<u8>>,
}

/// One HTTP-like response from a [`OneDriveTransport`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OneDriveResponse {
    pub status: u16,
    /// Response headers with lowercase names, for transport implementations
    /// whose callers need to inspect them.
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}

impl OneDriveResponse {
    /// Returns a response header by case-insensitive name.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .get(&name.to_ascii_lowercase())
            .map(String::as_str)
    }
}

/// The transport failed before an HTTP response was available.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OneDriveTransportError;

/// The narrow network seam used by [`OneDriveHistoryStore`].
///
/// Implementations exchange one request and borrow the bearer token for that
/// exchange only. They must not retain, log or persist the token. This port
/// exists so a synthetic server can drive the adapter in tests without
/// pretending that a vendor SDK is the contract.
pub trait OneDriveTransport: Sync {
    fn exchange(
        &self,
        request: OneDriveRequest,
        bearer_token: &str,
    ) -> Result<OneDriveResponse, OneDriveTransportError>;
}

impl<T: OneDriveTransport + Send + Sync + ?Sized> OneDriveTransport for Arc<T> {
    fn exchange(
        &self,
        request: OneDriveRequest,
        bearer_token: &str,
    ) -> Result<OneDriveResponse, OneDriveTransportError> {
        (**self).exchange(request, bearer_token)
    }
}

/// Production [`OneDriveTransport`] backed by `ureq`.
#[derive(Clone)]
pub struct UreqOneDriveTransport {
    agent: ureq::Agent,
}

impl UreqOneDriveTransport {
    /// Builds a transport with bounded connect and overall timeouts.
    pub fn new() -> Self {
        Self {
            agent: ureq::AgentBuilder::new()
                .timeout_connect(Duration::from_secs(10))
                .timeout(Duration::from_secs(60))
                .build(),
        }
    }
}

impl Default for UreqOneDriveTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for UreqOneDriveTransport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("UreqOneDriveTransport")
            .finish_non_exhaustive()
    }
}

impl OneDriveTransport for UreqOneDriveTransport {
    fn exchange(
        &self,
        request: OneDriveRequest,
        bearer_token: &str,
    ) -> Result<OneDriveResponse, OneDriveTransportError> {
        let mut pending = self
            .agent
            .request(request.method.as_str(), &request.url)
            .set("Authorization", &format!("Bearer {bearer_token}"))
            .set("Accept", "application/json");
        if let Some(condition) = &request.if_none_match {
            pending = pending.set("If-None-Match", condition);
        }
        let outcome = match request.body {
            Some(body) => pending
                .set("Content-Type", "application/json")
                .send_bytes(&body),
            None => pending.call(),
        };
        let response = match outcome {
            Ok(response) => response,
            Err(ureq::Error::Status(_, response)) => response,
            Err(ureq::Error::Transport(_)) => return Err(OneDriveTransportError),
        };
        let status = response.status();
        let headers = response
            .headers_names()
            .into_iter()
            .filter_map(|name| {
                response
                    .header(&name)
                    .map(|value| (name.to_ascii_lowercase(), value.to_owned()))
            })
            .collect();
        let mut body = Vec::new();
        response
            .into_reader()
            .read_to_end(&mut body)
            .map_err(|_| OneDriveTransportError)?;
        Ok(OneDriveResponse {
            status,
            headers,
            body,
        })
    }
}

/// A history store backed by OneDrive and an already-authorized bearer token.
///
/// Writes are conditional (`If-None-Match: *`). A rejected immutable write is
/// resolved by reading the existing value: identical content is accepted and
/// different content is [`HistoryStoreError::ImmutableConflict`]. A read of a
/// missing object returns [`HistoryStoreError::ObjectMissing`] and never a
/// synthetic or default body.
pub struct OneDriveHistoryStore<T: OneDriveTransport> {
    config: OneDriveHistoryConfig,
    access_token: Zeroizing<String>,
    transport: T,
}

impl<T: OneDriveTransport> OneDriveHistoryStore<T> {
    /// Builds an adapter around an authorized token and a transport.
    ///
    /// The token is validated for presence and header safety, then held only in
    /// memory for the lifetime of the adapter. It is never written anywhere.
    pub fn new(
        config: OneDriveHistoryConfig,
        access_token: impl Into<String>,
        transport: T,
    ) -> Result<Self, OneDriveConfigError> {
        let access_token = access_token.into();
        if access_token.is_empty() || access_token.chars().any(char::is_whitespace) {
            return Err(OneDriveConfigError::MissingAccessToken);
        }
        Ok(Self {
            config,
            access_token: Zeroizing::new(access_token),
            transport,
        })
    }

    fn exchange(&self, request: OneDriveRequest) -> Result<OneDriveResponse, HistoryStoreError> {
        self.transport
            .exchange(request, self.access_token.as_str())
            .map_err(|_| HistoryStoreError::ProviderFailure)
    }

    fn get_bytes(&self, url: String) -> Result<OneDriveResponse, HistoryStoreError> {
        self.exchange(OneDriveRequest {
            method: OneDriveMethod::Get,
            url,
            if_none_match: None,
            body: None,
        })
    }

    fn put_bytes(&self, url: String, body: Vec<u8>) -> Result<OneDriveResponse, HistoryStoreError> {
        self.exchange(OneDriveRequest {
            method: OneDriveMethod::Put,
            url,
            if_none_match: Some("*".to_owned()),
            body: Some(body),
        })
    }

    fn object_url(&self, object_id: &ObjectId) -> String {
        format!(
            "{}/objects/{}",
            self.config.root_url(),
            encode_component(object_id.as_str())
        )
    }

    fn segment_url(&self, device_id: &DeviceId, segment_id: &SegmentId) -> String {
        format!(
            "{}/manifest/segments/{}/{}",
            self.config.root_url(),
            encode_component(device_id.as_str()),
            encode_component(segment_id.as_str())
        )
    }

    fn read_existing_object(
        &self,
        object_id: &ObjectId,
    ) -> Result<StoredHistoryObject, HistoryStoreError> {
        let response = self.get_bytes(self.object_url(object_id))?;
        match response.status {
            200 => serde_json::from_slice(&response.body)
                .map_err(|_| HistoryStoreError::ProviderFailure),
            401 | 403 => Err(HistoryStoreError::AccessRequired),
            _ => Err(HistoryStoreError::ProviderFailure),
        }
    }

    fn read_existing_segment(
        &self,
        device_id: &DeviceId,
        segment_id: &SegmentId,
    ) -> Result<ManifestSegment, HistoryStoreError> {
        let response = self.get_bytes(self.segment_url(device_id, segment_id))?;
        match response.status {
            200 => serde_json::from_slice(&response.body)
                .map_err(|_| HistoryStoreError::ProviderFailure),
            401 | 403 => Err(HistoryStoreError::AccessRequired),
            _ => Err(HistoryStoreError::ProviderFailure),
        }
    }
}

impl<T: OneDriveTransport> fmt::Debug for OneDriveHistoryStore<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OneDriveHistoryStore")
            .field("base_url", &self.config.base_url)
            .field("drive_id", &self.config.drive_id)
            .field("container", &self.config.container)
            .field("access_token", &"<redacted>")
            .finish_non_exhaustive()
    }
}

impl<T: OneDriveTransport> AuthorizedHistoryStore for OneDriveHistoryStore<T> {
    fn list_manifest_page(&self, cursor: Option<&str>) -> Result<InventoryPage, HistoryStoreError> {
        let url = match cursor {
            Some(cursor) => format!(
                "{}/manifest?cursor={}",
                self.config.root_url(),
                encode_component(cursor)
            ),
            None => format!("{}/manifest", self.config.root_url()),
        };
        let response = self.get_bytes(url)?;
        if response.status != 200 {
            return Err(provider_status_error(response.status));
        }
        let page: ManifestPage = serde_json::from_slice(&response.body)
            .map_err(|_| HistoryStoreError::InvalidManifest)?;
        if page.next_cursor.as_deref() == Some("") {
            return Err(HistoryStoreError::InvalidManifest);
        }
        let version =
            InventoryVersion::new(page.version).map_err(|_| HistoryStoreError::InvalidManifest)?;
        for segment in &page.segments {
            validate_segment(segment)?;
        }
        Ok(InventoryPage {
            version,
            segments: page.segments,
            next_cursor: page.next_cursor,
        })
    }

    fn get_immutable(
        &self,
        object_id: &ObjectId,
        frozen_version: &InventoryVersion,
    ) -> Result<StoredHistoryObject, HistoryStoreError> {
        let url = format!(
            "{}?inventoryVersion={}",
            self.object_url(object_id),
            encode_component(frozen_version.as_str())
        );
        let response = self.get_bytes(url)?;
        match response.status {
            200 => {
                let object: StoredHistoryObject = serde_json::from_slice(&response.body)
                    .map_err(|_| HistoryStoreError::ProviderFailure)?;
                if &object.header.object_id != object_id {
                    return Err(HistoryStoreError::ProviderFailure);
                }
                Ok(object)
            }
            404 | 410 => Err(HistoryStoreError::ObjectMissing),
            409 => Err(HistoryStoreError::StaleInventory),
            status => Err(provider_status_error(status)),
        }
    }

    fn put_immutable(&mut self, object: StoredHistoryObject) -> Result<(), HistoryStoreError> {
        let object_id = object.header.object_id.clone();
        let body = serde_json::to_vec(&object).map_err(|_| HistoryStoreError::ProviderFailure)?;
        let response = self.put_bytes(self.object_url(&object_id), body)?;
        match response.status {
            200 | 201 | 204 => Ok(()),
            412 => {
                let existing = self.read_existing_object(&object_id)?;
                if existing == object {
                    Ok(())
                } else {
                    Err(HistoryStoreError::ImmutableConflict)
                }
            }
            status => Err(provider_status_error(status)),
        }
    }

    fn append_manifest_segment(
        &mut self,
        segment: ManifestSegment,
    ) -> Result<(), HistoryStoreError> {
        validate_segment(&segment)?;
        let device_id = segment.device_id.clone();
        let segment_id = segment.segment_id.clone();
        let body = serde_json::to_vec(&segment).map_err(|_| HistoryStoreError::ProviderFailure)?;
        let response = self.put_bytes(self.segment_url(&device_id, &segment_id), body)?;
        match response.status {
            200 | 201 | 204 => Ok(()),
            412 => {
                let existing = self.read_existing_segment(&device_id, &segment_id)?;
                if existing == segment {
                    Ok(())
                } else {
                    Err(HistoryStoreError::ImmutableConflict)
                }
            }
            status => Err(provider_status_error(status)),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManifestPage {
    version: String,
    #[serde(default)]
    segments: Vec<ManifestSegment>,
    #[serde(default)]
    next_cursor: Option<String>,
}

fn validate_segment(segment: &ManifestSegment) -> Result<(), HistoryStoreError> {
    if segment.format_version != 1
        || segment
            .entries
            .windows(2)
            .any(|pair| pair[0].object_id >= pair[1].object_id)
    {
        return Err(HistoryStoreError::InvalidManifest);
    }
    DeviceId::new(segment.device_id.as_str()).map_err(|_| HistoryStoreError::InvalidManifest)?;
    SegmentId::new(segment.segment_id.as_str()).map_err(|_| HistoryStoreError::InvalidManifest)?;
    for entry in &segment.entries {
        ObjectId::new(entry.object_id.as_str()).map_err(|_| HistoryStoreError::InvalidManifest)?;
    }
    Ok(())
}

fn provider_status_error(status: u16) -> HistoryStoreError {
    match status {
        401 | 403 => HistoryStoreError::AccessRequired,
        _ => HistoryStoreError::ProviderFailure,
    }
}

fn is_valid_base_url(value: &str) -> bool {
    let Some(rest) = value.strip_prefix("https://") else {
        return false;
    };
    !rest.is_empty()
        && !value.ends_with('/')
        && !value.contains('?')
        && !value.contains('#')
        && !value
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
}

fn is_safe_path_component(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && !value.contains('/')
        && !value
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
}

fn is_safe_container(value: &str) -> bool {
    value.split('/').all(is_safe_path_component)
}

/// Percent-encodes every byte outside the RFC 3986 unreserved set.
fn encode_component(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push('%');
            encoded.push(HEX[(byte >> 4) as usize] as char);
            encoded.push(HEX[(byte & 0x0f) as usize] as char);
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        sync::{Arc, Mutex},
    };

    use serde_json::{Value, json};
    use zeroize::Zeroizing;

    use super::{
        OneDriveConfigError, OneDriveHistoryConfig, OneDriveHistoryStore, OneDriveMethod,
        OneDriveRequest, OneDriveResponse, OneDriveTransport, OneDriveTransportError,
    };
    use crate::domain::history_backup::{
        AuthorizedHistoryStore, ContentKind, DeviceId, HistoryKeySource, HistoryStoreError,
        InventoryVersion, ManifestEntry, ManifestSegment, ObjectId, SegmentId, StoredHistoryObject,
        append_backup_segment, open_history_object,
        policy::{StorageMode, digest},
        seal_history_object,
    };

    const TOKEN: &str = "synthetic-bearer-token";

    struct PanicKeys;

    impl HistoryKeySource for PanicKeys {
        fn key_for_generation(&self, generation: u64) -> Option<Zeroizing<[u8; 32]>> {
            panic!("ProviderManaged objects must not consult keys (generation {generation})")
        }
    }

    fn fixture_entry(id: &str, value: Value) -> (ManifestEntry, Vec<u8>) {
        let bytes = serde_json::to_vec(&value).unwrap();
        let entry = ManifestEntry {
            object_id: ObjectId::new(id).unwrap(),
            content_kind: ContentKind::SemanticConversation,
            storage_mode: StorageMode::ProviderManaged,
            canonical_length: bytes.len() as u64,
            canonical_digest: digest(&bytes),
        };
        (entry, bytes)
    }

    fn segment(device: &str, segment: &str, entries: Vec<ManifestEntry>) -> ManifestSegment {
        ManifestSegment::new(
            DeviceId::new(device).unwrap(),
            SegmentId::new(segment).unwrap(),
            entries,
        )
        .unwrap()
    }

    fn entry_json(id: &str) -> Value {
        json!({
            "objectId": id,
            "contentKind": "semantic-conversation",
            "storageMode": {"kind": "provider-managed"},
            "canonicalLength": 1,
            "canonicalDigest": vec![0_u8; 32],
        })
    }

    fn assert_store_error<T>(result: Result<T, HistoryStoreError>, expected: HistoryStoreError) {
        match result {
            Err(error) => assert_eq!(error, expected),
            Ok(_) => panic!("expected {expected:?}, got an object"),
        }
    }

    fn synthetic_endpoint() -> (OneDriveHistoryConfig, Arc<SyntheticOneDriveServer>) {
        let config = OneDriveHistoryConfig::new(
            "https://synthetic.onedrive.example/v1.0",
            "drive-1",
            "licoup-history",
        )
        .unwrap();
        let server = Arc::new(SyntheticOneDriveServer::new(&config));
        (config, server)
    }

    #[test]
    fn append_list_and_open_follow_the_trusted_history_contract() {
        let (config, server) = synthetic_endpoint();
        let mut store = OneDriveHistoryStore::new(config, TOKEN, Arc::clone(&server)).unwrap();
        let (first_entry, first_bytes) = fixture_entry(
            "conversation/c1/revision/e1",
            json!({"id":"c1","revision":"e1","parts":["hello"]}),
        );
        let (second_entry, second_bytes) = fixture_entry(
            "conversation/c1/revision/e2",
            json!({"id":"c1","revision":"e2","parts":["hello","again"]}),
        );
        let first_object = seal_history_object(&first_entry, &first_bytes, &PanicKeys).unwrap();
        let second_object = seal_history_object(&second_entry, &second_bytes, &PanicKeys).unwrap();
        let first_segment = segment("device-a", "segment-1", vec![first_entry.clone()]);
        let second_segment = segment("device-b", "segment-1", vec![second_entry.clone()]);

        append_backup_segment(
            &mut store,
            vec![first_object.clone()],
            first_segment.clone(),
        )
        .unwrap();
        append_backup_segment(
            &mut store,
            vec![second_object.clone()],
            second_segment.clone(),
        )
        .unwrap();

        let first_page = store.list_manifest_page(None).unwrap();
        assert_eq!(first_page.version.as_str(), "inventory-2");
        assert_eq!(first_page.segments, vec![first_segment]);
        let cursor = first_page
            .next_cursor
            .clone()
            .expect("the first page leaves a second page behind");
        let second_page = store.list_manifest_page(Some(&cursor)).unwrap();
        assert_eq!(second_page.version, first_page.version);
        assert_eq!(second_page.segments, vec![second_segment]);
        assert_eq!(second_page.next_cursor, None);

        let first = store
            .get_immutable(&first_entry.object_id, &second_page.version)
            .unwrap();
        assert!(first == first_object);
        assert_eq!(
            open_history_object(&first_entry, &first, &PanicKeys).unwrap(),
            first_bytes
        );
        let second = store
            .get_immutable(&second_entry.object_id, &second_page.version)
            .unwrap();
        assert!(second == second_object);
        assert_eq!(
            open_history_object(&second_entry, &second, &PanicKeys).unwrap(),
            second_bytes
        );

        assert!(
            !format!("{store:?}").contains(TOKEN),
            "the adapter never prints its bearer token"
        );
        let requests = server.requests();
        assert!(requests.iter().all(|request| request.bearer_token == TOKEN));
        assert!(
            requests
                .iter()
                .filter(|request| request.method == OneDriveMethod::Put)
                .all(|request| request.if_none_match.as_deref() == Some("*")),
            "every immutable write is conditional"
        );
    }

    #[test]
    fn immutable_conflicts_compare_the_existing_value() {
        let (config, server) = synthetic_endpoint();
        let mut store = OneDriveHistoryStore::new(config, TOKEN, Arc::clone(&server)).unwrap();
        let (entry, bytes) = fixture_entry(
            "conversation/c1/revision/e1",
            json!({"id":"c1","revision":"e1"}),
        );
        let object = seal_history_object(&entry, &bytes, &PanicKeys).unwrap();
        let manifest = segment("device-a", "segment-1", vec![entry]);
        append_backup_segment(&mut store, vec![object.clone()], manifest.clone()).unwrap();

        // Repeating an exact immutable write is accepted even though the
        // synthetic server answers every conditional write with 412.
        store.put_immutable(object.clone()).unwrap();
        store.append_manifest_segment(manifest.clone()).unwrap();

        let mut changed_object = object;
        changed_object.body.push(b'!');
        assert_eq!(
            store.put_immutable(changed_object),
            Err(HistoryStoreError::ImmutableConflict)
        );

        let (other_entry, _) = fixture_entry(
            "conversation/c1/revision/e2",
            json!({"id":"c1","revision":"e2"}),
        );
        let changed_segment = segment("device-a", "segment-1", vec![other_entry]);
        assert_eq!(
            store.append_manifest_segment(changed_segment),
            Err(HistoryStoreError::ImmutableConflict)
        );
    }

    #[test]
    fn missing_objects_are_explicitly_unavailable_and_never_substituted() {
        let (config, server) = synthetic_endpoint();
        let mut store = OneDriveHistoryStore::new(config, TOKEN, Arc::clone(&server)).unwrap();
        let (entry, bytes) = fixture_entry(
            "conversation/c1/revision/e1",
            json!({"id":"c1","revision":"e1"}),
        );
        let object = seal_history_object(&entry, &bytes, &PanicKeys).unwrap();
        append_backup_segment(
            &mut store,
            vec![object],
            segment("device-a", "segment-1", vec![entry.clone()]),
        )
        .unwrap();
        let version = store.list_manifest_page(None).unwrap().version;

        for status in [404_u16, 410] {
            server.expire_object(&entry.object_id, status);
            assert_store_error(
                store.get_immutable(&entry.object_id, &version),
                HistoryStoreError::ObjectMissing,
            );
        }
    }

    #[test]
    fn malformed_manifest_pages_are_rejected_without_inventing_a_manifest() {
        let (config, server) = synthetic_endpoint();
        let store = OneDriveHistoryStore::new(config, TOKEN, Arc::clone(&server)).unwrap();
        let unsorted_page = json!({
            "version": "inventory-0",
            "segments": [{
                "formatVersion": 1,
                "deviceId": "device-a",
                "segmentId": "segment-1",
                "entries": [
                    entry_json("conversation/c1/revision/e2"),
                    entry_json("conversation/c1/revision/e1"),
                ],
            }],
            "nextCursor": null,
        });
        let future_format_page = json!({
            "version": "inventory-0",
            "segments": [{
                "formatVersion": 2,
                "deviceId": "device-a",
                "segmentId": "segment-1",
                "entries": [],
            }],
            "nextCursor": null,
        });
        let cases = vec![
            b"not a manifest page".to_vec(),
            serde_json::to_vec(&json!({"segments": [], "nextCursor": null})).unwrap(),
            serde_json::to_vec(&json!({"version": "", "segments": [], "nextCursor": null}))
                .unwrap(),
            serde_json::to_vec(
                &json!({"version": "inventory-0", "segments": [], "nextCursor": ""}),
            )
            .unwrap(),
            serde_json::to_vec(&unsorted_page).unwrap(),
            serde_json::to_vec(&future_format_page).unwrap(),
        ];

        for body in cases {
            server.set_raw_page(body);
            assert_eq!(
                store.list_manifest_page(None),
                Err(HistoryStoreError::InvalidManifest)
            );
        }
    }

    #[test]
    fn a_frozen_version_that_is_no_longer_current_is_stale() {
        let (config, server) = synthetic_endpoint();
        let mut store = OneDriveHistoryStore::new(config, TOKEN, Arc::clone(&server)).unwrap();
        let (first_entry, first_bytes) = fixture_entry(
            "conversation/c1/revision/e1",
            json!({"id":"c1","revision":"e1"}),
        );
        let (second_entry, second_bytes) = fixture_entry(
            "conversation/c1/revision/e2",
            json!({"id":"c1","revision":"e2"}),
        );
        append_backup_segment(
            &mut store,
            vec![seal_history_object(&first_entry, &first_bytes, &PanicKeys).unwrap()],
            segment("device-a", "segment-1", vec![first_entry.clone()]),
        )
        .unwrap();
        let frozen = store.list_manifest_page(None).unwrap().version;

        append_backup_segment(
            &mut store,
            vec![seal_history_object(&second_entry, &second_bytes, &PanicKeys).unwrap()],
            segment("device-b", "segment-1", vec![second_entry]),
        )
        .unwrap();
        assert_store_error(
            store.get_immutable(&first_entry.object_id, &frozen),
            HistoryStoreError::StaleInventory,
        );
        let current = store.list_manifest_page(None).unwrap().version;
        assert!(
            store
                .get_immutable(&first_entry.object_id, &current)
                .is_ok()
        );
    }

    #[test]
    fn provider_failures_map_to_one_clear_error() {
        let (config, server) = synthetic_endpoint();
        let wrong_token =
            OneDriveHistoryStore::new(config.clone(), "not-the-token", Arc::clone(&server))
                .unwrap();
        assert_eq!(
            wrong_token.list_manifest_page(None),
            Err(HistoryStoreError::AccessRequired)
        );

        let mut store = OneDriveHistoryStore::new(config, TOKEN, Arc::clone(&server)).unwrap();
        server.fail_transport();
        assert_eq!(
            store.list_manifest_page(None),
            Err(HistoryStoreError::ProviderFailure)
        );
        let (entry, bytes) = fixture_entry(
            "conversation/c1/revision/e1",
            json!({"id":"c1","revision":"e1"}),
        );
        let object = seal_history_object(&entry, &bytes, &PanicKeys).unwrap();
        assert_eq!(
            store.put_immutable(object),
            Err(HistoryStoreError::ProviderFailure)
        );
        assert_store_error(
            store.get_immutable(
                &entry.object_id,
                &InventoryVersion::new("inventory-0").unwrap(),
            ),
            HistoryStoreError::ProviderFailure,
        );
    }

    #[test]
    fn endpoint_and_token_configuration_are_validated_before_use() {
        assert_eq!(
            OneDriveHistoryConfig::new("http://insecure.example", "drive-1", "history"),
            Err(OneDriveConfigError::InvalidEndpoint)
        );
        assert_eq!(
            OneDriveHistoryConfig::new(
                "https://synthetic.onedrive.example/v1.0",
                "drive/one",
                "history"
            ),
            Err(OneDriveConfigError::InvalidEndpoint)
        );
        assert_eq!(
            OneDriveHistoryConfig::new(
                "https://synthetic.onedrive.example/v1.0",
                "drive-1",
                "../history"
            ),
            Err(OneDriveConfigError::InvalidEndpoint)
        );
        let config = OneDriveHistoryConfig::new(
            "https://synthetic.onedrive.example/v1.0",
            "drive-1",
            "licoup-history",
        )
        .unwrap();
        let server = Arc::new(SyntheticOneDriveServer::new(&config));
        assert_eq!(
            OneDriveHistoryStore::new(config, "token with spaces", server).unwrap_err(),
            OneDriveConfigError::MissingAccessToken
        );
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    struct RecordedRequest {
        method: OneDriveMethod,
        path: String,
        cursor: Option<String>,
        inventory_version: Option<String>,
        if_none_match: Option<String>,
        bearer_token: String,
    }

    struct SyntheticServerState {
        root_url: String,
        token: String,
        version: String,
        version_counter: u64,
        page_size: usize,
        raw_page: Option<Vec<u8>>,
        fail_transport: bool,
        segments: Vec<ManifestSegment>,
        objects: BTreeMap<ObjectId, StoredHistoryObject>,
        unavailable: BTreeMap<ObjectId, u16>,
        requests: Vec<RecordedRequest>,
    }

    /// A synthetic OneDrive endpoint that implements the same narrow transport
    /// port as production, including HTTP status semantics and paged listings.
    struct SyntheticOneDriveServer {
        state: Mutex<SyntheticServerState>,
    }

    impl SyntheticOneDriveServer {
        fn new(config: &OneDriveHistoryConfig) -> Self {
            Self {
                state: Mutex::new(SyntheticServerState {
                    root_url: config.root_url(),
                    token: TOKEN.to_owned(),
                    version: "inventory-0".to_owned(),
                    version_counter: 0,
                    page_size: 1,
                    raw_page: None,
                    fail_transport: false,
                    segments: Vec::new(),
                    objects: BTreeMap::new(),
                    unavailable: BTreeMap::new(),
                    requests: Vec::new(),
                }),
            }
        }

        fn expire_object(&self, object_id: &ObjectId, status: u16) {
            self.state
                .lock()
                .unwrap()
                .unavailable
                .insert(object_id.clone(), status);
        }

        fn set_raw_page(&self, body: Vec<u8>) {
            self.state.lock().unwrap().raw_page = Some(body);
        }

        fn fail_transport(&self) {
            self.state.lock().unwrap().fail_transport = true;
        }

        fn requests(&self) -> Vec<RecordedRequest> {
            self.state.lock().unwrap().requests.clone()
        }
    }

    impl OneDriveTransport for SyntheticOneDriveServer {
        fn exchange(
            &self,
            request: OneDriveRequest,
            bearer_token: &str,
        ) -> Result<OneDriveResponse, OneDriveTransportError> {
            let mut state = self.state.lock().unwrap();
            let (path, query) = request
                .url
                .split_once('?')
                .unwrap_or((request.url.as_str(), ""));
            let params = parse_query(query);
            state.requests.push(RecordedRequest {
                method: request.method,
                path: path.to_owned(),
                cursor: params.get("cursor").cloned(),
                inventory_version: params.get("inventoryVersion").cloned(),
                if_none_match: request.if_none_match.clone(),
                bearer_token: bearer_token.to_owned(),
            });
            if state.fail_transport {
                return Err(OneDriveTransportError);
            }
            if bearer_token != state.token {
                return Ok(json_response(401, &json!({"error": "unauthorized"})));
            }
            Ok(route(&mut state, &request, &params))
        }
    }

    fn route(
        state: &mut SyntheticServerState,
        request: &OneDriveRequest,
        params: &BTreeMap<String, String>,
    ) -> OneDriveResponse {
        let path = request
            .url
            .split_once('?')
            .map_or(request.url.as_str(), |(path, _)| path);
        let Some(relative) = path.strip_prefix(&state.root_url) else {
            return not_found(404);
        };
        match (request.method, relative) {
            (OneDriveMethod::Get, "/manifest") => {
                list_page(state, params.get("cursor").map(String::as_str))
            }
            (OneDriveMethod::Get, path) if path.starts_with("/objects/") => {
                get_object(state, path, params.get("inventoryVersion"))
            }
            (OneDriveMethod::Put, path) if path.starts_with("/objects/") => {
                put_object(state, path, request)
            }
            (OneDriveMethod::Get, path) if path.starts_with("/manifest/segments/") => {
                get_segment(state, path)
            }
            (OneDriveMethod::Put, path) if path.starts_with("/manifest/segments/") => {
                put_segment(state, path, request)
            }
            _ => not_found(404),
        }
    }

    fn list_page(state: &mut SyntheticServerState, cursor: Option<&str>) -> OneDriveResponse {
        if let Some(body) = &state.raw_page {
            return OneDriveResponse {
                status: 200,
                headers: json_headers(),
                body: body.clone(),
            };
        }
        let start = match cursor {
            Some(cursor) => match cursor.parse::<usize>() {
                Ok(start) if start <= state.segments.len() => start,
                _ => return empty_response(400),
            },
            None => 0,
        };
        let end = (start + state.page_size).min(state.segments.len());
        let page = json!({
            "version": state.version,
            "segments": &state.segments[start..end],
            "nextCursor": (end < state.segments.len()).then(|| end.to_string()),
        });
        json_response(200, &page)
    }

    fn get_object(
        state: &SyntheticServerState,
        path: &str,
        inventory_version: Option<&String>,
    ) -> OneDriveResponse {
        let Some(object_id) = object_id_from_path(path) else {
            return empty_response(400);
        };
        if let Some(status) = state.unavailable.get(&object_id) {
            return not_found(*status);
        }
        if let Some(version) = inventory_version
            && version.as_str() != state.version
        {
            return empty_response(409);
        }
        match state.objects.get(&object_id) {
            Some(object) => json_response(200, object),
            None => not_found(404),
        }
    }

    fn put_object(
        state: &mut SyntheticServerState,
        path: &str,
        request: &OneDriveRequest,
    ) -> OneDriveResponse {
        let Some(object_id) = object_id_from_path(path) else {
            return empty_response(400);
        };
        if request.if_none_match.as_deref() != Some("*") {
            return empty_response(400);
        }
        let Some(body) = &request.body else {
            return empty_response(400);
        };
        let Ok(object) = serde_json::from_slice::<StoredHistoryObject>(body) else {
            return empty_response(400);
        };
        if object.header.object_id != object_id {
            return empty_response(400);
        }
        if state.objects.contains_key(&object_id) {
            return empty_response(412);
        }
        state.objects.insert(object_id, object);
        empty_response(201)
    }

    fn get_segment(state: &SyntheticServerState, path: &str) -> OneDriveResponse {
        let Some((device_id, segment_id)) = segment_key_from_path(path) else {
            return empty_response(400);
        };
        match state
            .segments
            .iter()
            .find(|segment| segment.device_id == device_id && segment.segment_id == segment_id)
        {
            Some(segment) => json_response(200, segment),
            None => not_found(404),
        }
    }

    fn put_segment(
        state: &mut SyntheticServerState,
        path: &str,
        request: &OneDriveRequest,
    ) -> OneDriveResponse {
        let Some((device_id, segment_id)) = segment_key_from_path(path) else {
            return empty_response(400);
        };
        if request.if_none_match.as_deref() != Some("*") {
            return empty_response(400);
        }
        let Some(body) = &request.body else {
            return empty_response(400);
        };
        let Ok(segment) = serde_json::from_slice::<ManifestSegment>(body) else {
            return empty_response(400);
        };
        if segment.device_id != device_id || segment.segment_id != segment_id {
            return empty_response(400);
        }
        if state
            .segments
            .iter()
            .any(|existing| existing.device_id == device_id && existing.segment_id == segment_id)
        {
            return empty_response(412);
        }
        state.segments.push(segment);
        state.version_counter += 1;
        state.version = format!("inventory-{}", state.version_counter);
        empty_response(201)
    }

    fn object_id_from_path(path: &str) -> Option<ObjectId> {
        let encoded = path.strip_prefix("/objects/")?;
        if encoded.contains('/') {
            return None;
        }
        ObjectId::new(decode_component(encoded)?).ok()
    }

    fn segment_key_from_path(path: &str) -> Option<(DeviceId, SegmentId)> {
        let remainder = path.strip_prefix("/manifest/segments/")?;
        let (device, segment) = remainder.split_once('/')?;
        if segment.contains('/') {
            return None;
        }
        Some((
            DeviceId::new(decode_component(device)?).ok()?,
            SegmentId::new(decode_component(segment)?).ok()?,
        ))
    }

    fn parse_query(query: &str) -> BTreeMap<String, String> {
        query
            .split('&')
            .filter(|pair| !pair.is_empty())
            .filter_map(|pair| {
                let (key, value) = pair.split_once('=')?;
                Some((decode_component(key)?, decode_component(value)?))
            })
            .collect()
    }

    fn decode_component(value: &str) -> Option<String> {
        let bytes = value.as_bytes();
        let mut decoded = Vec::with_capacity(bytes.len());
        let mut index = 0;
        while index < bytes.len() {
            if bytes[index] == b'%' {
                let high = hex_value(*bytes.get(index + 1)?)?;
                let low = hex_value(*bytes.get(index + 2)?)?;
                decoded.push((high << 4) | low);
                index += 3;
            } else {
                decoded.push(bytes[index]);
                index += 1;
            }
        }
        String::from_utf8(decoded).ok()
    }

    fn hex_value(byte: u8) -> Option<u8> {
        match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'A'..=b'F' => Some(byte - b'A' + 10),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            _ => None,
        }
    }

    fn json_headers() -> BTreeMap<String, String> {
        BTreeMap::from([("content-type".to_owned(), "application/json".to_owned())])
    }

    fn json_response(status: u16, body: &impl serde::Serialize) -> OneDriveResponse {
        OneDriveResponse {
            status,
            headers: json_headers(),
            body: serde_json::to_vec(body).expect("synthetic response serializes"),
        }
    }

    fn empty_response(status: u16) -> OneDriveResponse {
        OneDriveResponse {
            status,
            headers: BTreeMap::new(),
            body: Vec::new(),
        }
    }

    fn not_found(status: u16) -> OneDriveResponse {
        json_response(
            status,
            &json!({
                "error": {
                    "code": "itemNotFound",
                    "message": "The resource could not be found.",
                },
            }),
        )
    }
}
