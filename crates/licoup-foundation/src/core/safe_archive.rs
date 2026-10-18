//! Safe archive extraction — streaming, bounded, and no-follow.
//!
//! Replaces the system `tar` subprocess with a Rust-native extractor that
//! enforces path traversal rejection, entry type allowlisting, and
//! configurable byte / entry / depth limits.

use std::cell::Cell;
use std::ffi::CString;
use std::fs::{self, OpenOptions};
use std::io::{BufReader, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;

#[cfg(unix)]
use std::os::fd::{AsRawFd, FromRawFd};
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;

use anyhow::{Context, Result, anyhow, ensure};
use flate2::bufread::GzDecoder;
use tar::{Archive, EntryType};
use zip::ZipArchive;

mod zip_structure;
pub(crate) use zip_structure::validate_zip_structure;

/// Default maximum total bytes extracted across all entries.
const DEFAULT_MAX_TOTAL_BYTES: u64 = 512 * 1024 * 1024; // 512 MiB

/// Default maximum number of entries.
const DEFAULT_MAX_ENTRIES: usize = 10_000;

/// Default maximum directory depth relative to destination root.
const DEFAULT_MAX_DEPTH: usize = 32;

/// One TAR header block.
const TAR_BLOCK_BYTES: u64 = 512;

/// Longest portable single path component; longer metadata is not portable anyway.
const PORTABLE_NAME_BYTES: u64 = 255;

/// The most decoded bytes a `.tar.gz` may carry: the declared payload plus the
/// format's own header blocks, padding and worst-case portable long-name metadata.
///
/// This bounds the physical decoded stream before the TAR library can buffer GNU or
/// PAX extension metadata, derived from the existing limits rather than a new cap.
pub(crate) fn decoded_tar_gz_budget(
    max_total_bytes: u64,
    max_entries: usize,
    max_depth: usize,
) -> u64 {
    max_total_bytes
        .saturating_add(TAR_BLOCK_BYTES * 2)
        .saturating_add(
            (max_entries as u64).saturating_mul(decoded_tar_gz_metadata_budget(max_depth)),
        )
}

/// The most decoded metadata one member may legally carry: its path twice (an extension
/// header and the entry header), block padded, plus the fixed header blocks.
///
/// Applied per advance as a pre-yield bound, so a single GNU long-name or PAX record
/// cannot consume the whole archive budget before the entry is admitted.
pub(crate) fn decoded_tar_gz_metadata_budget(max_depth: usize) -> u64 {
    (max_depth as u64)
        .saturating_mul(PORTABLE_NAME_BYTES.saturating_add(1))
        .div_ceil(TAR_BLOCK_BYTES)
        .saturating_mul(TAR_BLOCK_BYTES)
        .saturating_add(TAR_BLOCK_BYTES.saturating_mul(3))
}

/// A reader that refuses to hand out more than its budget and remembers how much it did.
///
/// Both the manifest scan and extraction run through it, so no pass can inflate the
/// archive past its policy before the TAR library buffers a single member. A detached
/// progress handle lets a pass measure one library advance while the archive borrows the
/// stream.
pub(crate) struct BoundedStream<R> {
    inner: R,
    remaining: u64,
    consumed: Rc<Cell<u64>>,
    exceeded: bool,
}

impl<R> BoundedStream<R> {
    pub(crate) fn new(inner: R, limit: u64) -> Self {
        Self {
            inner,
            remaining: limit,
            consumed: Rc::new(Cell::new(0)),
            exceeded: false,
        }
    }

    pub(crate) fn progress(&self) -> StreamProgress {
        StreamProgress(self.consumed.clone())
    }

    pub(crate) fn exceeded(&self) -> bool {
        self.exceeded
    }

    pub(crate) fn into_inner(self) -> R {
        self.inner
    }
}

/// Detached view of how many decoded bytes a [`BoundedStream`] has handed out.
pub(crate) struct StreamProgress(Rc<Cell<u64>>);

impl StreamProgress {
    pub(crate) fn consumed(&self) -> u64 {
        self.0.get()
    }
}

/// The decoded reader both TAR passes share: a single-member GZIP decoder over the
/// in-memory archive, so completion can be validated and trailing material detected.
pub(crate) type TarGzDecoder<'a> = GzDecoder<BufReader<std::io::Cursor<&'a [u8]>>>;

pub(crate) fn tar_gz_decoder(bytes: &[u8]) -> TarGzDecoder<'_> {
    GzDecoder::new(BufReader::new(std::io::Cursor::new(bytes)))
}

/// Consume the rest of a decoded `.tar.gz` stream to its valid end.
///
/// The TAR end padding and the GZIP trailer are read here, so the trailer's CRC and
/// ISIZE are verified and a truncated or corrupted stream is refused instead of
/// stopping at the first zero block. Compressed material after the single GZIP member
/// is refused as well, using the decoder's own buffered remainder.
pub(crate) fn finish_tar_gz(
    mut stream: BoundedStream<TarGzDecoder<'_>>,
    bytes_len: usize,
) -> Result<()> {
    std::io::copy(&mut stream, &mut std::io::sink())
        .map_err(|_| anyhow!("archive did not end cleanly"))?;
    let reader = stream.into_inner().into_inner();
    let trailing = (reader.buffer().len() as u64)
        .saturating_add((bytes_len as u64).saturating_sub(reader.get_ref().position()));
    ensure!(trailing == 0, "archive has trailing compressed material");
    Ok(())
}

