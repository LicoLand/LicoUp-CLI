//! A closed pipe is not absence evidence: the real escaped writer continues
//! modifying its granted root after host drain. Only nonce-bound fixture
//! cleanup ends it; neither the owner nor the durable predecessor is cleared.
use crate::support::*;
use licoup_native::platform::extension_host::isolation::{IsolationMode, ReleaseScope};
use licoup_native::platform::extension_host::{
    CatalogJournal, ExtensionHost, RuntimeCatalogJournal, SessionOwner,
};
use serde_json::{Value, json};
use std::{path::PathBuf, sync::Arc, time::Duration};

struct FixtureCleanup {
    root: PathBuf,
    nonce: String,
}
impl FixtureCleanup {
    fn stop(&self) -> bool {
        if std::fs::write(self.root.join("fixture-stop"), &self.nonce).is_err() {
            return false;
        }
        wait_for(Duration::from_secs(19), || {
            read_text(&self.root.join("writer-stopped.json"))
                .and_then(|s| serde_json::from_str::<Value>(&s).ok())
                .is_some_and(|v| {
                    v["nonce"] == self.nonce
                        && v["pid"].as_u64().is_some_and(|pid| !pid_alive(pid as u32))
                })
        })
    }
}
impl Drop for FixtureCleanup {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

fn json_record(root: &std::path::Path, name: &str) -> Value {
    assert!(
        wait_for(Duration::from_secs(8), || read_text(&root.join(name))
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .is_some()),
        "synthetic fixture did not record {name}"
    );
    serde_json::from_str(&read_text(&root.join(name)).expect("record")).expect("JSON")
}

#[test]
fn closed_stdio_writer_keeps_owner_and_predecessor_unverified() {
    let python = python_runtime();
    for route in ["setsid", "setpgid"] {
        let sandbox = Sandbox::new("silent-writer");
        let instance = sandbox.instance_root(route);
        let nonce = uuid::Uuid::new_v4().to_string();
        let script = sandbox.fixture_dir().join("silent_writer_agent.py");
        std::fs::write(&script, include_str!("fixtures/silent_writer_agent.py")).expect("fixture");
        // Declared before the carrier/host so fixture cleanup runs before the
        // synthetic root is removed, including during assertion unwinding.
        let cleanup = FixtureCleanup {
            root: instance.clone(),
            nonce: nonce.clone(),
        };
        let program = python_program(
            &python,
            vec![
                "-B".into(),
                script.display().to_string(),
                reference_sdk_dir().display().to_string(),
                route.into(),
                nonce.clone(),
            ],
            &sandbox,
            &instance,
        )
        .with_read_root(reference_sdk_dir())
        .with_requires_descendants();
        let carrier = carrier(
            programs("acme.silent/ext", "1.0.0", program),
            policy(IsolationMode::TrustedLocal, sandbox.root()),
        );
        let journal = Arc::new(RuntimeCatalogJournal::open(sandbox.root()).expect("journal"));
        let host = ExtensionHost::with_journal(carrier.clone(), contract_range(), journal.clone())
            .expect("host");
        let receipt = activate(
            &host,
            "acme.silent/ext",
            &["dev.example.agent/silent-writer"],
        )
        .expect("activate SDK fixture");
        let identity = json_record(&instance, "writer-identity.json");
        assert_eq!(identity["nonce"], nonce);
        assert_eq!(identity["route"], route);
        let child = identity["pid"].as_u64().expect("child pid") as u32;
        let facts = carrier.facts(&receipt.instance_id).expect("facts");
        assert_ne!(
            identity["pgid"],
            json!(facts.pid),
            "child really left the supervised group"
        );
        let call = host
            .begin("dev.example.agent/silent-writer", &json!({"input":"run"}))
            .expect("admit");
        settle(&host, &call.binding, Duration::from_secs(5));
        host.revoke("acme.silent/ext");
        assert_eq!(facts.release_scope(), Some(ReleaseScope::ProcessGroup));
        assert!(
            !facts.pipe_held_after_release(),
            "writer closed every stdio descriptor"
        );
        assert!(facts.exit().is_some(), "root exit was observed");
        assert!(
            pid_alive(child),
            "closed-stdio child survives host teardown"
        );
        assert_eq!(
            host.catalog()
                .entry(&receipt.instance_id)
                .expect("entry")
                .session_owner,
            SessionOwner::StoppedUnverified
        );
        assert!(!host.unverified_session_owners().is_empty());
        let before = std::fs::metadata(instance.join("writer-heartbeat"))
            .expect("heartbeat")
            .len();
        assert!(
            wait_for(Duration::from_secs(2), || std::fs::metadata(
                instance.join("writer-heartbeat")
            )
            .is_ok_and(|m| m.len() > before)),
            "escaped writer continues modifying its granted root after drain"
        );
        assert_eq!(journal.watermark().expect("watermark").active.len(), 1);
        drop(host);
        drop(journal);
        let reopened = Arc::new(RuntimeCatalogJournal::open(sandbox.root()).expect("reopen"));
        // A successor preparation uses a separate synthetic root and a
        // non-forking responder, never the adversarial writer's root or nonce.
        let successor = crate::support::carrier(
            programs(
                "acme.silent/ext",
                "1.0.0",
                shell_program(
                    &sandbox,
                    "respond",
                    "successor",
                    &sandbox.instance_root("successor"),
                ),
            ),
            policy(IsolationMode::TrustedLocal, sandbox.root()),
        );
        let restarted = ExtensionHost::with_journal(successor, contract_range(), reopened.clone())
            .expect("restart");
        assert_eq!(restarted.pending_predecessors().len(), 1);
        assert_eq!(
            activate(
                &restarted,
                "acme.silent/ext",
                &["dev.example.agent/silent-writer"]
            )
            .expect_err("scope stays fenced")
            .code,
            "extension_predecessor_unreconciled"
        );
        assert_eq!(reopened.watermark().expect("watermark").active.len(), 1);
        assert!(
            cleanup.stop(),
            "explicit nonce-bound fixture cleanup observed exit"
        );
        let stopped = json_record(&instance, "writer-stopped.json");
        assert_eq!(stopped["nonce"], nonce);
        assert_eq!(stopped["pid"], child);
        assert_eq!(stopped["reason"], "explicit-fixture-cleanup");
        // Even observed fixture cleanup is not host verification or permission
        // to call confirm_session_owner_stopped; the pointer stays intact.
        assert_eq!(restarted.pending_predecessors().len(), 1);
        assert_eq!(reopened.watermark().expect("watermark").active.len(), 1);
        eprintln!(
            "X2_PROBE closed-stdio route={route} continued-writes=true owner=unverified predecessor=retained cleanup=fixture-not-host"
        );
    }
}
