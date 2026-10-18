use std::{
    fs,
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};

use anyhow::{Context, Result, ensure};

use crate::domain::client_update::constants::UPDATE_COPY_BUFFER_BYTES;

pub(super) fn copy_remaining_bytes(
    source: &Path,
    destination: &Path,
    offset: u64,
    expected_size: u64,
) -> Result<()> {
    let mut source_file = fs::File::open(source).context("failed to open client update source")?;
    source_file.seek(SeekFrom::Start(offset))?;
    let mut options = fs::OpenOptions::new();
    options.write(true).append(true);
    if destination.exists() {
        options.create(false);
    } else {
        options.create_new(true);
    }
    let mut destination_file = options
        .open(destination)
        .context("failed to open partial client update artifact")?;
    let mut written = offset;
    let mut buffer = [0_u8; UPDATE_COPY_BUFFER_BYTES];
    while written < expected_size {
        let remaining = (expected_size - written).min(buffer.len() as u64) as usize;
        let read = source_file
            .read(&mut buffer[..remaining])
            .context("failed to read client update source")?;
        ensure!(read > 0, "client update source artifact is truncated");
        destination_file
            .write_all(&buffer[..read])
            .context("failed to stage client update artifact")?;
        written = written
            .checked_add(read as u64)
            .context("client update staged size overflow")?;
    }
    destination_file
        .sync_all()
        .context("failed to persist partial client update artifact")?;
    Ok(())
}
