use super::{CanonicalModel, CatalogDocument};
use anyhow::{Result, ensure};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

#[derive(Clone, Copy, Debug)]
enum Alias {
    Unique(usize),
    Ambiguous,
}

/// Immutable indexes shared by every consumer for the lifetime of a report.
#[derive(Clone, Debug)]
pub struct RegistrySnapshot {
    pub(crate) models: Vec<CanonicalModel>,
    canonical_ids: HashMap<String, Alias>,
    canonical_aliases: HashMap<String, Alias>,
    historical_aliases: HashMap<String, Alias>,
    provider_ids: HashMap<String, Alias>,
    provider_names: HashMap<String, Alias>,
    provider_selectors: HashMap<(usize, String), Alias>,
    aliases: HashMap<String, Alias>,
    wrappers: HashSet<String>,
    pub(crate) provider_count: usize,
    revision: String,
}

impl RegistrySnapshot {
    pub fn from_catalog(value: Value) -> Result<Self> {
        let catalog: CatalogDocument = serde_json::from_value(value)?;
        Self::from_document(&catalog)
    }

    /// Build a snapshot against explicitly declared Agent wrapper labels.
    ///
    /// Production reads the labels the composed port answers. A caller that
    /// states its own declaration fixture — a test, or any caller resolving
    /// against a synthetic catalog — names them here instead of inheriting a
    /// host's declarations.
    pub fn from_catalog_with_wrappers(
        value: Value,
        wrappers: impl IntoIterator<Item = impl AsRef<str>>,
    ) -> Result<Self> {
        let catalog: CatalogDocument = serde_json::from_value(value)?;
        Self::from_document_with(&catalog, &normalize_wrappers(wrappers))
    }

    /// An empty snapshot whose wrapper labels are the ones this process
    /// composed.
    pub fn empty() -> Self {
        Self::empty_with(&normalize_wrappers(
            (crate::port::model_catalog_port().agent_wrapper_labels)()
        ))
    }

    /// An empty snapshot against explicitly declared Agent wrapper labels.
    pub fn empty_with_wrappers(wrappers: impl IntoIterator<Item = impl AsRef<str>>) -> Self {
        Self::empty_with(&normalize_wrappers(wrappers))
    }

    pub(crate) fn empty_with(wrappers: &HashSet<String>) -> Self {
        let wrappers = wrappers.clone();
        Self {
            models: Vec::new(),
            canonical_ids: HashMap::new(),
            canonical_aliases: HashMap::new(),
            historical_aliases: HashMap::new(),
            provider_ids: HashMap::new(),
            provider_names: HashMap::new(),
            provider_selectors: HashMap::new(),
            aliases: HashMap::new(),
            wrappers,
            provider_count: 0,
            revision: String::new(),
        }
    }

    pub(crate) fn from_document(catalog: &CatalogDocument) -> Result<Self> {
        Self::from_document_with(
            catalog,
            (crate::port::model_catalog_port().agent_wrapper_labels)(),
        )
    }

