//! Line-oriented Key Transparency authority for selected-target interoperability evidence.
//!
//! This binary is available only behind `secure-mesh-acceptance-mock-kt`; the crate rejects that
//! feature in release profiles. It never labels its ephemeral authority as production and never
//! exports the signing key. A topology orchestrator owns the process and explicitly provisions
//! every client with the emitted public pin.

use std::collections::BTreeMap;
use std::io::{self, BufRead, Write};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Result, anyhow, ensure};
use ed25519_dalek::SigningKey;
use licoup_native::core::secure_mesh_directory::{
    SecureMeshDirectoryLeafClaim, UntrustedDirectoryAbsenceResponse, UntrustedDirectoryResponse,
};
use licoup_native::core::secure_mesh_transparency::{
    KT_JSON_SAFE_INTEGER_MAX, SecureMeshKtGossipPayload, SecureMeshKtLog,
};
use rand_core::OsRng;
use serde_json::{Value, json};

const MOCK_SCHEMA_VERSION: u64 = 1;
const MOCK_PROVENANCE: &str = "local-acceptance-mock";
const MAX_REQUEST_BYTES: usize = 256 * 1024;

fn main() -> Result<()> {
    let mut service = AcceptanceKtService::new();
    write_line(&service.ready_report())?;
    let stdin = io::stdin();
    let mut input = stdin.lock();
    loop {
        let line = match read_bounded_line(&mut input, MAX_REQUEST_BYTES)? {
            BoundedLine::Eof => break,
            BoundedLine::Oversized => {
                write_line(&rejected("request_too_large"))?;
                continue;
            }
            BoundedLine::Line(line) if line.iter().all(u8::is_ascii_whitespace) => continue,
            BoundedLine::Line(line) => line,
        };
        let request: Value = match serde_json::from_slice(&line) {
            Ok(request) => request,
            Err(_) => {
                write_line(&rejected("invalid_json"))?;
                continue;
            }
        };
        if request.get("action").and_then(Value::as_str) == Some("shutdown") {
            write_line(&json!({
                "ok": true,
                "schemaVersion": MOCK_SCHEMA_VERSION,
                "authorityProvenance": MOCK_PROVENANCE,
                "productionEligible": false,
                "stopped": true
            }))?;
            break;
        }
        let response = service
            .handle(&request)
            .unwrap_or_else(|_| rejected("request_rejected"));
        write_line(&response)?;
    }
    Ok(())
}

struct AcceptanceKtService {
    log: SecureMeshKtLog,
    claims: BTreeMap<String, SecureMeshDirectoryLeafClaim>,
}

impl AcceptanceKtService {
    fn new() -> Self {
        Self {
            log: SecureMeshKtLog::with_identity(
                SigningKey::generate(&mut OsRng),
                "lico-selected-target-acceptance-kt",
                "ephemeral-acceptance-key",
            ),
            claims: BTreeMap::new(),
        }
    }

    fn ready_report(&self) -> Value {
        let pin = self.log.pin();
        json!({
            "ok": true,
            "event": "ready",
            "schemaVersion": MOCK_SCHEMA_VERSION,
            "authorityProvenance": MOCK_PROVENANCE,
            "mock": true,
            "productionEligible": false,
            "signingKeyExported": false,
            "pin": {
                "logId": pin.log_id(),
                "keyId": pin.key_id(),
                "publicKeyHex": pin.public_key_hex(),
                "provenance": pin.provenance().stable_code()
            }
        })
    }

    fn handle(&mut self, request: &Value) -> Result<Value> {
        match required_text(request, "action")? {
            "report" => Ok(self.ready_report()),
            "publish" => self.publish(request),
            "query" => self.query(request),
            "query-absence" => self.query_absence(request),
            "gossip" => self.gossip(request),
            _ => Err(anyhow!("unsupported acceptance KT action")),
        }
    }

