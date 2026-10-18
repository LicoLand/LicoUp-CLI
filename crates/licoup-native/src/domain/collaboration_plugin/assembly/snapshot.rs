use anyhow::{Result, anyhow, ensure};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::Path;

use super::model::LocalAssemblyRecord;
use super::{ASSEMBLY_MANIFEST_FILE, ASSEMBLY_SNAPSHOT_FILE};
use crate::domain::collaboration_plugin::package::{
    SelectedPayloadFile, SelectedServerRunner, read_file_no_follow,
};

const SNAPSHOT_MAGIC: &[u8] = b"LICOUP-SEALED-LOCAL-SERVER-SNAPSHOT-V1\0";
pub(super) const MAX_SNAPSHOT_BYTES: usize = 256 * 1024 * 1024;

pub(super) fn build(
    payload: &[SelectedPayloadFile],
    runner: &SelectedServerRunner,
    runner_destination_relative_path: &str,
    manifest: &[u8],
) -> Result<Vec<u8>> {
    let mut entries = payload
        .iter()
        .map(|file| {
            Ok((
                normalized_path(&file.destination_relative_path)?,
                file.bytes.as_slice(),
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    entries.push((ASSEMBLY_MANIFEST_FILE.to_owned(), manifest));
    entries.push((
        normalized_path(Path::new(runner_destination_relative_path))?,
        runner.bytes.as_slice(),
    ));
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    ensure!(
        entries.windows(2).all(|pair| pair[0].0 < pair[1].0),
        "collaboration_local_server_snapshot_path_conflict"
    );

    let mut output = Vec::new();
    output.extend_from_slice(SNAPSHOT_MAGIC);
    append_u32(&mut output, entries.len())?;
    for (path, bytes) in entries {
        append_u32(&mut output, path.len())?;
        output.extend_from_slice(path.as_bytes());
        output.extend_from_slice(
            &u64::try_from(bytes.len())
                .map_err(|_| anyhow!("collaboration_local_server_snapshot_too_large"))?
                .to_be_bytes(),
        );
        output.extend_from_slice(bytes);
        ensure!(
            output.len() <= MAX_SNAPSHOT_BYTES,
            "collaboration_local_server_snapshot_too_large"
        );
    }
    Ok(output)
}

pub(super) fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub(super) fn destination_digest(path: &Path) -> Result<String> {
    ensure!(
        path.is_absolute(),
        "collaboration_local_server_destination_invalid"
    );
    let text = path
        .to_str()
        .ok_or_else(|| anyhow!("collaboration_local_server_destination_encoding_invalid"))?;
    let mut hasher = Sha256::new();
    hasher.update(b"LICOUP-LOCAL-SERVER-DESTINATION-V1\0");
    hasher.update((text.len() as u64).to_be_bytes());
    hasher.update(text.as_bytes());
    Ok(format!("{:x}", hasher.finalize()))
}

pub(super) fn verify(record: &LocalAssemblyRecord) -> Result<()> {
    let root = Path::new(&record.destination);
    let path = root.join(ASSEMBLY_SNAPSHOT_FILE);
    let bytes = read_file_no_follow(&path, record.sealed_snapshot_bytes)?;
    ensure!(
        bytes.len() == record.sealed_snapshot_bytes
            && digest(&bytes) == record.sealed_snapshot_digest_sha256,
        "collaboration_local_server_snapshot_digest_mismatch"
    );
    let entries = parse(&bytes)?;
    ensure!(
        entries.len() == record.selected_payload_files.len().saturating_add(2),
        "collaboration_local_server_snapshot_inventory_mismatch"
    );
    let expected = record
        .selected_payload_files
        .iter()
        .map(|file| {
            (
                file.destination_relative_path.as_str(),
                file.digest_sha256.as_str(),
            )
        })
        .chain([
            (
                ASSEMBLY_MANIFEST_FILE,
                record.manifest_digest_sha256.as_str(),
            ),
            (
                record.runner_destination_relative_path.as_str(),
                record.runner_digest_sha256.as_str(),
            ),
        ])
        .collect::<BTreeMap<_, _>>();
    ensure!(
        entries.iter().all(|(path, payload)| {
            expected
                .get(path.as_str())
                .is_some_and(|digest| **digest == format!("{:x}", Sha256::digest(payload)))
        }),
        "collaboration_local_server_snapshot_inventory_mismatch"
    );
    Ok(())
}

fn parse(bytes: &[u8]) -> Result<Vec<(String, Vec<u8>)>> {
    ensure!(
        bytes.starts_with(SNAPSHOT_MAGIC),
        "collaboration_local_server_snapshot_invalid"
    );
    let mut offset = SNAPSHOT_MAGIC.len();
    let count = read_u32(bytes, &mut offset)? as usize;
    ensure!(
        (2..=258).contains(&count),
        "collaboration_local_server_snapshot_invalid"
    );
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        let path_len = read_u32(bytes, &mut offset)? as usize;
        ensure!(
            (1..=4096).contains(&path_len),
            "collaboration_local_server_snapshot_invalid"
        );
        let path_end = offset
            .checked_add(path_len)
            .ok_or_else(|| anyhow!("collaboration_local_server_snapshot_invalid"))?;
        let path = std::str::from_utf8(
            bytes
                .get(offset..path_end)
                .ok_or_else(|| anyhow!("collaboration_local_server_snapshot_invalid"))?,
        )
        .map_err(|_| anyhow!("collaboration_local_server_snapshot_invalid"))?
        .to_owned();
        ensure!(
            normalized_path(Path::new(&path))? == path,
            "collaboration_local_server_snapshot_invalid"
        );
        offset = path_end;
        let payload_len = read_u64(bytes, &mut offset)?;
        let payload_len = usize::try_from(payload_len)
            .map_err(|_| anyhow!("collaboration_local_server_snapshot_invalid"))?;
        let payload_end = offset
            .checked_add(payload_len)
            .ok_or_else(|| anyhow!("collaboration_local_server_snapshot_invalid"))?;
        let payload = bytes
            .get(offset..payload_end)
            .ok_or_else(|| anyhow!("collaboration_local_server_snapshot_invalid"))?
            .to_vec();
        entries.push((path, payload));
        offset = payload_end;
    }
    ensure!(
        offset == bytes.len() && entries.windows(2).all(|pair| pair[0].0 < pair[1].0),
        "collaboration_local_server_snapshot_invalid"
    );
    Ok(entries)
}

fn normalized_path(path: &Path) -> Result<String> {
    crate::domain::collaboration_plugin::manifest::normalized_relative_protocol_path(path)
}

fn append_u32(output: &mut Vec<u8>, value: usize) -> Result<()> {
    output.extend_from_slice(
        &u32::try_from(value)
            .map_err(|_| anyhow!("collaboration_local_server_snapshot_too_large"))?
            .to_be_bytes(),
    );
    Ok(())
}

fn read_u32(bytes: &[u8], offset: &mut usize) -> Result<u32> {
    let end = offset
        .checked_add(4)
        .ok_or_else(|| anyhow!("collaboration_local_server_snapshot_invalid"))?;
    let value = u32::from_be_bytes(
        bytes
            .get(*offset..end)
            .ok_or_else(|| anyhow!("collaboration_local_server_snapshot_invalid"))?
            .try_into()
            .map_err(|_| anyhow!("collaboration_local_server_snapshot_invalid"))?,
    );
    *offset = end;
    Ok(value)
}

fn read_u64(bytes: &[u8], offset: &mut usize) -> Result<u64> {
    let end = offset
        .checked_add(8)
        .ok_or_else(|| anyhow!("collaboration_local_server_snapshot_invalid"))?;
    let value = u64::from_be_bytes(
        bytes
            .get(*offset..end)
            .ok_or_else(|| anyhow!("collaboration_local_server_snapshot_invalid"))?
            .try_into()
            .map_err(|_| anyhow!("collaboration_local_server_snapshot_invalid"))?,
    );
    *offset = end;
    Ok(value)
}
