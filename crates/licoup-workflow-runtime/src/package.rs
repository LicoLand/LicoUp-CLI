use anyhow::{Context, Result, anyhow, ensure};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use uuid::Uuid;
use zip::write::SimpleFileOptions;

use licoup_foundation::core::safe_archive::{ZipEntryInfo, ZipExtractionLimits, extract_zip_safe};

use licoup_workflow::{
    CompiledWorkflow, PreflightDiagnostic, WorkflowDefinition, WorkflowDiagnosticCode,
    WorkflowDiagnosticExpected, WorkflowDiagnosticRecovery, WorkflowDiagnosticStage,
    WorkflowValidationFailure, compile_workflow, compile_workflow_source,
};

const MAX_PACKAGE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_EXTRACTED_BYTES: u64 = 16 * 1024 * 1024;
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_WORKFLOW_BYTES: u64 = 512 * 1024;
const MAX_PACKAGE_ENTRIES: usize = 128;
const MAX_PACKAGE_DEPTH: usize = 8;
const MAX_SCRIPT_FILES: usize = 64;
const PREPARATION_SCHEMA: &str = "licoup.adaptive-flywheel.preparation.v1";
const SYNTHETIC_FIXTURE_WORKFLOW: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/workflow_runtime/synthetic-entry-worker.fixture"
));

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreparedPackage {
    pub preparation_id: String,
    pub definition_id: String,
    pub revision_digest: String,
    pub semantics_digest: String,
    pub name: String,
    pub version: String,
    pub asset_count: usize,
    pub prepared_at_unix_ms: i64,
}

#[derive(Clone, Debug)]
pub(crate) struct CommittedPackage {
    pub prepared: PreparedPackage,
    pub workflow: WorkflowDefinition,
}

#[derive(Clone, Debug)]
pub struct StrategyPackageImporter {
    root: PathBuf,
}

impl StrategyPackageImporter {
    pub fn open(portable_root: &Path) -> Result<Self> {
        let flywheel_root = portable_root.join("client-state").join("adaptive-flywheel");
        licoup_foundation::platform::file_security::ensure_private_dir(
            &portable_root.join("client-state"),
        )?;
        licoup_foundation::platform::file_security::ensure_private_dir(&flywheel_root)?;
        let root = flywheel_root.join("strategy-packages");
        licoup_foundation::platform::file_security::ensure_private_dir(&root)?;
        licoup_foundation::platform::file_security::ensure_private_dir(&root.join("prepared"))?;
        licoup_foundation::platform::file_security::ensure_private_dir(&root.join("revisions"))?;
        let root = fs::canonicalize(root)?;
        Ok(Self { root })
    }

    pub fn prepare_bytes(&self, bytes: &[u8]) -> Result<PreparedPackage> {
        ensure!(bytes.len() as u64 <= MAX_PACKAGE_BYTES, "package_too_large");
        let preparation_id = format!("preparation-{}", Uuid::new_v4());
        let staging = self.root.join("prepared").join(&preparation_id);
        ensure!(!staging.exists(), "preparation_identity_conflict");
        licoup_foundation::platform::file_security::ensure_private_dir(&staging)?;
        let content = staging.join("content");
        let prepared = (|| {
            let entries = extract_zip_safe(
                bytes,
                &content,
                ZipExtractionLimits {
                    max_archive_bytes: MAX_PACKAGE_BYTES,
                    max_total_bytes: MAX_EXTRACTED_BYTES,
                    max_file_bytes: MAX_FILE_BYTES,
                    max_entries: MAX_PACKAGE_ENTRIES,
                    max_depth: MAX_PACKAGE_DEPTH,
                },
            )
            .map_err(map_archive_error)?;
            let inventory = validate_layout(&content, &entries)?;
            let workflow_path = content.join("workflow.json");
            let workflow_metadata =
                fs::metadata(&workflow_path).map_err(|_| anyhow!("package_layout_invalid"))?;
            ensure!(
                workflow_metadata.is_file() && workflow_metadata.len() <= MAX_WORKFLOW_BYTES,
                "workflow_invalid"
            );
            let mut source = Vec::with_capacity(workflow_metadata.len() as usize);
            fs::File::open(&workflow_path)?.read_to_end(&mut source)?;
            let compiled = compile_workflow_source(&source)?;
            validate_script_references(&compiled, &inventory)?;
            let canonical = serde_json::to_vec(compiled.definition())?;
            licoup_foundation::platform::file_security::atomic_write_private_text(
                &workflow_path,
                std::str::from_utf8(&canonical).map_err(|_| anyhow!("workflow_invalid"))?,
            )?;
            let semantics_digest = sha256_hex(&canonical);
            let revision_digest = revision_digest(&content, &inventory, &semantics_digest)?;
            let prepared = PreparedPackage {
                preparation_id: preparation_id.clone(),
                definition_id: compiled.definition().metadata.id.clone(),
                revision_digest,
                semantics_digest,
                name: compiled.definition().metadata.name.clone(),
                version: compiled.definition().metadata.version.clone(),
                asset_count: inventory.len(),
                prepared_at_unix_ms: now_ms(),
            };
            let envelope = PreparationEnvelope {
                schema: PREPARATION_SCHEMA.into(),
                prepared: prepared.clone(),
            };
            let envelope_json = serde_json::to_string(&envelope)?;
            licoup_foundation::platform::file_security::atomic_write_private_text(
                &staging.join("preparation.json"),
                &envelope_json,
            )?;
            Ok(prepared)
        })();
        if prepared.is_err() {
            let _ = fs::remove_dir_all(&staging);
        }
        prepared
    }