/// Admit every raw TAR header before the pinned TAR library can buffer anything.
///
/// The scanner reads the decoded stream block by block, verifies each header checksum,
/// parses the size field, and enforces the derived policy on raw metadata, member type
/// and member size. GNU long-name and PAX extension bodies are bounded by the derived
/// per-member metadata budget and consumed here, so a hostile header cannot make the
/// library allocate before the policy refuses it. Sparse and every other unsupported
/// type is refused here, before the library builds descriptor vectors.
///
/// After the zero terminator every remaining decoded byte must be zero TAR padding, so a
/// member hidden behind the terminator is refused instead of silently discarded.
pub(crate) fn validate_tar_gz_structure(
    bytes: &[u8],
    max_total_bytes: u64,
    max_entries: usize,
    max_depth: usize,
) -> Result<()> {
    let budget = decoded_tar_gz_budget(max_total_bytes, max_entries, max_depth);
    let metadata_budget = decoded_tar_gz_metadata_budget(max_depth);
    let mut stream = BoundedStream::new(tar_gz_decoder(bytes), budget);
    let mut entry_count = 0_usize;
    let mut total_bytes = 0_u64;
    let mut metadata_bytes = 0_u64;
    let mut pax_size = None;
    let mut has_pax = false;
    let mut has_long_name = false;
    loop {
        let mut block = [0_u8; TAR_BLOCK_BYTES as usize];
        read_fully(&mut stream, &mut block)?;
        if block.iter().all(|byte| *byte == 0) {
            ensure!(!has_pax && !has_long_name, "archive has orphaned metadata");
            // End of archive: everything left in the decoded stream must be zero padding.
            let mut padding = [0_u8; TAR_BLOCK_BYTES as usize];
            loop {
                let read = stream.read(&mut padding)?;
                if read == 0 {
                    return finish_tar_gz(stream, bytes.len());
                }
                ensure!(
                    padding[..read].iter().all(|byte| *byte == 0),
                    "archive has material after its terminator"
                );
            }
        }
        let header = RawTarHeader::parse(&block)?;
        match header.typeflag {
            b'L' | b'x' => {
                metadata_bytes = metadata_bytes
                    .saturating_add(TAR_BLOCK_BYTES)
                    .saturating_add(
                        header
                            .size
                            .div_ceil(TAR_BLOCK_BYTES)
                            .saturating_mul(TAR_BLOCK_BYTES),
                    );
                ensure!(
                    metadata_bytes.saturating_add(TAR_BLOCK_BYTES) <= metadata_budget,
                    "archive metadata exceeds the portable header budget"
                );
                if header.typeflag == b'x' {
                    ensure!(!has_pax, "archive has duplicate PAX metadata");
                    has_pax = true;
                    let mut body = vec![0; header.size as usize];
                    read_fully(&mut stream, &mut body)?;
                    // Use the pinned parser's record grammar and first-size semantics,
                    // but refuse ambiguous or malformed records instead of ignoring them.
                    for extension in tar::PaxExtensions::new(&body) {
                        let extension = extension?;
                        let key = extension.key()?;
                        ensure!(
                            !key.starts_with("GNU.sparse."),
                            "archive entry has unsupported type"
                        );
                        if key == "size" {
                            ensure!(pax_size.is_none(), "archive has duplicate PAX size");
                            pax_size = Some(extension.value()?.parse::<u64>()?);
                        }
                    }
                    let padding =
                        (TAR_BLOCK_BYTES - header.size % TAR_BLOCK_BYTES) % TAR_BLOCK_BYTES;
                    read_fully(&mut stream, &mut [0; 512][..padding as usize])?;
                } else {
                    ensure!(!has_long_name, "archive has duplicate long-name metadata");
                    has_long_name = true;
                    skip_fully(&mut stream, header.size)?;
                }
            }
            b'0' | 0 | b'5' => {
                let size = pax_size.take().unwrap_or(header.size);
                if header.typeflag == b'5' {
                    ensure!(size == 0, "archive directory entry declares a body");
                }
                total_bytes = total_bytes
                    .checked_add(size)
                    .ok_or_else(|| anyhow!("archive byte count overflowed"))?;
                ensure!(
                    total_bytes <= max_total_bytes,
                    "archive byte limit exceeded"
                );
                entry_count += 1;
                ensure!(
                    entry_count <= max_entries,
                    "archive entry count exceeds maximum"
                );
                skip_fully(&mut stream, size)?;
                metadata_bytes = 0;
                has_pax = false;
                has_long_name = false;
            }
            _ => return Err(anyhow!("archive entry has unsupported type")),
        }
    }
}

/// One raw TAR header: the checksum-verified size and type fields.
struct RawTarHeader {
    size: u64,
    typeflag: u8,
}

impl RawTarHeader {
    fn parse(block: &[u8; TAR_BLOCK_BYTES as usize]) -> Result<Self> {
        let stored = parse_tar_number(&block[148..156])?;
        let mut unsigned = 0_u64;
        let mut signed = 0_i64;
        for (index, byte) in block.iter().enumerate() {
            let value = if (148..156).contains(&index) {
                b' '
            } else {
                *byte
            };
            unsigned = unsigned.saturating_add(u64::from(value));
            signed = signed.saturating_add(i64::from(value as i8));
        }
        ensure!(
            stored == unsigned || (signed >= 0 && stored == signed as u64),
            "archive header checksum does not match"
        );
        let size = parse_tar_number(&block[124..136])?;
        Ok(Self {
            size,
            typeflag: block[156],
        })
    }
}

/// Parse one TAR numeric field: NUL/space padded octal, or GNU base-256.
fn parse_tar_number(field: &[u8]) -> Result<u64> {
    if field.first().is_some_and(|byte| byte & 0x80 != 0) {
        let mut value = u64::from(field[0] & 0x7F);
        for byte in &field[1..] {
            value = value
                .checked_mul(256)
                .and_then(|value| value.checked_add(u64::from(*byte)))
                .ok_or_else(|| anyhow!("archive numeric field overflowed"))?;
        }
        return Ok(value);
    }
    let text = std::str::from_utf8(field).map_err(|_| anyhow!("archive numeric field invalid"))?;
    let text = text.trim_matches(|character| character == '\0' || character == ' ');
    if text.is_empty() {
        return Ok(0);
    }
    u64::from_str_radix(text, 8).map_err(|_| anyhow!("archive numeric field invalid"))
}

fn read_fully<R: Read>(reader: &mut R, buffer: &mut [u8]) -> Result<()> {
    let mut filled = 0_usize;
    while filled < buffer.len() {
        let read = reader.read(&mut buffer[filled..])?;
        ensure!(read > 0, "archive ended before its terminator");
        filled += read;
    }
    Ok(())
}

fn skip_fully<R: Read>(reader: &mut R, length: u64) -> Result<()> {
    // Consume the member body and the block padding that follows it.
    let padding = (TAR_BLOCK_BYTES - (length % TAR_BLOCK_BYTES)) % TAR_BLOCK_BYTES;
    let mut remaining = length.saturating_add(padding);
    let mut scratch = [0_u8; 8 * 1024];
    while remaining > 0 {
        let take = remaining.min(scratch.len() as u64) as usize;
        let read = reader.read(&mut scratch[..take])?;
        ensure!(read > 0, "archive ended inside a member");
        remaining -= read as u64;
    }
    Ok(())
}

