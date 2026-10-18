use super::{CatalogDocument, ProviderDocument};
use anyhow::{Result, anyhow, ensure};
use flate2::read::GzDecoder;
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
use std::io::{Cursor, Read};
use std::time::Duration;

pub(super) const REPOSITORY_SOURCE: &str =
    "https://codeload.github.com/anomalyco/models.dev/tar.gz/refs/heads/dev";
const MAX_DOWNLOAD_BYTES: u64 = 64 * 1024 * 1024;

pub(super) struct DownloadedCatalog {
    pub catalog: CatalogDocument,
    pub source: String,
    pub skipped_entries: usize,
}

pub(super) fn download() -> Result<DownloadedCatalog> {
    // The public source distribution preserves explicit base_model links.
    // Generated catalog JSON drops them and is not an equivalent authority.
    let bytes = fetch(REPOSITORY_SOURCE)?;
    from_archive(&bytes)
}

fn fetch(url: &str) -> Result<Vec<u8>> {
    let response = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(30))
        .redirects(3)
        .build()
        .get(url)
        .set("User-Agent", "LicoUp-ModelRegistry")
        .call()
        .map_err(|_| anyhow!("model_registry_source_unavailable"))?;
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(MAX_DOWNLOAD_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| anyhow!("model_registry_source_unavailable"))?;
    ensure!(
        bytes.len() as u64 <= MAX_DOWNLOAD_BYTES,
        "model_registry_source_too_large"
    );
    Ok(bytes)
}

enum SourceEntry {
    Text(String),
    Link(String),
}

pub(super) fn from_archive(bytes: &[u8]) -> Result<DownloadedCatalog> {
    let mut archive = tar::Archive::new(GzDecoder::new(Cursor::new(bytes)));
    let mut entries = BTreeMap::new();
    let mut decoded_bytes = 0u64;
    for entry in archive.entries()? {
        let mut entry = entry?;
        let full = entry.path()?.to_string_lossy().into_owned();
        let Some((_, relative)) = full.split_once('/') else {
            continue;
        };
        if !relative.ends_with(".toml")
            || !(relative.starts_with("models/") || relative.starts_with("providers/"))
        {
            continue;
        }
        let name = relative.to_owned();
        if entry.header().entry_type().is_file() {
            decoded_bytes = decoded_bytes.saturating_add(entry.size());
            ensure!(
                decoded_bytes <= MAX_DOWNLOAD_BYTES,
                "model_registry_source_too_large"
            );
            let mut text = String::new();
            entry.read_to_string(&mut text)?;
            entries.insert(name, SourceEntry::Text(text));
        } else if entry.header().entry_type().is_symlink()
            && let Some(link) = entry.link_name()?
            && let Some(target) = relative_link(&name, &link.to_string_lossy())
        {
            entries.insert(name, SourceEntry::Link(target));
        }
    }
    let mut catalog = CatalogDocument::default();
    let mut skipped_entries = 0;
    for path in entries.keys() {
        let Some(text) = resolve_text(&entries, path) else {
            skipped_entries += 1;
            continue;
        };
        let parsed: toml::Value =
            toml::from_str(text).map_err(|_| anyhow!("model_registry_source_invalid"))?;
        let value = serde_json::to_value(parsed)?;
        if let Some(id) = path
            .strip_prefix("models/")
            .and_then(|p| p.strip_suffix(".toml"))
        {
            catalog.models.insert(id.to_owned(), value);
        } else if let Some(provider_path) = path.strip_prefix("providers/")
            && let Some((provider_id, rest)) = provider_path.split_once('/')
        {
            let provider = catalog.providers.entry(provider_id.to_owned()).or_default();
            if rest == "provider.toml" {
                provider.name = value
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or(provider_id)
                    .to_owned();
            } else if let Some(id) = rest
                .strip_prefix("models/")
                .and_then(|p| p.strip_suffix(".toml"))
            {
                provider.models.insert(id.to_owned(), value);
            }
        }
    }
    catalog
        .providers
        .retain(|_, provider: &mut ProviderDocument| {
            !provider.name.is_empty() && !provider.models.is_empty()
        });
    ensure!(
        !catalog.models.is_empty() && !catalog.providers.is_empty(),
        "model_registry_source_invalid"
    );
    Ok(DownloadedCatalog {
        catalog,
        source: REPOSITORY_SOURCE.to_owned(),
        skipped_entries,
    })
}

fn resolve_text<'a>(entries: &'a BTreeMap<String, SourceEntry>, start: &'a str) -> Option<&'a str> {
    let mut current = start;
    let mut seen = HashSet::new();
    while seen.insert(current) {
        match entries.get(current)? {
            SourceEntry::Text(text) => return Some(text),
            SourceEntry::Link(target) => current = target,
        }
    }
    None
}

fn relative_link(path: &str, link: &str) -> Option<String> {
    if link.starts_with('/') || link.contains('\\') {
        return None;
    }
    let mut parts = path.split('/').collect::<Vec<_>>();
    parts.pop();
    for part in link.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            _ => parts.push(part),
        }
    }
    Some(parts.join("/"))
}