    pub fn prepared(&self, preparation_id: &str) -> Result<PreparedPackage> {
        validate_preparation_id(preparation_id)?;
        let path = self
            .root
            .join("prepared")
            .join(preparation_id)
            .join("preparation.json");
        let bytes = read_bounded(&path, 64 * 1024).map_err(|_| anyhow!("preparation_not_found"))?;
        let envelope: PreparationEnvelope =
            serde_json::from_slice(&bytes).map_err(|_| anyhow!("preparation_not_found"))?;
        ensure!(
            envelope.schema == PREPARATION_SCHEMA
                && envelope.prepared.preparation_id == preparation_id,
            "preparation_not_found"
        );
        Ok(envelope.prepared)
    }

    pub(crate) fn commit(
        &self,
        preparation_id: &str,
        expected_revision_digest: &str,
    ) -> Result<CommittedPackage> {
        let prepared = self.prepared(preparation_id)?;
        ensure!(
            prepared.revision_digest == expected_revision_digest,
            "revision_conflict"
        );
        let commit_lock = licoup_foundation::platform::file_security::open_private_lock_file(
            &self.root.join("commit.lock"),
        )?;
        commit_lock
            .lock_exclusive()
            .map_err(|_| anyhow!("strategy_revision_commit_failed"))?;
        let prepared_directory = self.root.join("prepared").join(preparation_id);
        let revisions = self.root.join("revisions");
        let target = revisions.join(&prepared.revision_digest);
        if target.exists() {
            self.verified_revision_content(&prepared.revision_digest, &prepared.semantics_digest)?;
            fs::remove_dir_all(&prepared_directory).ok();
        } else {
            fs::rename(&prepared_directory, &target)
                .with_context(|| "strategy_revision_commit_failed")?;
            harden_read_only_tree(&target)?;
        }
        let content =
            self.verified_revision_content(&prepared.revision_digest, &prepared.semantics_digest)?;
        let workflow_bytes = read_bounded(&content.join("workflow.json"), MAX_WORKFLOW_BYTES)?;
        let compiled = compile_workflow_source(&workflow_bytes)?;
        ensure!(
            sha256_hex(&serde_json::to_vec(compiled.definition())?) == prepared.semantics_digest,
            "revision_conflict"
        );
        Ok(CommittedPackage {
            prepared,
            workflow: compiled.into_definition(),
        })
    }

    pub(crate) fn revision_content(&self, digest: &str) -> Result<PathBuf> {
        validate_digest(digest)?;
        let content = self.root.join("revisions").join(digest).join("content");
        ensure!(content.is_dir(), "definition_not_found");
        Ok(content)
    }