    pub(crate) fn from_document_with(
        catalog: &CatalogDocument,
        wrappers: impl IntoIterator<Item = impl AsRef<str>>,
    ) -> Result<Self> {
        ensure!(!catalog.models.is_empty(), "model_registry_catalog_empty");
        let mut result = Self::empty_with(&normalize_wrappers(wrappers));
        result.provider_count = catalog.providers.len();
        let mut model_ids = BTreeMap::<String, usize>::new();
        let mut labs = HashSet::new();
        for (id, facts) in &catalog.models {
            let Some((lab, _)) = id.split_once('/') else {
                continue;
            };
            labs.insert(lab.to_owned());
            result.insert_model(&mut model_ids, id, facts);
        }
        // Lab-owned endpoints can supply model facts absent from models/.
        // Relay-only rows never establish a lab identity by name guessing.
        for (provider_index, (provider_id, provider)) in catalog.providers.iter().enumerate() {
            result.wrappers.insert(normalize(provider_id));
            result.wrappers.insert(normalize(&provider.name));
            insert_index(
                &mut result.provider_ids,
                provider_id.to_lowercase(),
                provider_index,
            );
            if !provider.name.trim().is_empty() {
                insert_index(
                    &mut result.provider_names,
                    normalize(&provider.name),
                    provider_index,
                );
            }
            if !labs.contains(provider_id) {
                continue;
            }
            for (id, facts) in &provider.models {
                if facts.get("base_model").and_then(Value::as_str).is_none() {
                    let canonical = if id.starts_with(&format!("{provider_id}/")) {
                        id.clone()
                    } else {
                        format!("{provider_id}/{id}")
                    };
                    result.insert_model(&mut model_ids, &canonical, facts);
                }
            }
        }
        // Stable model IDs/names must not acquire the changing destination of
        // a provider's floating endpoint alias when projecting older usage.
        result.canonical_aliases = result.aliases.clone();
        let mut provider_aliases = Vec::new();
        for (provider_index, (provider_id, provider)) in catalog.providers.iter().enumerate() {
            for (id, facts) in &provider.models {
                let explicit = facts.get("base_model").and_then(Value::as_str);
                let resolved = explicit
                    .and_then(|base| model_ids.get(base).copied())
                    .or_else(|| {
                        if explicit.is_some() {
                            return None;
                        }
                        result.unique_facts_match(id, facts)
                    });
                provider_aliases.push((provider_index, provider_id, provider, id, facts, resolved));
            }
        }
        // Resolve names against canonical/lab facts before adding relay-only
        // rows. Otherwise iteration order could merge unrelated relay models.
        for (provider_index, provider_id, provider, id, facts, resolved) in provider_aliases {
            let index = match resolved {
                Some(index) => index,
                None if facts.get("base_model").and_then(Value::as_str).is_none() => {
                    let canonical = format!("{provider_id}/{id}");
                    result.insert_model(&mut model_ids, &canonical, facts)
                }
                None => continue,
            };
            if facts.get("base_model").and_then(Value::as_str).is_some()
                && let Some(key) = stable_version_alias(id, &result.models[index])
            {
                insert_index(&mut result.historical_aliases, key, index);
            }
            result.insert_alias(id, index);
            insert_index(
                &mut result.provider_selectors,
                (provider_index, id.to_lowercase()),
                index,
            );
            result.insert_alias(&format!("{provider_id}/{id}"), index);
            result.insert_alias(&format!("{} {id}", provider.name), index);
            if let Some(name) = facts.get("name").and_then(Value::as_str) {
                result.insert_alias(name, index);
                result.insert_alias(&format!("{} {name}", provider.name), index);
            }
        }
        ensure!(!result.models.is_empty(), "model_registry_catalog_empty");
        result.revision = format!("{:x}", Sha256::digest(serde_json::to_vec(catalog)?));
        Ok(result)
    }

    pub fn revision(&self) -> &str {
        &self.revision
    }

    /// Resolve a published canonical ID without following provider routes or
    /// display aliases. Confirmed execution-mode identities can be corrected
    /// even after their original directory entry has disappeared.
    pub fn canonical_model(&self, id: &str) -> Option<&CanonicalModel> {
        match self.canonical_ids.get(&id.to_lowercase()) {
            Some(Alias::Unique(index)) => Some(&self.models[*index]),
            Some(Alias::Ambiguous) => None,
            None if id.contains('/') => confirmed_execution_model(&normalize(id)),
            None => None,
        }
    }

    /// Catalog provider labels and admitted Agent labels identify a source,
    /// not a model. Qualified selectors remain resolvable model evidence.
    pub fn is_provider_label(&self, value: &str) -> bool {
        let key = normalize(value);
        !key.is_empty() && self.wrappers.contains(&key)
    }

    /// Only unique model evidence resolves. Unknown versions, modalities,
    /// provider-only labels, and conflicting aliases remain unclassified.
    pub fn resolve(&self, raw: &str, source_agent_id: Option<&str>) -> Option<&CanonicalModel> {
        self.resolve_with_provider(raw, None, source_agent_id)
    }

    /// A recorded serving provider scopes native selector IDs, including IDs
    /// containing a lab prefix. Agent membership does not establish a serving
    /// provider. Canonical output IDs keep their independent identity index.
    pub fn resolve_with_provider(
        &self,
        raw: &str,
        provider_id: Option<&str>,
        source_agent_id: Option<&str>,
    ) -> Option<&CanonicalModel> {
        self.resolve_model(raw, provider_id, source_agent_id, false)
    }

    /// Historical usage cannot infer an earlier destination from a provider's
    /// current opaque route. Only stable model facts and explicit aliases that
    /// preserve the target's full version and variant are admitted here.
    pub fn resolve_historical(
        &self,
        raw: &str,
        provider_id: Option<&str>,
        source_agent_id: Option<&str>,
    ) -> Option<&CanonicalModel> {
        self.resolve_model(raw, provider_id, source_agent_id, true)
    }

