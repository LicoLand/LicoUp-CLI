//! Process contract proof using a synthetic CLI, with no linked kernel or GUI.
//!
//! The subject is the optional `lico-subagent-mcp` binary, so this target needs
//! the `service` feature. Building the crate without it yields the neutral MCP
//! engine alone, and this proof does not apply there.
#![cfg(all(unix, feature = "service"))]
use serde_json::{Value, json};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::{Child, Command, Stdio},
};
const MCP: &str = env!("CARGO_BIN_EXE_lico-subagent-mcp");

struct Fixture {
    root: PathBuf,
    cli: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("licoup-mcp-contract-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let cli = root.join("fixture-cli");
        fs::write(&cli, r##"#!/usr/bin/env node
const fs = require('node:fs');
const path = require('node:path');
const root = process.env.LICOUP_HOME;
const marker = path.join(root, 'synthetic-kernel-state');
if (!fs.existsSync(marker)) fs.writeFileSync(marker, 'turn:fixture:running');
const tools = ['lico_subagents_list','lico_subagent_probe','lico_subagent_delegate','lico_subagent_continue','lico_subagent_cancel','lico_assistant_profiles'].map(name => ({name,inputSchema:{type:'object',additionalProperties:false,properties:{},required:[]}}));
require('node:readline').createInterface({input:process.stdin}).on('line', line => {
  const request = JSON.parse(line);
  let result = {callers:['fixture'],tools};
  function respond(result) {
    process.stdout.write(JSON.stringify({protocol:'licoup.stdio.v1',id:request.id,workflowId:request.workflowId,ok:true,result})+'\n');
  }
  if (request.args[1] === 'execute') {
    const call = JSON.parse(request.args[3]);
    result = {accepted:true,operation:call.name,state:fs.readFileSync(marker,'utf8')};
    if (call.name === 'lico_subagents_list' && fs.existsSync(path.join(root,'hold-list'))) {
      fs.writeFileSync(path.join(root,'list-started'),'yes');
      const timer = setInterval(() => {
        if (fs.existsSync(path.join(root,'cancelled'))) { clearInterval(timer); respond(result); }
      }, 10);
      return;
    }
    if (call.name === 'lico_subagent_cancel') fs.writeFileSync(path.join(root,'cancelled'),'yes');
  }
  respond(result);
});
"##).unwrap();
        fs::set_permissions(&cli, fs::Permissions::from_mode(0o700)).unwrap();
        Self { root, cli }
    }
    fn lifecycle(&self, action: &str) -> Value {
        let output = Command::new(MCP)
            .args(["service", action])
            .env("LICOUP_HOME", &self.root)
            .env("LICOUP_CLI_BINARY", &self.cli)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "synthetic service lifecycle failed"
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }
    fn connector(&self) -> Child {
        Command::new(MCP)
            .args(["--caller", "fixture"])
            .env("LICOUP_HOME", &self.root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = Command::new(MCP)
            .args(["service", "stop"])
            .env("LICOUP_HOME", &self.root)
            .env("LICOUP_CLI_BINARY", &self.cli)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn exchange(input: &mut impl Write, output: &mut impl BufRead, value: Value) -> Value {
    writeln!(input, "{value}").unwrap();
    input.flush().unwrap();
    let mut line = String::new();
    assert!(output.read_line(&mut line).unwrap() > 0);
    serde_json::from_str(&line).unwrap()
}
#[test]
fn reload_renews_existing_connector_and_preserves_native_turn_state() {
    let fixture = Fixture::new();
    assert_eq!(fixture.lifecycle("start")["state"], "running");
    let mut connector = fixture.connector();
    let mut input = connector.stdin.take().unwrap();
    let mut output = BufReader::new(connector.stdout.take().unwrap());
    let initialized = exchange(
        &mut input,
        &mut output,
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"fixture","version":"1"}}}),
    );
    assert_eq!(
        initialized["result"]["serverInfo"]["name"],
        "lico-up-subagents"
    );
    let list = exchange(
        &mut input,
        &mut output,
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}),
    );
    assert_eq!(list["result"]["tools"].as_array().unwrap().len(), 5);
    let rejected = exchange(
        &mut input,
        &mut output,
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"lico_assistant_profiles","arguments":{}}}),
    );
    assert_eq!(rejected["error"]["code"], -32601);
    assert_eq!(fixture.lifecycle("stop")["state"], "stopped");
    let unavailable = exchange(
        &mut input,
        &mut output,
        json!({"jsonrpc":"2.0","id":30,"method":"tools/call","params":{"name":"lico_subagent_delegate","arguments":{}}}),
    );
    assert_eq!(unavailable["id"], 30);
    assert_eq!(
        unavailable["error"]["data"]["reasonCode"],
        "mcp_module_unavailable"
    );
    assert_eq!(
        unavailable["error"]["data"]["requestMayHaveExecuted"],
        false
    );
    assert_eq!(fixture.lifecycle("start")["state"], "running");
    assert_eq!(fixture.lifecycle("reload")["state"], "running");
    let call = exchange(
        &mut input,
        &mut output,
        json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"lico_subagents_list","arguments":{}}}),
    );
    assert_eq!(
        call["result"]["structuredContent"]["state"],
        "turn:fixture:running"
    );
    drop(input);
    assert!(connector.wait().unwrap().success());
    assert_eq!(fixture.lifecycle("stop")["state"], "stopped");
    assert_eq!(
        fs::read_to_string(fixture.root.join("synthetic-kernel-state")).unwrap(),
        "turn:fixture:running"
    );
}