    pub(crate) fn verified_revision_content(
        &self,
        digest: &str,
        expected_semantics_digest: &str,
    ) -> Result<PathBuf> {
        let content = self.revision_content(digest)?;
        let inventory = persisted_inventory(&content)?;
        let workflow_bytes = read_bounded(&content.join("workflow.json"), MAX_WORKFLOW_BYTES)?;
        let definition: WorkflowDefinition = serde_json::from_slice(&workflow_bytes)
            .map_err(|_| anyhow!("strategy_revision_content_drifted"))?;
        let canonical = serde_json::to_vec(&definition)?;
        ensure!(
            workflow_bytes == canonical,
            "strategy_revision_content_drifted"
        );
        let semantics_digest = sha256_hex(&canonical);
        ensure!(
            semantics_digest == expected_semantics_digest,
            "strategy_revision_content_drifted"
        );
        ensure!(
            revision_digest(&content, &inventory, &semantics_digest)? == digest,
            "strategy_revision_content_drifted"
        );
        let definition = licoup_workflow_store::normalize_legacy_workflow(definition.clone())
            .unwrap_or(definition);
        compile_workflow(definition).map_err(|_| anyhow!("strategy_revision_content_drifted"))?;
        Ok(content)
    }

    /// Open the restored package store only when the recovery actually carried one.
    ///
    /// A data root whose committed revisions never existed is legitimate; reading it
    /// back must not create package state merely to observe its absence.
    pub fn open_restored(portable_root: &Path) -> Result<Option<Self>> {
        let root = portable_root
            .join("client-state")
            .join("adaptive-flywheel")
            .join("strategy-packages");
        if !root.is_dir() {
            return Ok(None);
        }
        Ok(Some(Self::open(portable_root)?))
    }

    /// Re-establish the immutable revision invariant and read every revision back.
    ///
    /// A generic archive restore publishes each member under fresh permissions, so the
    /// read-only hardening applied when a revision was committed is gone even though
    /// every byte is intact. This re-applies that same hardening through this owner and
    /// then verifies each revision digest, semantics digest and asset inventory through
    /// the ordinary `verified_revision_content` readback. No revision byte is rewritten;
    /// a missing, altered or unexpected revision is refused instead of accepted.
    pub fn restore_revision_invariants(&self) -> Result<Vec<String>> {
        let revisions = self.root.join("revisions");
        let entries = match fs::read_dir(&revisions) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(_) => return Err(anyhow!("strategy_revision_content_drifted")),
        };
        let mut digests = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|_| anyhow!("strategy_revision_content_drifted"))?;
            let digest = entry
                .file_name()
                .into_string()
                .map_err(|_| anyhow!("strategy_revision_content_drifted"))?;
            let metadata = fs::symlink_metadata(entry.path())
                .map_err(|_| anyhow!("strategy_revision_content_drifted"))?;
            ensure!(
                metadata.is_dir() && !metadata.file_type().is_symlink(),
                "strategy_revision_content_drifted"
            );
            validate_digest(&digest).map_err(|_| anyhow!("strategy_revision_content_drifted"))?;
            // The preparation envelope travels with its committed revision; it names
            // the semantics digest this readback must verify.
            let prepared_bytes = read_bounded(&entry.path().join("preparation.json"), 64 * 1024)
                .map_err(|_| anyhow!("strategy_revision_content_drifted"))?;
            let envelope: PreparationEnvelope = serde_json::from_slice(&prepared_bytes)
                .map_err(|_| anyhow!("strategy_revision_content_drifted"))?;
            ensure!(
                envelope.schema == PREPARATION_SCHEMA
                    && envelope.prepared.revision_digest == digest,
                "strategy_revision_content_drifted"
            );
            harden_read_only_tree(&entry.path())?;
            self.verified_revision_content(&digest, &envelope.prepared.semantics_digest)?;
            digests.push(digest);
        }
        digests.sort();
        Ok(digests)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct PreparationEnvelope {
    schema: String,
    prepared: PreparedPackage,
}

fn validate_layout(content: &Path, entries: &[ZipEntryInfo]) -> Result<BTreeSet<String>> {
    let files = entries
        .iter()
        .filter(|entry| !entry.directory)
        .map(|entry| entry.path.to_string_lossy().replace('\\', "/"))
        .collect::<BTreeSet<_>>();
    ensure!(files.contains("workflow.json"), "package_layout_invalid");
    let scripts = files
        .iter()
        .filter(|path| path.starts_with("scripts/"))
        .count();
    ensure!(scripts <= MAX_SCRIPT_FILES, "package_resource_limit");
    ensure!(
        files
            .iter()
            .all(|path| path == "workflow.json" || path.starts_with("scripts/")),
        "package_layout_invalid"
    );
    ensure!(
        entries.iter().filter(|entry| entry.directory).all(|entry| {
            let path = entry.path.to_string_lossy().replace('\\', "/");
            path == "scripts" || path.starts_with("scripts/")
        }),
        "package_layout_invalid"
    );
    ensure!(
        content.join("workflow.json").is_file(),
        "package_layout_invalid"
    );
    Ok(files)
}