impl<R: Read> Read for BoundedStream<R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        if self.remaining == 0 {
            if self.inner.read(&mut [0])? == 0 {
                return Ok(0);
            }
            self.exceeded = true;
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "archive decoded stream limit exceeded",
            ));
        }
        let allowed = buffer
            .len()
            .min(usize::try_from(self.remaining).unwrap_or(usize::MAX));
        let read = self.inner.read(&mut buffer[..allowed])?;
        self.remaining -= read as u64;
        self.consumed
            .set(self.consumed.get().saturating_add(read as u64));
        Ok(read)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ZipExtractionLimits {
    pub max_archive_bytes: u64,
    pub max_total_bytes: u64,
    pub max_file_bytes: u64,
    pub max_entries: usize,
    pub max_depth: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ZipEntryInfo {
    pub path: PathBuf,
    pub size: u64,
    pub directory: bool,
}

/// The extractor's own defaults, exposed so callers reuse one policy rather than
/// inventing their own bounds.
pub fn default_zip_extraction_limits() -> ZipExtractionLimits {
    ZipExtractionLimits {
        max_archive_bytes: DEFAULT_MAX_TOTAL_BYTES,
        max_total_bytes: DEFAULT_MAX_TOTAL_BYTES,
        max_file_bytes: DEFAULT_MAX_TOTAL_BYTES,
        max_entries: DEFAULT_MAX_ENTRIES,
        max_depth: DEFAULT_MAX_DEPTH,
    }
}

/// Extract a ZIP from memory below one no-follow destination root.
///
/// All names must be UTF-8 POSIX-relative paths. Duplicate normalized names,
/// case-colliding names, links and special entries are rejected before a file
/// can be published outside the private staging directory.
pub fn extract_zip_safe(
    bytes: &[u8],
    destination: &Path,
    limits: ZipExtractionLimits,
) -> Result<Vec<ZipEntryInfo>> {
    ensure!(
        bytes.len() as u64 <= limits.max_archive_bytes,
        "zip_archive_byte_limit_exceeded"
    );
    ensure!(limits.max_entries > 0, "zip_entry_limit_invalid");
    validate_zip_structure(bytes, limits)?;
    let cursor = std::io::Cursor::new(bytes);
    let mut archive = ZipArchive::new(cursor).map_err(|_| anyhow!("zip_archive_invalid"))?;
    ensure!(
        archive.len() <= limits.max_entries,
        "zip_entry_count_limit_exceeded"
    );

    let extraction_root = ExtractionRoot::open(destination)?;
    let mut total_bytes = 0_u64;
    let mut exact_names = std::collections::BTreeSet::new();
    let mut folded_names = std::collections::BTreeSet::new();
    let mut result = Vec::with_capacity(archive.len());
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|_| anyhow!("zip_entry_unreadable"))?;
        let raw_name = entry.name_raw();
        let name = std::str::from_utf8(raw_name).map_err(|_| anyhow!("zip_entry_name_invalid"))?;
        ensure!(
            !name.is_empty()
                && !name.contains('\0')
                && !name.contains('\\')
                && !name.starts_with('/')
                && !name.contains(':'),
            "zip_entry_path_invalid"
        );
        let directory = entry.is_dir();
        ensure!(directory || entry.is_file(), "zip_entry_type_unsupported");
        if let Some(mode) = entry.unix_mode() {
            let file_type = mode & 0o170000;
            ensure!(
                file_type == 0
                    || (directory && file_type == 0o040000)
                    || (!directory && file_type == 0o100000),
                "zip_entry_type_unsupported"
            );
        }
        let path_name = name.trim_end_matches('/');
        ensure!(!path_name.is_empty(), "zip_entry_path_invalid");
        let relative = sanitize_entry_path(Path::new(path_name), limits.max_depth)?;
        let canonical = relative
            .iter()
            .map(|part| part.to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        ensure!(exact_names.insert(canonical.clone()), "zip_entry_duplicate");
        ensure!(
            folded_names.insert(canonical.to_lowercase()),
            "zip_entry_case_collision"
        );

        if directory {
            // A directory body is never legitimate: the member is a directory, so its
            // declared size must be zero. The stream is consumed and the checksum compared
            // as well, because a crafted central directory can claim an empty directory
            // while the member still carries a body.
            ensure!(entry.size() == 0, "zip_entry_directory_body_unsupported");
            let mut sink = std::io::sink();
            let body = std::io::copy(&mut entry, &mut sink)?;
            ensure!(body == 0, "zip_entry_directory_body_unsupported");
            ensure!(entry.crc32() == 0, "zip_entry_directory_body_unsupported");
            extraction_root.create_directory(&relative)?;
            result.push(ZipEntryInfo {
                path: relative,
                size: 0,
                directory: true,
            });
            continue;
        }
        let declared_size = entry.size();
        ensure!(
            declared_size <= limits.max_file_bytes,
            "zip_file_byte_limit_exceeded"
        );
        let next_total = total_bytes
            .checked_add(declared_size)
            .ok_or_else(|| anyhow!("zip_total_byte_count_overflowed"))?;
        ensure!(
            next_total <= limits.max_total_bytes,
            "zip_total_byte_limit_exceeded"
        );
        let mut output = extraction_root.create_file(&relative)?;
        let mut bounded = (&mut entry).take(declared_size.saturating_add(1));
        let written = std::io::copy(&mut bounded, &mut output)?;
        ensure!(written == declared_size, "zip_entry_size_mismatch");
        output.flush()?;
        output.sync_all()?;
        total_bytes = next_total;
        result.push(ZipEntryInfo {
            path: relative,
            size: declared_size,
            directory: false,
        });
    }
    Ok(result)
}

/// Extract a `.tar.gz` byte slice to `destination` with safety bounds.
///
/// Rejects:
/// - Path traversal components (`../`, absolute paths).
/// - Special entries (device nodes, FIFOs, hard links, symlinks to
///   external paths).
/// - Archives exceeding `max_total_bytes`, `max_entries`, or
///   `max_depth`.
pub fn extract_tar_gz_safe(
    bytes: &[u8],
    destination: &Path,
    max_total_bytes: Option<u64>,
    max_entries: Option<usize>,
    max_depth: Option<usize>,
) -> Result<()> {
    let max_total_bytes = max_total_bytes.unwrap_or(DEFAULT_MAX_TOTAL_BYTES);
    let max_entries = max_entries.unwrap_or(DEFAULT_MAX_ENTRIES);
    let max_depth = max_depth.unwrap_or(DEFAULT_MAX_DEPTH);

    // Raw admission first: every header, type and metadata size is bounded before the
    // TAR library parses the stream, so no GNU/PAX body or sparse descriptor can be
    // buffered ahead of the policy refusal.
    validate_tar_gz_structure(bytes, max_total_bytes, max_entries, max_depth)?;

    let budget = decoded_tar_gz_budget(max_total_bytes, max_entries, max_depth);
    let metadata_budget = decoded_tar_gz_metadata_budget(max_depth);
    let decoder = tar_gz_decoder(bytes);
    let mut stream = BoundedStream::new(decoder, budget);
    let progress = stream.progress();

    let mut total_bytes = 0_u64;
    let mut entry_count: usize = 0;
    let extraction_root = ExtractionRoot::open(destination)?;

    {
        let mut archive = Archive::new(&mut stream);
        let mut entries = archive.entries()?;
        loop {
            let before = progress.consumed();
            let Some(entry) = entries.next() else {
                break;
            };
            // One library advance may buffer GNU long-name or PAX metadata before the
            // entry is admitted; that metadata is bounded here, before it can consume
            // the aggregate decoded budget.
            ensure!(
                progress.consumed().saturating_sub(before) <= metadata_budget,
                "archive member metadata exceeds the portable header budget"
            );
            let mut entry = entry?;
            entry_count += 1;

            ensure!(
                entry_count <= max_entries,
                "archive entry count {entry_count} exceeds maximum {max_entries}"
            );

            let entry_type = entry.header().entry_type();

            // Allow only regular files and directories.
            ensure!(
                entry_type == EntryType::Regular || entry_type == EntryType::Directory,
                "archive entry {entry_count} has unsupported type {entry_type:?}; only regular files and directories are allowed"
            );

            // Validate and sanitize the entry path.
            let entry_path = entry.path()?;
            let relative = sanitize_entry_path(&entry_path, max_depth)?;

            if entry_type == EntryType::Directory {
                // A directory body is never legitimate and would otherwise be decompressed
                // while the iterator skips it without counting against the payload total.
                ensure!(
                    entry.size() == 0,
                    "archive directory entry {entry_count} declares a body"
                );
                extraction_root.create_directory(&relative)?;
                continue;
            }

            let declared_size = entry.size();
            let next_total = total_bytes
                .checked_add(declared_size)
                .ok_or_else(|| anyhow!("archive extracted byte count overflowed"))?;
            ensure!(next_total <= max_total_bytes, "archive byte limit exceeded");
            let mut file = extraction_root.create_file(&relative)?;
            let written = std::io::copy(&mut entry, &mut file)?;

            ensure!(
                written == declared_size,
                "archive entry {entry_count} size did not match its header"
            );
            file.flush()?;
            file.sync_all()?;
            total_bytes = next_total;
        }
    }

    // Read the remainder of the decoded stream to its valid end: the TAR end padding and
    // the GZIP trailer are consumed here, so the trailer's CRC and ISIZE are verified and
    // a truncated or corrupted stream is refused instead of stopping at the first zero
    // block.
    finish_tar_gz(stream, bytes.len())?;

    Ok(())
}