#[test]
fn cancellation_has_a_reserved_session_while_inventory_is_held() {
    let fixture = Fixture::new();
    fixture.lifecycle("start");
    let mut connector = fixture.connector();
    let mut input = connector.stdin.take().unwrap();
    let mut output = BufReader::new(connector.stdout.take().unwrap());
    exchange(
        &mut input,
        &mut output,
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"fixture","version":"1"}}}),
    );
    fs::write(fixture.root.join("hold-list"), "synthetic hold").unwrap();
    writeln!(input, "{}", json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"lico_subagents_list","arguments":{}}})).unwrap();
    input.flush().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !fixture.root.join("list-started").exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "synthetic inventory did not start"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    writeln!(input, "{}", json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"lico_subagent_cancel","arguments":{}}})).unwrap();
    input.flush().unwrap();
    let (send, receive) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let responses = (0..2)
            .map(|_| {
                let mut line = String::new();
                output.read_line(&mut line).unwrap();
                serde_json::from_str::<Value>(&line).unwrap()
            })
            .collect::<Vec<_>>();
        send.send(responses).unwrap();
    });
    let responses = receive
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("cancel must pass the held synthetic inventory request");
    assert!(
        responses
            .iter()
            .any(|v| v["id"] == 3 && v["result"]["structuredContent"]["accepted"] == true)
    );
    drop(input);
    assert!(connector.wait().unwrap().success());
}

#[test]
fn stale_discovery_after_an_owned_service_crash_does_not_block_stop() {
    let fixture = Fixture::new();
    let mut service = Command::new(MCP)
        .args(["service", "serve"])
        .env("LICOUP_HOME", &fixture.root)
        .env("LICOUP_CLI_BINARY", &fixture.cli)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if fixture.lifecycle("status")["state"] == "running" {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "synthetic service did not start"
        );
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    service.kill().unwrap();
    service.wait().unwrap();
    assert_eq!(fixture.lifecycle("stop")["state"], "stopped");
    assert!(
        !fixture
            .root
            .join("client-state/subagent-mcp/discovery.json")
            .exists()
    );
}

#[test]
fn missing_saved_data_home_refuses_the_real_service_writer_path() {
    let fixture_root = std::env::temp_dir()
        .canonicalize()
        .unwrap()
        .join(format!("licoup-mcp-missing-home-{}", uuid::Uuid::new_v4()));
    let home = fixture_root.join("home");
    let missing_data_root = fixture_root.join("removed-volume/licoup-data");
    #[cfg(target_os = "macos")]
    let locator_parent = home.join("Library/Application Support/LicoUp");
    #[cfg(not(target_os = "macos"))]
    let locator_parent = home.join(".config/licoup");
    fs::create_dir_all(&locator_parent).unwrap();
    fs::set_permissions(&locator_parent, fs::Permissions::from_mode(0o700)).unwrap();
    let locator = locator_parent.join("data-home");
    fs::write(&locator, format!("{}\n", missing_data_root.display())).unwrap();
    fs::set_permissions(&locator, fs::Permissions::from_mode(0o600)).unwrap();

    let output = Command::new(MCP)
        .args(["service", "serve"])
        .env("HOME", &home)
        .env_remove("LICOUP_HOME")
        .env_remove("LICOUP_PORTABLE_DIR")
        .env_remove("XDG_CONFIG_HOME")
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(!missing_data_root.exists());
    assert!(String::from_utf8_lossy(&output.stderr).contains("service operation failed"));
    fs::remove_dir_all(fixture_root).unwrap();
}
