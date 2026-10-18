use anyhow::{Result, ensure};
use serde_json::Value;
use std::io::Read;

const MAX_BYTES: usize = 16 * 1024 * 1024;

pub(super) fn materialize_private_stdin_json<R: Read>(
    mut args: Vec<String>,
    reader: R,
) -> Result<Vec<String>> {
    let sentinels = args
        .windows(2)
        .enumerate()
        .filter_map(|(index, pair)| (pair == ["--stdin-json", "true"]).then_some(index + 1))
        .collect::<Vec<_>>();
    if sentinels.is_empty() {
        return Ok(args);
    }
    ensure!(sentinels.len() == 1, "private_stdin_json_marker_ambiguous");
    let mut bytes = Vec::new();
    reader
        .take((MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    ensure!(
        !bytes.is_empty() && bytes.len() <= MAX_BYTES,
        "private_stdin_json_size_invalid"
    );
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|_| anyhow::anyhow!("private_stdin_json_invalid"))?;
    ensure!(value.is_object(), "private_stdin_json_object_required");
    args[sentinels[0]] = serde_json::to_string(&value)?;
    Ok(args)
}