/// Validate and sanitize a single entry path from the archive.
fn sanitize_entry_path(raw: &Path, max_depth: usize) -> Result<PathBuf> {
    // Reject absolute paths.
    ensure!(
        raw.is_relative(),
        "archive entry path must be relative: {}",
        raw.display()
    );

    // Reject path traversal components.
    for component in raw.components() {
        match component {
            Component::ParentDir => {
                return Err(anyhow!(
                    "archive entry contains path traversal: {}",
                    raw.display()
                ));
            }
            Component::Prefix(_) | Component::RootDir => {
                return Err(anyhow!(
                    "archive entry contains absolute or prefixed component: {}",
                    raw.display()
                ));
            }
            _ => {}
        }
    }

    // Reject empty paths or paths that normalize to empty.
    let normalized: PathBuf = raw
        .components()
        .filter(|c| !matches!(c, std::path::Component::CurDir))
        .collect();
    if normalized.as_os_str().is_empty() {
        return Err(anyhow!("archive entry path is empty after normalization"));
    }

    // Every component must be portable independently of the host platform: the same
    // rules the ZIP path enforces, so a member name can never mean a different location
    // on a different host.
    for component in normalized.iter() {
        let name = component.to_str().ok_or_else(|| {
            anyhow!(
                "archive entry path is not portable UTF-8: {}",
                normalized.display()
            )
        })?;
        ensure!(
            !name.is_empty() && name != "." && name != "..",
            "archive entry path component is not portable: {}",
            normalized.display()
        );
        ensure!(
            !name.contains('\\') && !name.contains(':'),
            "archive entry path component is not portable: {}",
            normalized.display()
        );
        ensure!(
            !name.chars().any(char::is_control),
            "archive entry path component contains a control character: {}",
            normalized.display()
        );
    }

    // Check depth.
    let depth = normalized.components().count();
    ensure!(
        depth <= max_depth,
        "archive entry depth {} exceeds maximum {}: {}",
        depth,
        max_depth,
        normalized.display()
    );

    Ok(normalized)
}

/// A root directory held open for the whole extraction. On Unix every descendant is opened
/// relative to an already-open directory descriptor with `O_NOFOLLOW`, closing the ancestor
/// symlink race that path-based `canonicalize` checks leave behind.
struct ExtractionRoot {
    #[cfg(unix)]
    directory: fs::File,
    #[cfg(not(unix))]
    path: PathBuf,
}

impl ExtractionRoot {
    fn open(path: &Path) -> Result<Self> {
        ensure!(!path.as_os_str().is_empty(), "archive destination is empty");
        #[cfg(unix)]
        {
            let mut current = if path.is_absolute() {
                open_directory(Path::new("/"))?
            } else {
                open_directory(Path::new("."))?
            };
            for component in path.components() {
                match component {
                    Component::RootDir | Component::CurDir => {}
                    Component::Normal(name) => {
                        current = open_or_create_directory_at(&current, name)?;
                    }
                    Component::ParentDir | Component::Prefix(_) => {
                        return Err(anyhow!(
                            "archive destination contains an unsafe path component"
                        ));
                    }
                }
            }
            Ok(Self { directory: current })
        }
        #[cfg(not(unix))]
        {
            create_directory_path_no_follow(path)?;
            Ok(Self {
                path: path.to_path_buf(),
            })
        }
    }

    fn create_directory(&self, relative: &Path) -> Result<()> {
        #[cfg(unix)]
        {
            let _ = self.open_parent(relative, true)?;
            Ok(())
        }
        #[cfg(not(unix))]
        {
            create_directory_path_no_follow(&self.path.join(relative))
        }
    }

    fn create_file(&self, relative: &Path) -> Result<fs::File> {
        let leaf = relative
            .file_name()
            .ok_or_else(|| anyhow!("archive file entry has no file name"))?;
        #[cfg(unix)]
        {
            let parent = self.open_parent(relative, false)?;
            create_file_at(&parent, leaf)
        }
        #[cfg(not(unix))]
        {
            let destination = self.path.join(relative);
            let parent = destination
                .parent()
                .ok_or_else(|| anyhow!("archive file entry has no parent"))?;
            create_directory_path_no_follow(parent)?;
            ensure_missing_or_regular_no_follow(&destination)?;
            OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&destination)
                .with_context(|| "archive output file could not be created")
        }
    }

    #[cfg(unix)]
    fn open_parent(&self, relative: &Path, include_leaf: bool) -> Result<fs::File> {
        let mut current = self.directory.try_clone()?;
        let component_count = relative.components().count();
        for (index, component) in relative.components().enumerate() {
            let Component::Normal(name) = component else {
                return Err(anyhow!("archive entry contains an unsafe component"));
            };
            if !include_leaf && index + 1 == component_count {
                break;
            }
            current = open_or_create_directory_at(&current, name)?;
        }
        Ok(current)
    }
}

#[cfg(unix)]
fn open_directory(path: &Path) -> Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_DIRECTORY | nix::libc::O_NOFOLLOW | nix::libc::O_CLOEXEC)
        .open(path)
        .with_context(|| "archive destination ancestor is not a no-follow directory")
}

#[cfg(unix)]
fn component_cstring(name: &std::ffi::OsStr) -> Result<CString> {
    CString::new(name.as_bytes()).map_err(|_| anyhow!("archive path contains a NUL byte"))
}