fn validate_script_references(
    workflow: &CompiledWorkflow,
    inventory: &BTreeSet<String>,
) -> std::result::Result<(), WorkflowValidationFailure> {
    let mut diagnostics = Vec::new();
    for (index, state) in workflow.definition().states.iter().enumerate() {
        if let Some(entry) = &state.entry
            && !inventory.contains(entry)
        {
            diagnostics.push(PreflightDiagnostic {
                code: WorkflowDiagnosticCode::WorkflowScriptEntryInvalid,
                stage: WorkflowDiagnosticStage::PackageValidate,
                path: Some(format!("/states/{index}/entry")),
                related_paths: Vec::new(),
                membership_id: None,
                actual: None,
                limit: None,
                expected: Some(WorkflowDiagnosticExpected::ExistingReference),
                actual_kind: None,
                recovery: Some(WorkflowDiagnosticRecovery::CorrectReference),
                line: None,
                column: None,
            });
        }
    }
    if diagnostics.is_empty() {
        Ok(())
    } else {
        Err(WorkflowValidationFailure { diagnostics })
    }
}

fn persisted_inventory(content: &Path) -> Result<BTreeSet<String>> {
    let root_metadata = fs::symlink_metadata(content)?;
    ensure!(
        root_metadata.is_dir()
            && !root_metadata.file_type().is_symlink()
            && root_metadata.permissions().readonly(),
        "strategy_revision_content_drifted"
    );
    let mut stack = vec![(content.to_path_buf(), 0usize)];
    let mut files = BTreeSet::new();
    let mut entries = 0usize;
    let mut total_bytes = 0u64;
    while let Some((directory, depth)) = stack.pop() {
        ensure!(
            depth <= MAX_PACKAGE_DEPTH,
            "strategy_revision_content_drifted"
        );
        for entry in fs::read_dir(&directory)? {
            entries = entries.saturating_add(1);
            ensure!(
                entries <= MAX_PACKAGE_ENTRIES,
                "strategy_revision_content_drifted"
            );
            let path = entry?.path();
            let metadata = fs::symlink_metadata(&path)?;
            ensure!(
                !metadata.file_type().is_symlink() && metadata.permissions().readonly(),
                "strategy_revision_content_drifted"
            );
            let relative = path
                .strip_prefix(content)
                .map_err(|_| anyhow!("strategy_revision_content_drifted"))?;
            let normalized = relative
                .iter()
                .map(|part| {
                    part.to_str()
                        .filter(|value| !value.is_empty())
                        .ok_or_else(|| anyhow!("strategy_revision_content_drifted"))
                })
                .collect::<Result<Vec<_>>>()?
                .join("/");
            ensure!(
                normalized == "workflow.json"
                    || normalized == "scripts"
                    || normalized.starts_with("scripts/"),
                "strategy_revision_content_drifted"
            );
            if metadata.is_dir() {
                stack.push((path, depth + 1));
            } else {
                ensure!(metadata.is_file(), "strategy_revision_content_drifted");
                ensure!(
                    metadata.len() <= MAX_FILE_BYTES,
                    "strategy_revision_content_drifted"
                );
                total_bytes = total_bytes
                    .checked_add(metadata.len())
                    .ok_or_else(|| anyhow!("strategy_revision_content_drifted"))?;
                ensure!(
                    total_bytes <= MAX_EXTRACTED_BYTES,
                    "strategy_revision_content_drifted"
                );
                files.insert(normalized);
            }
        }
    }
    ensure!(
        files.contains("workflow.json"),
        "strategy_revision_content_drifted"
    );
    ensure!(
        files
            .iter()
            .filter(|path| path.starts_with("scripts/"))
            .count()
            <= MAX_SCRIPT_FILES,
        "strategy_revision_content_drifted"
    );
    Ok(files)
}

fn revision_digest(
    content: &Path,
    inventory: &BTreeSet<String>,
    semantics_digest: &str,
) -> Result<String> {
    let mut hasher = revision_hasher(semantics_digest);
    for path in inventory {
        let bytes = read_bounded(&content.join(path), MAX_FILE_BYTES)?;
        hash_revision_asset(&mut hasher, path, &bytes);
    }
    Ok(hex_digest(hasher.finalize().as_slice()))
}