    fn resolve_model(
        &self,
        raw: &str,
        provider_id: Option<&str>,
        source_agent_id: Option<&str>,
        historical: bool,
    ) -> Option<&CanonicalModel> {
        let raw = raw.trim();
        let key = normalize(raw);
        if key.is_empty() {
            return None;
        }
        let provider = if historical {
            None
        } else {
            provider_id.and_then(|id| self.provider_index(id))
        };
        if self.is_provider_label(raw) {
            if historical {
                return None;
            }
            return provider.and_then(|provider| {
                match self.provider_selectors.get(&(provider, raw.to_lowercase())) {
                    Some(Alias::Unique(index)) => Some(&self.models[*index]),
                    Some(Alias::Ambiguous) | None => None,
                }
            });
        }
        let mut pending = VecDeque::from([raw.to_owned()]);
        let mut seen = HashSet::new();
        let mut resolved: Option<&CanonicalModel> = None;
        let source = source_agent_id.map(normalize);
        while let Some(candidate) = pending.pop_front() {
            if !seen.insert(candidate.clone()) {
                continue;
            }
            let key = normalize(&candidate);
            let exact = candidate.to_lowercase();
            let model = match self
                .canonical_ids
                .get(&exact)
                .or_else(|| {
                    self.canonical_aliases
                        .get(&key)
                        .filter(|alias| historical || matches!(alias, Alias::Unique(_)))
                })
                .or_else(|| {
                    if historical {
                        self.historical_aliases.get(&key)
                    } else {
                        provider.and_then(|provider| {
                            self.provider_selectors.get(&(provider, exact.clone()))
                        })
                    }
                })
                .or_else(|| {
                    if !historical && provider_id.is_none() {
                        self.aliases.get(&key)
                    } else {
                        None
                    }
                }) {
                Some(Alias::Ambiguous) => return None,
                Some(Alias::Unique(index)) => Some(&self.models[*index]),
                None => confirmed_execution_model(&key),
            };
            if let Some(model) = model {
                if resolved.is_some_and(|previous| previous.id != model.id) {
                    return None;
                }
                resolved = Some(model);
                continue;
            }
            if let Some(unwrapped) = strip_suffix(&candidate, source.as_deref() == Some("cursor")) {
                pending.push_back(unwrapped.to_owned());
            }
            let mut prefix = String::new();
            let mut previous_digit = false;
            for (offset, character) in candidate.char_indices() {
                if character.is_alphanumeric() {
                    prefix.extend(character.to_lowercase());
                    previous_digit = character.is_ascii_digit();
                    continue;
                }
                if self.wrappers.contains(&prefix) || source.as_ref() == Some(&prefix) {
                    let rest =
                        candidate[offset..].trim_start_matches(|c: char| !c.is_alphanumeric());
                    if !rest.is_empty() {
                        pending.push_back(rest.to_owned());
                    }
                }
                if matches!(character, '.' | '-' | '_')
                    && previous_digit
                    && candidate[offset + character.len_utf8()..]
                        .chars()
                        .next()
                        .is_some_and(|next| next.is_ascii_digit())
                {
                    prefix.push('.');
                }
                previous_digit = false;
            }
        }
        resolved
    }

    fn provider_index(&self, id: &str) -> Option<usize> {
        match self
            .provider_ids
            .get(&id.trim().to_lowercase())
            .or_else(|| self.provider_names.get(&normalize(id)))
        {
            Some(Alias::Unique(index)) => Some(*index),
            Some(Alias::Ambiguous) | None => None,
        }
    }

    fn insert_model(
        &mut self,
        ids: &mut BTreeMap<String, usize>,
        id: &str,
        facts: &Value,
    ) -> usize {
        if let Some(index) = ids.get(id) {
            return *index;
        }
        let confirmed = confirmed_execution_model(&normalize(id));
        let canonical_id = confirmed.map_or(id, |model| model.id.as_str());
        let name = facts.get("name").and_then(Value::as_str).unwrap_or(id);
        let index = if let Some(index) = ids.get(canonical_id) {
            *index
        } else {
            let index = self.models.len();
            self.models.push(confirmed.cloned().unwrap_or_else(|| {
                CanonicalModel {
                    id: id.to_owned(),
                    display_name: super::model_display_name(name),
                    lab_id: id.split('/').next().unwrap_or_default().to_owned(),
                    family: facts
                        .get("family")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                }
            }));
            index
        };
        if canonical_id != id {
            ids.insert(canonical_id.to_owned(), index);
            insert_index(&mut self.canonical_ids, canonical_id.to_lowercase(), index);
            self.insert_alias(canonical_id, index);
            if let Some((_, short)) = canonical_id.split_once('/') {
                self.insert_alias(short, index);
            }
        }
        ids.insert(id.to_owned(), index);
        insert_index(&mut self.canonical_ids, id.to_lowercase(), index);
        self.insert_alias(id, index);
        self.insert_alias(name, index);
        if let Some((_, short)) = id.split_once('/') {
            self.insert_alias(short, index);
        }
        index
    }