#[cfg(unix)]
fn open_or_create_directory_at(parent: &fs::File, name: &std::ffi::OsStr) -> Result<fs::File> {
    let name = component_cstring(name)?;
    let flags =
        nix::libc::O_RDONLY | nix::libc::O_DIRECTORY | nix::libc::O_NOFOLLOW | nix::libc::O_CLOEXEC;
    let mut fd = unsafe { nix::libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) };
    if fd < 0 {
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::NotFound {
            return Err(anyhow!(
                "archive destination ancestor is not a no-follow directory"
            ));
        }
        let created = unsafe { nix::libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o700) };
        if created != 0 {
            let create_error = std::io::Error::last_os_error();
            if create_error.kind() != std::io::ErrorKind::AlreadyExists {
                return Err(anyhow!(
                    "archive destination directory could not be created"
                ));
            }
        }
        fd = unsafe { nix::libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) };
    }
    if fd < 0 {
        return Err(anyhow!(
            "archive destination ancestor changed during extraction"
        ));
    }
    Ok(unsafe { fs::File::from_raw_fd(fd) })
}

#[cfg(unix)]
fn create_file_at(parent: &fs::File, name: &std::ffi::OsStr) -> Result<fs::File> {
    let name = component_cstring(name)?;
    let flags = nix::libc::O_WRONLY
        | nix::libc::O_CREAT
        | nix::libc::O_EXCL
        | nix::libc::O_NOFOLLOW
        | nix::libc::O_CLOEXEC;
    let fd = unsafe { nix::libc::openat(parent.as_raw_fd(), name.as_ptr(), flags, 0o600) };
    if fd < 0 {
        return Err(anyhow!(
            "archive output must be a new regular file below the extraction root"
        ));
    }
    Ok(unsafe { fs::File::from_raw_fd(fd) })
}

