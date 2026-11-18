use anyhow::{Result, anyhow, ensure};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::borrow::Cow;

use super::super::manifest::MANIFEST_FILE;
use super::PackageFile;

pub(in crate::domain::collaboration_plugin) fn signed_inventory_digest(
    files: &[PackageFile],
) -> Result<String> {
    let mut inventory = Sha256::new();
    inventory.update(b"LICOUP-SIGNED-PACKAGE-INVENTORY-V1\0");
    for file in files {
        let relative =
            super::super::manifest::normalized_relative_protocol_path(&file.relative_path)?;
        let bytes = if relative == MANIFEST_FILE {
            Cow::Owned(canonical_unsigned_manifest(&file.bytes)?)
        } else {
            Cow::Borrowed(file.bytes.as_slice())
        };
        let digest = Sha256::digest(bytes.as_ref());
        inventory.update((relative.len() as u64).to_be_bytes());
        inventory.update(relative.as_bytes());
        inventory.update((bytes.len() as u64).to_be_bytes());
        inventory.update(digest);
    }
    Ok(format!("{:x}", inventory.finalize()))
}

fn canonical_unsigned_manifest(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut value: Value = serde_json::from_slice(bytes)
        .map_err(|_| anyhow!("collaboration_plugin_manifest_invalid"))?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| anyhow!("collaboration_plugin_manifest_invalid"))?;
    object.remove("signedPackageInventoryDigestSha256");
    let runners = object
        .get_mut("serverRunners")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| anyhow!("collaboration_plugin_server_runner_contract_invalid"))?;
    for runner in runners {
        ensure!(
            runner
                .as_object_mut()
                .and_then(|runner| runner.remove("signatureBase64url"))
                .is_some(),
            "collaboration_plugin_server_runner_signature_invalid"
        );
    }
    let mut output = Vec::new();
    write_canonical_json(&value, &mut output)?;
    Ok(output)
}

fn write_canonical_json(value: &Value, output: &mut Vec<u8>) -> Result<()> {
    match value {
        Value::Null => output.extend_from_slice(b"null"),
        Value::Bool(value) => output.extend_from_slice(if *value { b"true" } else { b"false" }),
        Value::Number(value) => output.extend_from_slice(value.to_string().as_bytes()),
        Value::String(value) => output.extend_from_slice(serde_json::to_string(value)?.as_bytes()),
        Value::Array(values) => {
            output.push(b'[');
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    output.push(b',');
                }
                write_canonical_json(value, output)?;
            }
            output.push(b']');
        }
        Value::Object(values) => {
            output.push(b'{');
            let mut keys = values.keys().collect::<Vec<_>>();
            keys.sort();
            for (index, key) in keys.iter().enumerate() {
                if index > 0 {
                    output.push(b',');
                }
                output.extend_from_slice(serde_json::to_string(key)?.as_bytes());
                output.push(b':');
                write_canonical_json(&values[*key], output)?;
            }
            output.push(b'}');
        }
    }
    Ok(())
}
