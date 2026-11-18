use crate::model::CatalogSnapshot;
use anyhow::{Result, anyhow, ensure};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

const STORE_SCHEMA: &str = "v0.0.1:licoup:catalog-cache-store-1";
const MAX_STORE_BYTES: usize = 4 * 1024 * 1024;
const MAX_PARTITIONS: usize = 64;
const MAX_TOOLS_PER_PARTITION: usize = 4096;
static NEXT_TEMP_FILE: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoreEnvelope {
    schema_version: String,
    partitions: Vec<CatalogSnapshot>,
}

pub struct CatalogCacheStore {
    root: PathBuf,
    write_lock: Mutex<()>,
}

impl CatalogCacheStore {
    pub fn open(root: PathBuf) -> Result<Self> {
        fs::create_dir_all(&root)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
        }
        Ok(Self {
            root,
            write_lock: Mutex::new(()),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn persist_partition(&self, snapshot: &CatalogSnapshot) -> Result<()> {
        validate_snapshot(snapshot)?;
        let _guard = self
            .write_lock
            .lock()
            .map_err(|_| anyhow!("catalog_cache_store_lock_failed"))?;
        let mut partitions = self.load_partitions_unlocked()?;
        partitions.retain(|entry| entry.partition_key != snapshot.partition_key);
        partitions.push(snapshot.clone());
        ensure!(
            partitions.len() <= MAX_PARTITIONS,
            "catalog_cache_store_capacity"
        );
        self.write_all(&partitions)
    }

    pub fn remove_partition(&self, partition_key: &str) -> Result<()> {
        let key = partition_key.trim();
        let _guard = self
            .write_lock
            .lock()
            .map_err(|_| anyhow!("catalog_cache_store_lock_failed"))?;
        let mut partitions = self.load_partitions_unlocked()?;
        let before = partitions.len();
        partitions.retain(|entry| entry.partition_key != key);
        if partitions.len() == before {
            return Ok(());
        }
        self.write_all(&partitions)
    }

    pub fn purge_all(&self) -> Result<()> {
        let _guard = self
            .write_lock
            .lock()
            .map_err(|_| anyhow!("catalog_cache_store_lock_failed"))?;
        let path = self.store_path();
        if path.exists() {
            fs::remove_file(&path)?;
        }
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let name = entry.file_name();
            if name.to_string_lossy().starts_with("partitions.json.")
                && name.to_string_lossy().ends_with(".tmp")
            {
                fs::remove_file(entry.path())?;
            }
        }
        Ok(())
    }

    pub fn load_partitions(&self) -> Result<Vec<CatalogSnapshot>> {
        let _guard = self
            .write_lock
            .lock()
            .map_err(|_| anyhow!("catalog_cache_store_lock_failed"))?;
        self.load_partitions_unlocked()
    }

    fn load_partitions_unlocked(&self) -> Result<Vec<CatalogSnapshot>> {
        let path = self.store_path();
        if !path.exists() {
            return Ok(Vec::new());
        }
        let bytes = fs::read(&path)?;
        ensure!(
            bytes.len() <= MAX_STORE_BYTES,
            "catalog_cache_store_oversized"
        );
        let text =
            std::str::from_utf8(&bytes).map_err(|_| anyhow!("catalog_cache_store_invalid"))?;
        if text.trim().is_empty() {
            return Ok(Vec::new());
        }
        let envelope: StoreEnvelope =
            serde_json::from_str(text).map_err(|_| anyhow!("catalog_cache_store_invalid"))?;
        ensure!(
            envelope.schema_version == STORE_SCHEMA,
            "catalog_cache_store_schema_unknown"
        );
        ensure!(
            envelope.partitions.len() <= MAX_PARTITIONS,
            "catalog_cache_store_capacity"
        );
        let mut keys = HashSet::new();
        for snapshot in &envelope.partitions {
            validate_snapshot(snapshot)?;
            ensure!(
                keys.insert(snapshot.partition_key.clone()),
                "catalog_cache_store_duplicate_partition"
            );
        }
        Ok(envelope.partitions)
    }

    fn write_all(&self, partitions: &[CatalogSnapshot]) -> Result<()> {
        let envelope = StoreEnvelope {
            schema_version: STORE_SCHEMA.to_string(),
            partitions: partitions.to_vec(),
        };
        let content = serde_json::to_string_pretty(&envelope)?;
        ensure!(
            content.len() <= MAX_STORE_BYTES,
            "catalog_cache_store_oversized"
        );
        atomic_write_json(&self.store_path(), &content)
    }

    fn store_path(&self) -> PathBuf {
        self.root.join("partitions.json")
    }
}

fn atomic_write_json(path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let sequence = NEXT_TEMP_FILE.fetch_add(1, Ordering::Relaxed);
    let tmp = path.with_extension(format!("json.{sequence}.tmp"));
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| -> Result<()> {
        let mut file = options.open(&tmp)?;
        file.write_all(content.as_bytes())?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, path)?;
        #[cfg(unix)]
        if let Some(parent) = path.parent() {
            OpenOptions::new().read(true).open(parent)?.sync_all()?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

fn validate_snapshot(snapshot: &CatalogSnapshot) -> Result<()> {
    ensure!(
        !snapshot.partition_key.trim().is_empty(),
        "catalog_cache_store_partition_invalid"
    );
    ensure!(
        snapshot.source_revision >= 0 && snapshot.audience_revision >= 0,
        "catalog_cache_store_revision_invalid"
    );
    ensure!(
        !snapshot.catalog_revision.trim().is_empty(),
        "catalog_cache_store_revision_invalid"
    );
    ensure!(
        snapshot.tools.len() <= MAX_TOOLS_PER_PARTITION,
        "catalog_cache_store_tool_capacity"
    );
    ensure!(
        snapshot.tool_count == snapshot.tools.len(),
        "catalog_cache_store_tool_count_invalid"
    );
    let mut names = HashSet::new();
    for tool in &snapshot.tools {
        ensure!(
            !tool.name.trim().is_empty(),
            "catalog_cache_store_tool_invalid"
        );
        ensure!(
            names.insert(tool.name.clone()),
            "catalog_cache_store_duplicate_tool"
        );
    }
    let expected = crate::model::digest_catalog_snapshot(
        &snapshot.partition_key,
        snapshot.source_revision,
        &snapshot.catalog_revision,
        snapshot.audience_revision,
        &snapshot.tools,
    );
    ensure!(
        snapshot.digest == expected,
        "catalog_cache_store_digest_mismatch"
    );
    Ok(())
}