    fn insert_alias(&mut self, alias: &str, index: usize) {
        let key = normalize(alias);
        if key.is_empty() {
            return;
        }
        insert_index(&mut self.aliases, key, index);
    }

    fn unique_facts_match(&self, id: &str, facts: &Value) -> Option<usize> {
        let mut matched = None;
        for alias in [Some(id), facts.get("name").and_then(Value::as_str)]
            .into_iter()
            .flatten()
        {
            match self.aliases.get(&normalize(alias)) {
                Some(Alias::Ambiguous) => return None,
                Some(Alias::Unique(index)) => {
                    if matched.is_some_and(|previous| previous != *index) {
                        return None;
                    }
                    matched = Some(*index);
                }
                None => {}
            }
        }
        matched
    }
}

/// The Agent declaration labels a snapshot strips before matching. The
/// inventory owns them and production reads them from the composed port; an
/// unanswered port contributes none, which keeps resolution strict: a wrapper
/// prefix is never stripped on a guess.
fn normalize_wrappers(labels: impl IntoIterator<Item = impl AsRef<str>>) -> HashSet<String> {
    labels
        .into_iter()
        .map(|label| normalize(label.as_ref()))
        .collect()
}

/// Official identity facts supplement directory rows that list serving tiers
/// as separate models. This is deliberately not a generic `-fast` rule:
/// other products use Fast as a distinct model name.
/// https://cursor.com/docs/models/cursor-composer-2-5
fn confirmed_execution_model(key: &str) -> Option<&'static CanonicalModel> {
    if !matches!(
        key,
        "composer2.5" | "composer2.5fast" | "cursorcomposer2.5" | "cursorcomposer2.5fast"
    ) {
        return None;
    }
    static COMPOSER: std::sync::OnceLock<CanonicalModel> = std::sync::OnceLock::new();
    Some(COMPOSER.get_or_init(|| CanonicalModel {
        id: "cursor/composer-2.5".to_owned(),
        display_name: "Composer 2.5".to_owned(),
        lab_id: "cursor".to_owned(),
        family: Some("composer".to_owned()),
    }))
}

fn insert_index<K: Eq + std::hash::Hash>(indexes: &mut HashMap<K, Alias>, key: K, index: usize) {
    indexes
        .entry(key)
        .and_modify(|previous| {
            if !matches!(previous, Alias::Unique(existing) if *existing == index) {
                *previous = Alias::Ambiguous;
            }
        })
        .or_insert(Alias::Unique(index));
}

fn normalize(value: &str) -> String {
    let mut normalized = String::new();
    let mut characters = value.chars().peekable();
    let mut previous_digit = false;
    while let Some(character) = characters.next() {
        if character.is_alphanumeric() {
            normalized.extend(character.to_lowercase());
        } else if matches!(character, '.' | '-' | '_')
            && previous_digit
            && characters.peek().is_some_and(char::is_ascii_digit)
        {
            normalized.push('.');
        }
        previous_digit = character.is_ascii_digit();
    }
    normalized
}

fn stable_version_alias(alias: &str, model: &CanonicalModel) -> Option<String> {
    let mut alias = alias;
    while let Some(unwrapped) = strip_suffix(alias, false) {
        alias = unwrapped;
    }
    let key = normalize(alias);
    if !key.chars().any(|c| c.is_ascii_digit()) {
        return None;
    }
    let short_id = model.id.split_once('/')?.1;
    for name in [short_id, &model.display_name] {
        let mut boundary = true;
        for (offset, character) in name.char_indices() {
            if boundary && normalize(&name[offset..]) == key {
                return Some(key);
            }
            // A suffix must retain every numeric version component and every
            // following variant, including dated revisions and modalities.
            if character.is_ascii_digit() {
                break;
            }
            boundary = !character.is_alphanumeric();
        }
    }
    None
}

fn strip_suffix(value: &str, allow_thinking: bool) -> Option<&str> {
    static SUFFIX: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let expression = SUFFIX.get_or_init(|| regex::Regex::new(
        r"(?i)(?:[-_ .]+|\s*[\[(])(?:(?:(?:extra[-_ ]+)?high|xhigh|low|medium|max|ultra|minimal|none)(?:[-_ ]+fast)?|(?P<thinking>thinking)|[0-9]+(?:\.[0-9]+)?[km](?:[-_ ]*context)?)[\])]?$",
    ).expect("fixed model wrapper expression"));
    let captures = expression.captures(value)?;
    if !allow_thinking && captures.name("thinking").is_some() {
        return None;
    }
    let found = captures.get(0)?;
    let unwrapped = value[..found.start()].trim_end();
    (!unwrapped.is_empty()).then_some(unwrapped)
}
