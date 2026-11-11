use crate::domain::agent_hub::contract::{
    ADAPTATION_DEEP, AgentRecipe, ArtifactIntegrity, ArtifactSpec, HOST_SCOPE, InstallChannel,
    PLUGIN_MANAGEMENT_BOUNDARY, PlatformInstallCapabilities, RecipeRegistryDocument,
    SCHEMA_VERSION,
};
use crate::platform::client_state::ClientStateStore;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::thread::{self, JoinHandle};
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) fn test_store(name: &str) -> ClientStateStore {
    ClientStateStore::new(temp_dir(&format!("store-{name}"))).unwrap()
}

pub(super) fn temp_dir(name: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let dir = env::temp_dir().join(format!(
        "lico-agent-hub-{}-{}-{}",
        name,
        now.as_secs(),
        now.subsec_nanos()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

pub(super) fn portable_params(name: &str) -> (PathBuf, serde_json::Value) {
    let dir = temp_dir(name);
    let params = serde_json::json!({
        "portableDir": dir.to_string_lossy(),
        "platformCapabilities": {
            "os": "macos",
            "architecture": "aarch64",
            "managers": ["homebrew", "npm"],
            "scanGeneration": 7
        },
        "discoveryCandidates": []
    });
    (dir, params)
}

pub(super) fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        output.push_str(&format!("{byte:02x}"));
    }
    output
}

pub(super) fn bare_host_capabilities(os: &str, architecture: &str) -> PlatformInstallCapabilities {
    PlatformInstallCapabilities {
        os: os.to_string(),
        architecture: architecture.to_string(),
        managers: Vec::new(),
        scan_generation: 1,
    }
}

/// One synthetic `official-artifact` channel whose source is the loopback
/// fixture server, so the production fetch path runs without the network.
pub(super) fn fixture_artifact_channel(
    base: &str,
    integrity: Option<ArtifactIntegrity>,
) -> InstallChannel {
    let origin_host = url::Url::parse(base)
        .ok()
        .and_then(|url| url.host_str().map(str::to_string))
        .expect("fixture base must carry a host");
    InstallChannel {
        id: "official-artifact".to_string(),
        kind: "official-artifact".to_string(),
        oses: vec!["macos".to_string(), "linux".to_string()],
        architectures: vec!["aarch64".to_string(), "x86_64".to_string()],
        priority: 10,
        official_recommended: true,
        licoup_verified: true,
        requires_manager: "none".to_string(),
        elevation: "none".to_string(),
        scope: "user".to_string(),
        selectable: true,
        unsupported_reason: None,
        package_coordinate: "synthetic-standalone".to_string(),
        package_form: None,
        official_source: "https://vendor.invalid/agent".to_string(),
        version_policy: "vendor-latest".to_string(),
        artifact: Some(ArtifactSpec {
            // A loopback origin is the one host the acquirer accepts without
            // HTTPS; it exists for this fixture. Bundled recipes never declare one.
            origin_host,
            url_template: format!("{base}/agent-{{vendorOs}}-{{vendorArch}}.tar.gz"),
            vendor_os: [("macos".to_string(), "darwin".to_string())]
                .into_iter()
                .collect(),
            vendor_arch: [("aarch64".to_string(), "arm64".to_string())]
                .into_iter()
                .collect(),
            installer: Default::default(),
            redirect_hosts: Vec::new(),
            integrity,
        }),
        install: None,
        install_argv: vec![
            "tar".to_string(),
            "-xzf".to_string(),
            "{artifact}".to_string(),
            "-C".to_string(),
            "{staging}".to_string(),
        ],
        windows_install_argv: Vec::new(),
        update_argv: vec![
            "tar".to_string(),
            "-xzf".to_string(),
            "{artifact}".to_string(),
            "-C".to_string(),
            "{staging}".to_string(),
        ],
        uninstall_argv: vec!["rm".to_string(), "{install}".to_string()],
        verify_argv: vec!["synthetic-agent".to_string(), "--version".to_string()],
    }
}

pub(super) fn synthetic_agent(id: &str, channels: Vec<InstallChannel>) -> AgentRecipe {
    AgentRecipe {
        id: id.to_string(),
        label: id.to_string(),
        adaptation: ADAPTATION_DEEP.to_string(),
        binary_names: vec![id.to_string()],
        protocol: "synthetic".to_string(),
        license: "MIT".to_string(),
        summary: "synthetic recipe".to_string(),
        homepage: "https://vendor.invalid/agent".to_string(),
        requires_login: false,
        connection_modes: vec!["local".to_string()],
        official_docs: "https://vendor.invalid/docs".to_string(),
        channels,
        unsupported: Vec::new(),
    }
}

pub(super) fn synthetic_registry(agents: Vec<AgentRecipe>) -> RecipeRegistryDocument {
    RecipeRegistryDocument {
        schema_version: SCHEMA_VERSION.to_string(),
        host_scope: HOST_SCOPE.to_string(),
        plugin_management_boundary: PLUGIN_MANAGEMENT_BOUNDARY.to_string(),
        adaptation_tags: vec![ADAPTATION_DEEP.to_string()],
        channel_kinds: vec!["official-artifact".to_string()],
        agents,
    }
}

pub(super) fn digest_document(name: &str, body: &[u8]) -> String {
    format!("{}  {name}\n", sha256_hex(body))
}

pub(super) enum FixtureReply {
    Body(String),
    #[allow(dead_code)]
    Status(u16),
    /// A redirect whose target the acquirer must re-pin before following.
    Redirect(String),
    /// A response whose declared length is far larger than any artifact bound.
    OversizedLength,
}

pub(super) struct FixtureRoute {
    pub path: String,
    pub reply: FixtureReply,
}

pub(super) struct FixtureServer {
    base: String,
    handle: JoinHandle<Vec<String>>,
}

/// Serves exactly `routes`, in order, over loopback HTTP.
///
/// The thread exits after the last route, so a request the acquirer should not
/// make is refused by the closed listener instead of hanging the test.
pub(super) fn serve(routes: Vec<FixtureRoute>) -> FixtureServer {
    serve_with(move |_| routes)
}

/// [`serve`] for a reply that has to name the fixture's own address, such as a
/// redirect to the same listener reached under a second hostname.
pub(super) fn serve_with<F>(routes: F) -> FixtureServer
where
    F: FnOnce(&str) -> Vec<FixtureRoute>,
{
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback fixture");
    let base = format!("http://{}", listener.local_addr().expect("fixture address"));
    let routes = routes(&base);
    let handle = thread::spawn(move || {
        let mut seen = Vec::new();
        for route in routes {
            let (mut stream, _) = listener.accept().expect("accept fixture request");
            let path = read_request_path(&mut stream);
            assert_eq!(path, route.path, "unexpected fixture request path");
            seen.push(path);
            match route.reply {
                FixtureReply::Body(body) => write_body(&mut stream, &body),
                FixtureReply::Status(status) => {
                    write!(
                        stream,
                        "HTTP/1.1 {status} Error\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                    )
                    .expect("write fixture status");
                }
                FixtureReply::Redirect(location) => {
                    write!(
                        stream,
                        "HTTP/1.1 302 Found\r\nlocation: {location}\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                    )
                    .expect("write fixture redirect");
                }
                FixtureReply::OversizedLength => {
                    write!(
                        stream,
                        "HTTP/1.1 200 OK\r\ncontent-type: application/octet-stream\r\ncontent-length: 999999999\r\nconnection: close\r\n\r\n"
                    )
                    .expect("write oversized fixture headers");
                    stream.write_all(b"x").expect("write fixture body");
                }
            }
        }
        seen
    });
    FixtureServer { base, handle }
}

impl FixtureServer {
    pub(super) fn base(&self) -> String {
        self.base.clone()
    }

    pub(super) fn finish(self) -> Vec<String> {
        self.handle.join().expect("fixture server thread")
    }
}

fn write_body(stream: &mut TcpStream, body: &str) {
    write!(
        stream,
        "HTTP/1.1 200 OK\r\ncontent-type: application/octet-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        body.len()
    )
    .expect("write fixture headers");
    stream
        .write_all(body.as_bytes())
        .expect("write fixture body");
}

fn read_request_path(stream: &mut TcpStream) -> String {
    let mut reader = BufReader::new(stream.try_clone().expect("clone fixture stream"));
    let mut request_line = String::new();
    reader
        .read_line(&mut request_line)
        .expect("read request line");
    let mut parts = request_line.split_whitespace();
    let path = parts.nth(1).expect("request target").to_string();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).expect("read header");
        if line.trim_end_matches(['\r', '\n']).is_empty() {
            break;
        }
    }
    path
}

/// Parameters for one bare host whose only installable channel is the fixture.
pub(super) fn fixture_params(state_root: &std::path::Path) -> Value {
    serde_json::json!({
        "stateRoot": state_root.to_string_lossy(),
        "platformCapabilities": {
            "os": "linux",
            "architecture": "x86_64",
            "managers": [],
            "scanGeneration": 11
        },
        "discoveryCandidates": []
    })
}