#[cfg(not(unix))]
fn create_directory_path_no_follow(path: &Path) -> Result<()> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut current = PathBuf::new();
    for component in absolute.components() {
        match component {
            // A Windows drive or UNC prefix is not a filesystem location by itself.
            // Probing `C:` before the following root component means "the current
            // directory on drive C" and can reject an otherwise valid absolute
            // destination. Build the rooted anchor first, then validate it and every
            // descendant without following links.
            Component::Prefix(_) => {
                current.push(component.as_os_str());
                continue;
            }
            Component::RootDir | Component::Normal(_) => {
                current.push(component.as_os_str());
            }
            Component::CurDir => continue,
            Component::ParentDir => {
                return Err(anyhow!("archive path contains a parent component"));
            }
        }
        match fs::symlink_metadata(&current) {
            Ok(metadata) => ensure!(
                metadata.is_dir() && !metadata.file_type().is_symlink(),
                "archive destination ancestor is not a no-follow directory"
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&current)?;
                ensure_missing_or_regular_no_follow(&current)?;
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

#[cfg(not(unix))]
fn ensure_missing_or_regular_no_follow(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => ensure!(
            !metadata.file_type().is_symlink(),
            "archive output path is a symbolic link"
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn temp_dir() -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("lico-safe-archive-test-{id}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir.canonicalize().unwrap()
    }

    fn create_test_tar_gz(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut tar_buf = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_buf);
            for (path, data) in entries {
                let mut header = tar::Header::new_gnu();
                header.set_path(path).unwrap();
                header.set_size(data.len() as u64);
                header.set_mode(0o644);
                header.set_cksum();
                builder.append(&header, &data[..]).unwrap();
            }
            builder.finish().unwrap();
        }
        let mut gz_buf = Vec::new();
        {
            let mut encoder =
                flate2::write::GzEncoder::new(&mut gz_buf, flate2::Compression::default());
            encoder.write_all(&tar_buf).unwrap();
            encoder.finish().unwrap();
        }
        gz_buf
    }

    fn create_test_zip(entries: &[(&str, &[u8], u32)]) -> Vec<u8> {
        let mut cursor = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut cursor);
            for (path, data, mode) in entries {
                let options = zip::write::SimpleFileOptions::default().unix_permissions(*mode);
                writer.start_file(path, options).unwrap();
                writer.write_all(data).unwrap();
            }
            writer.finish().unwrap();
        }
        cursor.into_inner()
    }

    fn create_test_zip_symlink(path: &str, target: &str) -> Vec<u8> {
        let mut cursor = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut cursor);
            writer
                .add_symlink(path, target, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.finish().unwrap();
        }
        cursor.into_inner()
    }

    fn zip_limits() -> ZipExtractionLimits {
        ZipExtractionLimits {
            max_archive_bytes: 1024 * 1024,
            max_total_bytes: 1024 * 1024,
            max_file_bytes: 128 * 1024,
            max_entries: 16,
            max_depth: 4,
        }
    }

    #[test]
    fn zip_rejects_traversal_case_collisions_and_links() {
        for archive in [
            create_test_zip(&[("../outside", b"blocked", 0o600)]),
            create_test_zip(&[
                ("scripts/check.py", b"one", 0o600),
                ("Scripts/check.py", b"two", 0o600),
            ]),
            create_test_zip_symlink("scripts/link", "target"),
        ] {
            let temp = temp_dir();
            let destination = temp.join("zip-out");
            assert!(extract_zip_safe(&archive, &destination, zip_limits()).is_err());
            assert!(!temp.join("outside").exists());
        }
    }

    /// `tar::Header::set_path` rejects hostile paths itself. Writing the raw name field ensures
    /// these fixtures reach the production extractor instead of failing in setup.
    fn create_test_tar_gz_with_raw_path(path: &[u8], data: &[u8]) -> Vec<u8> {
        assert!(path.len() < 100);
        let mut tar_buf = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_buf);
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o600);
            header.set_entry_type(EntryType::Regular);
            let bytes = header.as_mut_bytes();
            bytes[..100].fill(0);
            bytes[..path.len()].copy_from_slice(path);
            header.set_cksum();
            builder.append(&header, data).unwrap();
            builder.finish().unwrap();
        }
        let mut gz_buf = Vec::new();
        {
            let mut encoder =
                flate2::write::GzEncoder::new(&mut gz_buf, flate2::Compression::default());
            encoder.write_all(&tar_buf).unwrap();
            encoder.finish().unwrap();
        }
        gz_buf
    }

    #[test]
    fn safe_extract_regular_files() {
        let temp = temp_dir();
        let dest = temp.join("out");
        let archive = create_test_tar_gz(&[
            ("hello.txt", b"hello world"),
            ("sub/deep.txt", b"deep content"),
        ]);
        extract_tar_gz_safe(&archive, &dest, None, None, None).unwrap();
        assert!(dest.join("hello.txt").exists());
        assert!(dest.join("sub").join("deep.txt").exists());
        assert_eq!(
            fs::read_to_string(dest.join("hello.txt")).unwrap(),
            "hello world"
        );
    }

    #[test]
    fn rejects_path_traversal() {
        let temp = temp_dir();
        let dest = temp.join("out");
        let archive = create_test_tar_gz_with_raw_path(b"../outside.txt", b"evil");
        let result = extract_tar_gz_safe(&archive, &dest, None, None, None);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("path traversal"));
    }

    #[test]
    fn rejects_absolute_path() {
        let temp = temp_dir();
        let dest = temp.join("out");
        let absolute_path = ["/", "etc", "/", "passwd"].concat();
        let archive = create_test_tar_gz_with_raw_path(absolute_path.as_bytes(), b"evil");
        let result = extract_tar_gz_safe(&archive, &dest, None, None, None);
        assert!(result.is_err());
    }

    #[test]
    fn rejects_symlink_to_external() {
        let temp = temp_dir();
        let dest = temp.join("out");
        // Create a tar entry that is a symlink.
        let mut tar_buf = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_buf);
            let mut header = tar::Header::new_gnu();
            let absolute_path = ["/", "etc", "/", "passwd"].concat();
            header.set_path("link").unwrap();
            header.set_size(0);
            header.set_entry_type(EntryType::Symlink);
            header.set_link_name(absolute_path).unwrap();
            header.set_cksum();
            builder.append(&header, &[][..]).unwrap();
            builder.finish().unwrap();
        }
        let mut gz_buf = Vec::new();
        {
            let mut encoder =
                flate2::write::GzEncoder::new(&mut gz_buf, flate2::Compression::default());
            encoder.write_all(&tar_buf).unwrap();
            encoder.finish().unwrap();
        }
        let result = extract_tar_gz_safe(&gz_buf, &dest, None, None, None);
        assert!(result.is_err());
    }

    #[test]
    fn enforces_byte_limit() {
        let temp = temp_dir();
        let dest = temp.join("out");
        let archive = create_test_tar_gz(&[("big.txt", &[b'x'; 1024])]);
        let result = extract_tar_gz_safe(&archive, &dest, Some(100), None, None);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("limit exceeded"));
    }

    #[test]
    fn enforces_entry_limit() {
        let temp = temp_dir();
        let dest = temp.join("out");
        let entries: Vec<_> = (0..10)
            .map(|i| {
                (
                    format!("file_{i}.txt").leak() as &str,
                    &b"data"[..] as &[u8],
                )
            })
            .collect();
        let archive = create_test_tar_gz(&entries);
        let result = extract_tar_gz_safe(&archive, &dest, None, Some(5), None);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("entry count"));
    }

    #[test]
    fn enforces_depth_limit() {
        let temp = temp_dir();
        let dest = temp.join("out");
        let archive = create_test_tar_gz(&[("a/b/c/d/e/f/g/h/i/j/k/file.txt", b"deep")]);
        let result = extract_tar_gz_safe(&archive, &dest, None, None, Some(3));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("depth"));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_preexisting_symlink_ancestor_below_destination() {
        use std::os::unix::fs::symlink;

        let temp = temp_dir();
        let dest = temp.join("out");
        let external = temp.join("external");
        fs::create_dir_all(&dest).unwrap();
        fs::create_dir_all(&external).unwrap();
        symlink(&external, dest.join("sub")).unwrap();
        let archive = create_test_tar_gz(&[("sub/escaped.txt", b"blocked")]);

        let result = extract_tar_gz_safe(&archive, &dest, None, None, None);

        assert!(result.is_err());
        assert!(!external.join("escaped.txt").exists());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_ancestor_of_destination_root() {
        use std::os::unix::fs::symlink;

        let temp = temp_dir();
        let external = temp.join("external");
        fs::create_dir_all(&external).unwrap();
        let redirect = temp.join("redirect");
        symlink(&external, &redirect).unwrap();
        let archive = create_test_tar_gz(&[("escaped.txt", b"blocked")]);

        let result = extract_tar_gz_safe(&archive, &redirect.join("out"), None, None, None);

        assert!(result.is_err());
        assert!(!external.join("out").exists());
    }

    #[test]
    fn tar_gz_decoded_stream_is_bounded_including_metadata() {
        let temp = temp_dir();

        let supported = create_test_tar_gz(&[("data/a.txt", b"abc")]);
        extract_tar_gz_safe(
            &supported,
            &temp.join("supported"),
            Some(1024),
            Some(8),
            Some(4),
        )
        .expect("a supported archive extracts");

        // Build the long-name member through `append_data`, which emits the GNU
        // long-name extension the TAR library would buffer before yielding the entry.
        let long_name = format!("data/{}", "l".repeat(64 * 1024));
        let mut tar_bytes = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_bytes);
            let mut header = tar::Header::new_gnu();
            header.set_size(3);
            header.set_mode(0o600);
            header.set_cksum();
            builder
                .append_data(&mut header, &long_name, &b"abc"[..])
                .expect("append long-name member");
            builder.finish().expect("finish tar");
        }
        let mut oversized = Vec::new();
        {
            let mut encoder =
                flate2::write::GzEncoder::new(&mut oversized, flate2::Compression::default());
            encoder.write_all(&tar_bytes).expect("compress payload");
            encoder.finish().expect("finish gzip");
        }
        let refused_root = temp.join("long-name");
        assert!(
            extract_tar_gz_safe(&oversized, &refused_root, Some(1024), Some(8), Some(4)).is_err(),
            "long-name metadata beyond the decoded budget is refused"
        );
        assert!(!refused_root.exists());
    }

    #[test]
    fn tar_gz_directory_with_a_body_is_refused() {
        let temp = temp_dir();
        let mut tar_bytes = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_bytes);
            let mut header = tar::Header::new_gnu();
            header.set_path("data/body/").unwrap();
            header.set_entry_type(tar::EntryType::Directory);
            header.set_size(4096);
            header.set_mode(0o700);
            header.set_cksum();
            builder
                .append(&header, std::io::repeat(0).take(4096))
                .unwrap();
            builder.finish().unwrap();
        }
        let mut gz_bytes = Vec::new();
        {
            let mut encoder =
                flate2::write::GzEncoder::new(&mut gz_bytes, flate2::Compression::default());
            encoder.write_all(&tar_bytes).unwrap();
            encoder.finish().unwrap();
        }
        let destination = temp.join("body");
        assert!(
            extract_tar_gz_safe(&gz_bytes, &destination, Some(8192), Some(8), Some(4)).is_err(),
            "a directory body is refused"
        );
        assert!(!destination.exists());
    }

    #[test]
    fn bounded_stream_stops_at_its_budget() {
        let mut stream = BoundedStream::new(std::io::repeat(0_u8), 8);
        let mut buffer = [0_u8; 16];
        let mut total = 0_usize;
        loop {
            match stream.read(&mut buffer) {
                Ok(read) => total += read,
                Err(error) => {
                    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
                    break;
                }
            }
        }
        assert_eq!(total, 8);
        assert!(stream.exceeded());
    }

    #[test]
    fn bounded_stream_propagates_an_injected_read_fault() {
        struct FaultyReader {
            remaining: usize,
        }
        impl Read for FaultyReader {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                if self.remaining == 0 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "injected read fault",
                    ));
                }
                let read = buffer.len().min(self.remaining);
                self.remaining -= read;
                Ok(read)
            }
        }
        let mut stream = BoundedStream::new(FaultyReader { remaining: 2 }, 8);
        let mut buffer = [0_u8; 2];
        assert_eq!(stream.read(&mut buffer).unwrap(), 2);
        assert!(stream.read(&mut buffer).is_err());
    }

    fn gzip(bytes: &[u8]) -> Vec<u8> {
        let mut gz_buf = Vec::new();
        {
            let mut encoder =
                flate2::write::GzEncoder::new(&mut gz_buf, flate2::Compression::default());
            encoder.write_all(bytes).unwrap();
            encoder.finish().unwrap();
        }
        gz_buf
    }

    /// A TAR body whose member paths may exceed the short-header limit.
    fn tar_bytes_with(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut tar_buf = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_buf);
            for (path, data) in entries {
                let mut header = tar::Header::new_gnu();
                header.set_size(data.len() as u64);
                header.set_mode(0o644);
                header.set_cksum();
                builder.append_data(&mut header, path, &data[..]).unwrap();
            }
            builder.finish().unwrap();
        }
        tar_buf
    }

    #[test]
    fn tar_gz_truncated_trailer_is_refused() {
        let temp = temp_dir();
        let bytes = create_test_tar_gz(&[("a.txt", b"abc")]);
        let truncated = &bytes[..bytes.len() - 4];
        assert!(
            extract_tar_gz_safe(truncated, &temp.join("truncated"), None, None, None).is_err(),
            "a truncated GZIP trailer is refused"
        );
    }

    #[test]
    fn tar_gz_corrupted_trailer_is_refused() {
        let temp = temp_dir();
        let mut bytes = create_test_tar_gz(&[("a.txt", b"abc")]);
        let length = bytes.len();
        bytes[length - 5] ^= 0xFF; // the CRC field of the trailer
        assert!(
            extract_tar_gz_safe(&bytes, &temp.join("crc"), None, None, None).is_err(),
            "a corrupted GZIP trailer is refused"
        );
    }

    #[test]
    fn tar_gz_trailing_material_is_refused() {
        let temp = temp_dir();
        let mut bytes = create_test_tar_gz(&[("a.txt", b"abc")]);
        bytes.extend_from_slice(b"trailing");
        assert!(
            extract_tar_gz_safe(&bytes, &temp.join("trailing"), None, None, None).is_err(),
            "compressed material after the GZIP member is refused"
        );
    }

    #[test]
    fn tar_gz_member_metadata_is_bounded_before_the_library_buffers_it() {
        let temp = temp_dir();
        let long_name = format!("data/{}", "l".repeat(8 * 1024));
        let bytes = gzip(&tar_bytes_with(&[(long_name.as_str(), b"abc")]));
        // The aggregate budget admits this stream, so the refusal proves the per-member
        // metadata bound applied at the library advance.
        assert!(
            extract_tar_gz_safe(
                &bytes,
                &temp.join("metadata"),
                Some(64 * 1024),
                Some(8),
                Some(4)
            )
            .is_err(),
            "metadata beyond the per-member budget is refused"
        );
    }

    #[test]
    fn tar_gz_valid_long_paths_are_admitted() {
        let temp = temp_dir();
        let component = "m".repeat(200);
        let long_name = format!("data/{component}/{component}/{component}/{component}/file.txt");
        let bytes = gzip(&tar_bytes_with(&[(long_name.as_str(), b"abc")]));
        let destination = temp.join("long-path");
        extract_tar_gz_safe(&bytes, &destination, None, None, None).expect("long path extracts");
        assert!(destination.join(&long_name).is_file());
    }

    #[test]
    fn zip_directory_with_a_body_is_refused() {
        let temp = temp_dir();
        let mut cursor = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut cursor);
            // Directory-mode external attributes, so the fixture reaches the body guard
            // instead of being refused as a regular file with a directory name.
            let options = zip::write::SimpleFileOptions::default().unix_permissions(0o040755);
            writer.start_file("data/d/", options).unwrap();
            writer.write_all(b"body").unwrap();
            writer.finish().unwrap();
        }
        let mut bytes = cursor.into_inner();
        set_zip_directory_mode(&mut bytes);
        let destination = temp.join("zip-body");
        let error =
            extract_zip_safe(&bytes, &destination, default_zip_extraction_limits()).unwrap_err();
        assert_eq!(error.to_string(), "zip_entry_directory_body_unsupported");
        assert_eq!(fs::read_dir(&destination).unwrap().count(), 0);
    }

    #[test]
    fn zip_directory_with_a_hidden_body_behind_zero_declared_sizes_is_refused() {
        let temp = temp_dir();
        let mut cursor = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut cursor);
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored)
                .unix_permissions(0o040755);
            writer.start_file("data/d/", options).unwrap();
            writer.write_all(b"bodybody").unwrap();
            writer.finish().unwrap();
        }
        let mut bytes = cursor.into_inner();
        set_zip_directory_mode(&mut bytes);
        // Patch the central directory entry's compressed and uncompressed sizes to zero,
        // leaving the real body and its checksum in place.
        let central = bytes
            .windows(4)
            .position(|window| window == [0x50, 0x4b, 0x01, 0x02])
            .expect("central directory header");
        bytes[central + 20..central + 24].fill(0);
        bytes[central + 24..central + 28].fill(0);

        let destination = temp.join("zip-hidden-body");
        assert!(
            extract_zip_safe(&bytes, &destination, default_zip_extraction_limits()).is_err(),
            "a directory claiming zero sizes while carrying a checksummed body is refused"
        );
        assert!(
            !destination.exists(),
            "local/index mismatch is refused before writes"
        );
    }

    fn set_zip_directory_mode(bytes: &mut [u8]) {
        let central = {
            let mut archive = ZipArchive::new(std::io::Cursor::new(&*bytes)).unwrap();
            archive.by_index(0).unwrap().central_header_start() as usize
        };
        bytes[central + 38..central + 42]
            .copy_from_slice(&((0o040755_u32 << 16) | 16).to_le_bytes());
        let mut archive = ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        assert_eq!(
            archive.by_index(0).unwrap().unix_mode().unwrap() & 0o170000,
            0o040000
        );
    }

    fn pax_sized_tar(header_size: u64, effective_size: u64, body: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut bytes);
            let size = effective_size.to_string();
            builder
                .append_pax_extensions([("size", size.as_bytes())])
                .unwrap();
            let mut header = tar::Header::new_ustar();
            header.set_path("file.txt").unwrap();
            header.set_size(header_size);
            header.set_mode(0o600);
            header.set_cksum();
            builder.append(&header, body).unwrap();
            builder.finish().unwrap();
        }
        gzip(&bytes)
    }

    #[test]
    fn pax_effective_size_controls_admission_and_payload_alignment() {
        let root = temp_dir();
        let bytes = pax_sized_tar(0, 3, b"abc");
        extract_tar_gz_safe(&bytes, &root.join("valid"), Some(3), Some(1), Some(2)).unwrap();
        assert_eq!(fs::read(root.join("valid/file.txt")).unwrap(), b"abc");
        let error = extract_tar_gz_safe(&bytes, &root.join("limited"), Some(2), Some(1), Some(2))
            .unwrap_err();
        assert!(error.to_string().contains("byte limit"), "{error}");
        assert!(!root.join("limited").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn empty_transports_and_exact_stream_budget_are_supported() {
        let root = temp_dir();
        extract_tar_gz_safe(
            &gzip(&[0; 1024]),
            &root.join("tar"),
            Some(0),
            Some(0),
            Some(1),
        )
        .unwrap();
        extract_zip_safe(&create_test_zip(&[]), &root.join("zip"), zip_limits()).unwrap();
        let mut bounded = BoundedStream::new(std::io::Cursor::new([1, 2]), 2);
        let mut bytes = Vec::new();
        bounded.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, [1, 2]);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn accumulated_metadata_is_refused_before_reading_the_next_extension_body() {
        let mut builder = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(EntryType::GNULongName);
        header.set_path("long-name").unwrap();
        header.set_size(800);
        header.set_cksum();
        builder.append(&header, &[b'a'; 800][..]).unwrap();
        let mut bytes = builder.into_inner().unwrap();
        bytes.truncate(1536);
        let mut header = tar::Header::new_ustar();
        header.set_entry_type(EntryType::XHeader);
        header.set_size(500);
        header.set_cksum();
        bytes.extend(header.as_bytes());
        // No second body is supplied: an EOF error would mean admission was too late.
        let error = validate_tar_gz_structure(&gzip(&bytes), 4096, 1, 1).unwrap_err();
        assert!(error.to_string().contains("metadata exceeds"), "{error}");
    }

    #[test]
    fn raw_admission_refuses_oversized_metadata_before_the_library_buffers_it() {
        let long_name = format!("data/{}", "l".repeat(8 * 1024));
        let bytes = gzip(&tar_bytes_with(&[(long_name.as_str(), b"abc")]));
        // The aggregate budget admits this stream; the raw scanner refuses the 8 KiB
        // metadata against the 2,560-byte per-member budget before the library parses it.
        let error = validate_tar_gz_structure(&bytes, 64 * 1024, 8, 4)
            .expect_err("metadata beyond the per-member budget is refused");
        assert!(error.to_string().contains("metadata exceeds"), "{error}");
    }

    #[test]
    fn raw_admission_refuses_a_sparse_type_before_the_library_parses_it() {
        let temp = temp_dir();
        let mut tar_bytes = tar_bytes_with(&[("data/real.txt", b"abc")]);
        // Replace the archive with a sparse header ('S') ahead of the terminator.
        let mut sparse = [0_u8; 512];
        sparse[..4].copy_from_slice(b"spar");
        sparse[124..136].copy_from_slice(b"00000000000\0");
        sparse[156] = b'S';
        // A valid checksum for the crafted header (checksum field read as spaces).
        let mut unsigned = 0_u64;
        for (index, byte) in sparse.iter().enumerate() {
            let value = if (148..156).contains(&index) {
                b' '
            } else {
                *byte
            };
            unsigned += u64::from(value);
        }
        sparse[148..156].copy_from_slice(format!("{unsigned:06o}\0 ").as_bytes());
        tar_bytes.truncate(tar_bytes.len() - 1024);
        tar_bytes.extend_from_slice(&sparse);
        tar_bytes.extend_from_slice(&[0_u8; 1024]);
        let bytes = gzip(&tar_bytes);

        let destination = temp.join("sparse");
        let error = extract_tar_gz_safe(&bytes, &destination, None, None, None)
            .expect_err("a sparse type is refused");
        assert!(error.to_string().contains("unsupported type"), "{error}");
        assert!(!destination.exists());
    }

    #[test]
    fn raw_admission_refuses_a_member_hidden_behind_the_terminator() {
        let mut tar_bytes = tar_bytes_with(&[("data/real.txt", b"abc")]);
        // `finish` already wrote the zero terminator; append another member behind it
        // inside the same GZIP member.
        let mut hidden = [0_u8; 512];
        hidden[..8].copy_from_slice(b"hidden.t");
        hidden[124..136].copy_from_slice(b"00000000003\0");
        hidden[156] = b'0';
        let mut unsigned = 0_u64;
        for (index, byte) in hidden.iter().enumerate() {
            let value = if (148..156).contains(&index) {
                b' '
            } else {
                *byte
            };
            unsigned += u64::from(value);
        }
        hidden[148..156].copy_from_slice(format!("{unsigned:06o}\0 ").as_bytes());
        tar_bytes.extend_from_slice(&hidden);
        tar_bytes.extend_from_slice(b"hid");
        tar_bytes.extend_from_slice(&[0_u8; 509]);
        let bytes = gzip(&tar_bytes);

        let temp = temp_dir();
        let destination = temp.join("hidden");
        assert!(
            extract_tar_gz_safe(&bytes, &destination, None, None, None).is_err(),
            "a member behind the TAR terminator is refused"
        );
    }

    #[test]
    fn tar_gz_non_portable_member_names_are_refused() {
        let temp = temp_dir();
        for (index, name) in ["data/a\\b.txt", "data/a:b.txt", "data/a\u{1}b.txt"]
            .iter()
            .enumerate()
        {
            let bytes = gzip(&tar_bytes_with(&[(name, b"abc")]));
            let destination = temp.join(format!("non-portable-{index}"));
            assert!(
                extract_tar_gz_safe(&bytes, &destination, None, None, None).is_err(),
                "TAR member {name:?} must be refused"
            );
        }
    }

    #[test]
    fn zip_control_character_member_names_are_refused() {
        let temp = temp_dir();
        let bytes = create_test_zip(&[("a\u{1}b.txt", b"abc", 0o100600)]);
        let destination = temp.join("zip-control");
        assert!(
            extract_zip_safe(&bytes, &destination, default_zip_extraction_limits()).is_err(),
            "a ZIP control-character member name is refused"
        );
    }

    #[test]
    fn zip_local_header_data_start_mismatch_is_refused() {
        let temp = temp_dir();
        let mut bytes = create_test_zip(&[("data/test.txt", b"content", 0o100644)]);
        bytes[28] = 8;
        let destination = temp.join("zip-mismatch");
        assert!(
            extract_zip_safe(&bytes, &destination, default_zip_extraction_limits()).is_err(),
            "a ZIP local header data start mismatch is refused"
        );
        assert!(!destination.exists());
    }
}