fn revision_hasher(semantics_digest: &str) -> Sha256 {
    let mut hasher = Sha256::new();
    hasher.update(b"licoup-adaptive-flywheel-revision-v1\0");
    hasher.update(semantics_digest.as_bytes());
    hasher
}

fn hash_revision_asset(hasher: &mut Sha256, path: &str, bytes: &[u8]) {
    hasher.update(b"\0");
    hasher.update(path.as_bytes());
    hasher.update(b"\0");
    hasher.update((bytes.len() as u64).to_be_bytes());
    hasher.update(Sha256::digest(bytes));
}

fn map_archive_error(error: anyhow::Error) -> anyhow::Error {
    let message = error.to_string();
    if message.contains("duplicate") || message.contains("case_collision") {
        anyhow!("package_duplicate_entry")
    } else if message.contains("limit") || message.contains("overflow") {
        anyhow!("package_resource_limit")
    } else {
        anyhow!("package_entry_invalid")
    }
}

fn read_bounded(path: &Path, maximum: u64) -> Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink() && metadata.len() <= maximum,
        "strategy_revision_file_invalid"
    );
    fs::read(path).map_err(Into::into)
}

fn harden_read_only_tree(root: &Path) -> Result<()> {
    let mut paths = vec![root.to_path_buf()];
    let mut ordered = Vec::new();
    while let Some(path) = paths.pop() {
        let metadata = fs::symlink_metadata(&path)?;
        ensure!(
            !metadata.file_type().is_symlink(),
            "strategy_revision_symlink"
        );
        ordered.push(path.clone());
        if metadata.is_dir() {
            for entry in fs::read_dir(&path)? {
                paths.push(entry?.path());
            }
        }
    }
    ordered.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    for path in ordered {
        let metadata = fs::symlink_metadata(&path)?;
        let mut permissions = metadata.permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&path, permissions)?;
    }
    Ok(())
}

fn validate_preparation_id(value: &str) -> Result<()> {
    ensure!(
        value.starts_with("preparation-")
            && value.len() <= 64
            && value
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '-'),
        "preparation_not_found"
    );
    Ok(())
}

fn validate_digest(value: &str) -> Result<()> {
    ensure!(
        value.len() == 64 && value.chars().all(|character| character.is_ascii_hexdigit()),
        "revision_digest_invalid"
    );
    Ok(())
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex_digest(Sha256::digest(bytes).as_slice())
}

fn hex_digest(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

pub fn zip_package_files(files: &[(&str, &[u8])]) -> Result<Vec<u8>> {
    let mut cursor = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut cursor);
        let options = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .unix_permissions(0o100600);
        for (path, bytes) in files {
            writer.start_file(*path, options)?;
            writer.write_all(bytes)?;
        }
        writer.finish()?;
    }
    Ok(cursor.into_inner())
}