    fn publish(&mut self, request: &Value) -> Result<Value> {
        let claim: SecureMeshDirectoryLeafClaim = serde_json::from_value(
            request
                .get("claim")
                .cloned()
                .ok_or_else(|| anyhow!("claim is required"))?,
        )?;
        let first_tree_size = optional_u64(request, "firstTreeSize")?;
        let issued_at = issued_at(request)?;
        let stable_label = claim.stable_label();
        self.log.append_hashed_directory_leaf(
            &stable_label,
            claim.version(),
            claim.revoked(),
            claim.leaf_hash()?,
        )?;
        self.claims.insert(stable_label, claim.clone());
        self.directory_response(claim, first_tree_size, issued_at)
    }

    fn query(&self, request: &Value) -> Result<Value> {
        let stable_label = required_text(request, "stableLabel")?;
        let claim = self
            .claims
            .get(stable_label)
            .cloned()
            .ok_or_else(|| anyhow!("directory label is absent"))?;
        self.directory_response(
            claim,
            optional_u64(request, "firstTreeSize")?,
            issued_at(request)?,
        )
    }

    fn query_absence(&self, request: &Value) -> Result<Value> {
        let stable_label = required_text(request, "stableLabel")?.to_string();
        ensure!(
            !self.claims.contains_key(&stable_label),
            "directory label is present"
        );
        ensure!(
            self.log.tree_size() > 0,
            "mock KT log has no map checkpoint"
        );
        let first_tree_size = optional_u64(request, "firstTreeSize")?;
        let issued_at = issued_at(request)?;
        let response = UntrustedDirectoryAbsenceResponse {
            stable_label: stable_label.clone(),
            map_root_inclusion: self
                .log
                .inclusion_proof_at(self.log.tree_size() - 1, issued_at)?,
            absence_map: self.log.map_proof_at(&stable_label, issued_at)?,
            consistency: consistency(&self.log, first_tree_size, issued_at)?,
        };
        Ok(json!({
            "ok": true,
            "schemaVersion": MOCK_SCHEMA_VERSION,
            "authorityProvenance": MOCK_PROVENANCE,
            "mock": true,
            "productionEligible": false,
            "response": response
        }))
    }

    fn gossip(&self, request: &Value) -> Result<Value> {
        let issued_at = issued_at(request)?;
        let first_tree_size = optional_u64(request, "firstTreeSize")?;
        let payload = SecureMeshKtGossipPayload::from_sth(
            self.log.sign_tree_head(issued_at)?,
            consistency(&self.log, first_tree_size, issued_at)?,
        );
        Ok(json!({
            "ok": true,
            "schemaVersion": MOCK_SCHEMA_VERSION,
            "authorityProvenance": MOCK_PROVENANCE,
            "mock": true,
            "productionEligible": false,
            "gossip": payload
        }))
    }

    fn directory_response(
        &self,
        claim: SecureMeshDirectoryLeafClaim,
        first_tree_size: Option<u64>,
        issued_at: u64,
    ) -> Result<Value> {
        ensure!(
            self.log.tree_size() > 0,
            "mock KT log has no map checkpoint"
        );
        let stable_label = claim.stable_label();
        let response = UntrustedDirectoryResponse {
            claim,
            inclusion: self
                .log
                .inclusion_proof_at(self.log.tree_size() - 1, issued_at)?,
            latest_map: self.log.map_proof_at(&stable_label, issued_at)?,
            consistency: consistency(&self.log, first_tree_size, issued_at)?,
        };
        Ok(json!({
            "ok": true,
            "schemaVersion": MOCK_SCHEMA_VERSION,
            "authorityProvenance": MOCK_PROVENANCE,
            "mock": true,
            "productionEligible": false,
            "treeSize": self.log.tree_size(),
            "response": response
        }))
    }
}

fn consistency(
    log: &SecureMeshKtLog,
    first_tree_size: Option<u64>,
    issued_at: u64,
) -> Result<Option<licoup_native::core::secure_mesh_transparency::SecureMeshKtConsistencyProof>> {
    let Some(first_tree_size) = first_tree_size else {
        return Ok(None);
    };
    ensure!(
        first_tree_size <= log.tree_size(),
        "first tree size is ahead of the mock log"
    );
    if first_tree_size == log.tree_size() {
        return Ok(None);
    }
    Ok(Some(log.consistency_proof_at(first_tree_size, issued_at)?))
}

fn issued_at(request: &Value) -> Result<u64> {
    optional_u64(request, "issuedAtEpochSeconds")?.map_or_else(now_epoch_seconds, Ok)
}

fn now_epoch_seconds() -> Result<u64> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())
}

fn optional_u64(value: &Value, field: &str) -> Result<Option<u64>> {
    match value.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => {
            let parsed = value
                .as_u64()
                .ok_or_else(|| anyhow!("integer field is invalid"))?;
            ensure!(
                parsed <= KT_JSON_SAFE_INTEGER_MAX,
                "integer field exceeds cross-language safe range"
            );
            Ok(Some(parsed))
        }
    }
}

fn required_text<'a>(value: &'a Value, field: &str) -> Result<&'a str> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("text field is required"))
}

fn rejected(code: &str) -> Value {
    json!({
        "ok": false,
        "schemaVersion": MOCK_SCHEMA_VERSION,
        "authorityProvenance": MOCK_PROVENANCE,
        "mock": true,
        "productionEligible": false,
        "code": code,
        "detailRedacted": true
    })
}

fn write_line(value: &Value) -> Result<()> {
    let stdout = io::stdout();
    let mut lock = stdout.lock();
    serde_json::to_writer(&mut lock, value)?;
    lock.write_all(b"\n")?;
    lock.flush()?;
    Ok(())
}

#[derive(Debug, Eq, PartialEq)]
enum BoundedLine {
    Line(Vec<u8>),
    Oversized,
    Eof,
}

fn read_bounded_line<R: BufRead>(reader: &mut R, max_bytes: usize) -> Result<BoundedLine> {
    let mut output = Vec::with_capacity(max_bytes.min(8 * 1024));
    let mut oversized = false;
    loop {
        let buffer = reader.fill_buf()?;
        if buffer.is_empty() {
            return if output.is_empty() && !oversized {
                Ok(BoundedLine::Eof)
            } else if oversized {
                Ok(BoundedLine::Oversized)
            } else {
                Ok(BoundedLine::Line(output))
            };
        }
        let newline = buffer.iter().position(|byte| *byte == b'\n');
        let consumed = newline.map_or(buffer.len(), |index| index + 1);
        let content_len = newline.unwrap_or(buffer.len());
        if !oversized {
            if output.len().saturating_add(content_len) > max_bytes {
                oversized = true;
                output.clear();
            } else {
                output.extend_from_slice(&buffer[..content_len]);
            }
        }
        reader.consume(consumed);
        if newline.is_some() {
            return if oversized {
                Ok(BoundedLine::Oversized)
            } else {
                Ok(BoundedLine::Line(output))
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn oversized_line_is_discarded_without_losing_the_next_request() {
        let mut bytes = vec![b'x'; MAX_REQUEST_BYTES + 1];
        bytes.extend_from_slice(b"\n{\"action\":\"report\"}\n");
        let mut input = Cursor::new(bytes);

        assert_eq!(
            read_bounded_line(&mut input, MAX_REQUEST_BYTES).unwrap(),
            BoundedLine::Oversized
        );
        assert_eq!(
            read_bounded_line(&mut input, MAX_REQUEST_BYTES).unwrap(),
            BoundedLine::Line(br#"{"action":"report"}"#.to_vec())
        );
    }

    #[test]
    fn mock_protocol_rejects_cross_language_unsafe_integers() {
        let request = json!({"firstTreeSize": KT_JSON_SAFE_INTEGER_MAX + 1});
        assert!(optional_u64(&request, "firstTreeSize").is_err());
        let request = json!({"issuedAtEpochSeconds": KT_JSON_SAFE_INTEGER_MAX + 1});
        assert!(optional_u64(&request, "issuedAtEpochSeconds").is_err());
    }
}