pub fn synthetic_fixture_package_bytes() -> Result<Vec<u8>> {
    zip_package_files(&[("workflow.json", SYNTHETIC_FIXTURE_WORKFLOW)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn root() -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "lico-strategy-package-test-{}",
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn remove_root(path: PathBuf) {
        let mut stack = vec![path.clone()];
        while let Some(current) = stack.pop() {
            if let Ok(metadata) = fs::symlink_metadata(&current) {
                make_writable(&current, metadata.permissions());
                if metadata.is_dir()
                    && let Ok(entries) = fs::read_dir(&current)
                {
                    stack.extend(entries.flatten().map(|entry| entry.path()));
                }
            }
        }
        fs::remove_dir_all(path).unwrap();
    }

    fn make_writable(path: &Path, mut permissions: fs::Permissions) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            permissions.set_mode(permissions.mode() | 0o200);
        }
        #[cfg(not(unix))]
        permissions.set_readonly(false);
        fs::set_permissions(path, permissions).unwrap();
    }

    #[test]
    fn a_directory_member_with_a_body_is_refused_before_preparation() {
        let root = root();
        let importer = StrategyPackageImporter::open(&root).unwrap();
        let mut cursor = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut cursor);
            // Directory-mode external attributes, so the fixture reaches the directory
            // body guard instead of being refused as a regular file with a directory name.
            let options = SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated)
                .unix_permissions(0o040755);
            writer.start_file("workflow.json", options).unwrap();
            writer.write_all(SYNTHETIC_FIXTURE_WORKFLOW).unwrap();
            writer.start_file("data/d/", options).unwrap();
            writer.write_all(b"body").unwrap();
            writer.finish().unwrap();
        }
        let mut bytes = cursor.into_inner();
        let central = {
            let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap();
            archive.by_index(1).unwrap().central_header_start() as usize
        };
        // unix_permissions masks file type bits; set and verify a real directory mode.
        bytes[central + 38..central + 42]
            .copy_from_slice(&((0o040755_u32 << 16) | 16).to_le_bytes());
        {
            let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap();
            assert_eq!(
                archive.by_index(1).unwrap().unix_mode().unwrap() & 0o170000,
                0o040000
            );
        }
        let error = importer
            .prepare_bytes(&bytes)
            .expect_err("a directory member with a body is refused");
        assert_eq!(error.to_string(), "package_entry_invalid");
        remove_root(root);
    }

    #[test]
    fn multiline_instructions_survive_import_and_revision_reload() {
        let root = root();
        let importer = StrategyPackageImporter::open(&root).unwrap();
        let mut workflow: serde_json::Value =
            serde_json::from_slice(SYNTHETIC_FIXTURE_WORKFLOW).unwrap();
        let instruction = "Read the plan.\n\t- 修改 fixture。\r\n- Run tests.";
        workflow["states"][1]["instruction"] = instruction.into();
        let source = serde_json::to_vec(&workflow).unwrap();
        let bytes = zip_package_files(&[("workflow.json", &source)]).unwrap();
        let prepared = importer.prepare_bytes(&bytes).unwrap();
        let committed = importer
            .commit(&prepared.preparation_id, &prepared.revision_digest)
            .unwrap();
        assert_eq!(committed.workflow.states[1].instruction, instruction);
        importer
            .verified_revision_content(&prepared.revision_digest, &prepared.semantics_digest)
            .unwrap();
        remove_root(root);
    }

    #[test]
    fn instruction_validation_preserves_resource_and_control_character_limits() {
        let root = root();
        let importer = StrategyPackageImporter::open(&root).unwrap();
        for instruction in [
            "Read\0plan".to_owned(),
            "Read\u{1b}[31mplan".to_owned(),
            "Read\u{7f}plan".to_owned(),
            "x".repeat(16 * 1024 + 1),
            " padded ".to_owned(),
        ] {
            let mut workflow: serde_json::Value =
                serde_json::from_slice(SYNTHETIC_FIXTURE_WORKFLOW).unwrap();
            workflow["states"][1]["instruction"] = instruction.into();
            let source = serde_json::to_vec(&workflow).unwrap();
            let bytes = zip_package_files(&[("workflow.json", &source)]).unwrap();
            let error = importer.prepare_bytes(&bytes).unwrap_err();
            let failure = error.downcast_ref::<WorkflowValidationFailure>().unwrap();
            assert!(failure.diagnostics.iter().any(|diagnostic| diagnostic.code
                == WorkflowDiagnosticCode::WorkflowStateInstructionInvalid));
            assert!(
                licoup_workflow::compile_workflow(serde_json::from_value(workflow).unwrap())
                    .is_err()
            );
        }
        remove_root(root);
    }

    #[test]
    fn synthetic_package_round_trips_and_commit_is_idempotent() {
        let root = root();
        let importer = StrategyPackageImporter::open(&root).unwrap();
        let bytes = synthetic_fixture_package_bytes().unwrap();
        let first = importer.prepare_bytes(&bytes).unwrap();
        let committed = importer
            .commit(&first.preparation_id, &first.revision_digest)
            .unwrap();
        assert_eq!(committed.workflow.metadata.id, "fixture-entry-worker");
        assert_eq!(
            committed
                .workflow
                .actor_slots
                .iter()
                .filter(|slot| slot.entry)
                .map(|slot| slot.id.as_str())
                .collect::<Vec<_>>(),
            vec!["entry"]
        );
        let second = importer.prepare_bytes(&bytes).unwrap();
        assert_eq!(second.revision_digest, first.revision_digest);
        let committed_again = importer
            .commit(&second.preparation_id, &second.revision_digest)
            .unwrap();
        assert_eq!(
            committed_again.prepared.revision_digest,
            first.revision_digest
        );
        let revision_content = importer
            .revision_content(&committed_again.prepared.revision_digest)
            .unwrap();
        assert!(
            !revision_content
                .parent()
                .unwrap()
                .join("source.zip")
                .exists()
        );
        remove_root(root);
    }

    #[test]
    fn local_zip_index_disagreement_is_refused_before_preparation() {
        let root = root();
        let importer = StrategyPackageImporter::open(&root).unwrap();
        let mut bytes = synthetic_fixture_package_bytes().unwrap();
        bytes[22..26].copy_from_slice(&0_u32.to_le_bytes());
        let error = importer.prepare_bytes(&bytes).unwrap_err();
        assert_eq!(error.to_string(), "package_entry_invalid");
        remove_root(root);
    }

    #[test]
    fn persisted_revision_is_rehashed_before_execution() {
        let root = root();
        let importer = StrategyPackageImporter::open(&root).unwrap();
        let prepared = importer
            .prepare_bytes(&synthetic_fixture_package_bytes().unwrap())
            .unwrap();
        importer
            .commit(&prepared.preparation_id, &prepared.revision_digest)
            .unwrap();
        let content = importer
            .verified_revision_content(&prepared.revision_digest, &prepared.semantics_digest)
            .unwrap();
        let workflow = content.join("workflow.json");
        make_writable(&workflow, fs::metadata(&workflow).unwrap().permissions());
        fs::write(&workflow, b"{}").unwrap();
        assert!(
            importer
                .verified_revision_content(&prepared.revision_digest, &prepared.semantics_digest)
                .is_err()
        );
        remove_root(root);
    }

    #[test]
    fn legacy_revision_verification_preserves_immutable_package_identity() {
        let root = root();
        let importer = StrategyPackageImporter::open(&root).unwrap();
        let mut legacy: WorkflowDefinition =
            serde_json::from_slice(SYNTHETIC_FIXTURE_WORKFLOW).unwrap();
        legacy.actor_slots[0].entry = false;
        legacy
            .transitions
            .retain(|transition| transition.event != licoup_workflow::TransitionEvent::Failure);
        let workflow_bytes = serde_json::to_vec(&legacy).unwrap();
        assert!(compile_workflow(legacy).is_err());

        let staging = importer.root.join("revisions").join("legacy-staging");
        let content = staging.join("content");
        fs::create_dir_all(&content).unwrap();
        fs::write(content.join("workflow.json"), &workflow_bytes).unwrap();
        let inventory = BTreeSet::from(["workflow.json".to_owned()]);
        let semantics = sha256_hex(&workflow_bytes);
        let digest = revision_digest(&content, &inventory, &semantics).unwrap();
        let revision = importer.root.join("revisions").join(&digest);
        fs::rename(&staging, &revision).unwrap();
        harden_read_only_tree(&revision).unwrap();

        let verified = importer
            .verified_revision_content(&digest, &semantics)
            .unwrap();
        assert_eq!(
            fs::read(verified.join("workflow.json")).unwrap(),
            workflow_bytes
        );
        assert_eq!(sha256_hex(&workflow_bytes), semantics);
        assert_eq!(
            revision_digest(&verified, &inventory, &semantics).unwrap(),
            digest
        );
        remove_root(root);
    }

    #[test]
    fn malformed_workflow_package_preserves_only_typed_syntax_location() {
        let root = root();
        let importer = StrategyPackageImporter::open(&root).unwrap();
        let bytes =
            zip_package_files(&[("workflow.json", br#"{"private-workspace-sentinel":"#)]).unwrap();
        let error = importer.prepare_bytes(&bytes).unwrap_err();
        let failure = error.downcast_ref::<WorkflowValidationFailure>().unwrap();
        assert_eq!(failure.diagnostics.len(), 1);
        let serialized = serde_json::to_value(&failure.diagnostics).unwrap();
        assert_eq!(serialized[0]["code"], "workflow_syntax_invalid");
        assert!(serialized[0]["line"].is_u64());
        assert!(serialized[0]["column"].is_u64());
        assert_eq!(serialized[0].as_object().unwrap().len(), 4);
        assert!(
            !serde_json::to_string(&serialized)
                .unwrap()
                .contains("private-workspace-sentinel")
        );
        remove_root(root);
    }
}
