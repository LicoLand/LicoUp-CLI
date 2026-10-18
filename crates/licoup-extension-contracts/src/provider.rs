//! C10: model providers — configuration, catalog keys, credentials, and the
//! stream.
//!
//! A provider owns a model protocol, its authentication, its model catalog and
//! its stream. A gateway is one consumer of providers and is not a prerequisite
//! for defining one, so a user who already has a local model endpoint never has
//! to install a gateway to reach it, and a provider that is no longer wanted does
//! not remove the ones another consumer is still using.
//!
//! The rules that matter most here are about not inventing facts:
//!
//! - **Unknown stays unknown.** A model whose context limit, tool support or
//!   reasoning options are not published by its vendor reports `None`, not a
//!   convenient `false` or a default window. There is no uniform table to flatter.
//! - **A price is an estimate reference, never an incurred cost.** Cost is an
//!   observation ([`crate::usage`]); a provider may point at a pricing source and
//!   may not declare what the user has spent.
//! - **Secrets are handles.** The configuration carries a `credential:` handle
//!   and never key material ([`find_inline_secret`]), and a handle issued for one
//!   endpoint is not returned for another ([`CredentialScope::authorizes`]).

use crate::refusal;
use licoup_application::{ApplicationFailure, is_namespaced};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

const STAGE: &str = "extension/provider";

/// The longest credential handle accepted.
pub const MAX_CREDENTIAL_REFERENCE_BYTES: usize = 128;

/// The prefix a host-issued credential handle uses.
pub const CREDENTIAL_SCHEME: &str = "credential:";

/// The longest vendor model id accepted.
pub const MAX_VENDOR_MODEL_ID_BYTES: usize = 160;

/// Where a model catalog comes from.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CatalogSource {
    /// The configuration lists the models. Most providers work this way and need
    /// no `modelProvider.models` call at all.
    Static,
    /// The provider discovers them, so it must implement
    /// `modelProvider.models`.
    Discovered,
}

/// One model as its vendor describes it.
///
/// Every optional field is [`Option`] because "the vendor did not say" is a real
/// answer. `None` means unknown, and no reader may substitute a default.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderModel {
    /// The vendor's own model id, such as `gpt-4o`. It is not namespaced: the
    /// catalog key already carries the provider.
    pub id: String,
    pub display_name: String,
    #[serde(default)]
    pub input_modalities: Vec<String>,
    #[serde(default)]
    pub output_modalities: Vec<String>,
    /// Unknown when the vendor publishes nothing.
    #[serde(default)]
    pub tools: Option<bool>,
    /// Whether the model takes reasoning options. Unknown when unpublished.
    #[serde(default)]
    pub reasoning: Option<bool>,
    /// Context limit in tokens. Unknown when unpublished — never a default window.
    #[serde(default)]
    pub context_tokens: Option<u32>,
    /// A namespaced estimate reference. Never an incurred cost, and never a
    /// number this configuration asserts the user has spent.
    #[serde(default)]
    pub pricing_source: Option<String>,
}

impl ProviderModel {
    pub fn validate(&self) -> Result<(), ApplicationFailure> {
        if self.id.is_empty()
            || self.id.len() > MAX_VENDOR_MODEL_ID_BYTES
            || self.display_name.is_empty()
        {
            return Err(refusal::new("provider_model_invalid", STAGE).with_field("models"));
        }
        if let Some(source) = &self.pricing_source
            && !is_namespaced(source)
        {
            return Err(
                refusal::new("provider_model_invalid", STAGE).with_field("models.pricingSource")
            );
        }
        Ok(())
    }

    /// The modality list a caller may act on, or `None` when the vendor published
    /// none. An empty list is not the same as unknown and is reported as empty.
    pub fn input_modalities(&self) -> Option<&[String]> {
        Some(self.input_modalities.as_slice())
    }
}

/// The key a model is addressed by.
///
/// A human alias is a separate mapping ([`AliasTable`]) because two providers can
/// want the same readable name and neither may take it from the other.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCatalogKey {
    pub provider_id: String,
    /// The provider generation the host admitted. A catalog update makes a new
    /// generation; it does not rewrite the old one.
    pub provider_generation: u64,
    pub vendor_model_id: String,
}

/// One provider configuration.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConfig {
    /// The wire identifier, so a configuration is self-identifying.
    pub schema: String,
    /// The namespaced provider identity.
    pub id: String,
    pub display_name: String,
    /// The endpoint. Nothing is preset for the user and no allow-list is implied:
    /// a provider that speaks a compatible dialect is configurable by
    /// configuration alone.
    pub base_url: String,
    /// The dialect the endpoint speaks, in the provider's own words.
    pub api_dialect: String,
    /// A host-issued handle. Key material never appears here.
    #[serde(default)]
    pub credential_ref: Option<String>,
    pub config_revision: u32,
    pub catalog_source: CatalogSource,
    #[serde(default)]
    pub models: Vec<ProviderModel>,
    /// Versioned compatibility options. Semantic parameters only.
    #[serde(default)]
    pub compat: BTreeMap<String, Value>,
    /// Present when the dialect is not a compatible one, and required then: a
    /// custom API supplies its own stream adapter instead of pretending to be a
    /// dialect it is not.
    #[serde(default)]
    pub stream_adapter: Option<String>,
}

impl ProviderConfig {
    /// The endpoint origin — scheme, host and port — with no path.
    pub fn origin(&self) -> Option<String> {
        endpoint_origin(&self.base_url)
    }

    /// Whether this endpoint speaks a dialect the host already understands.
    ///
    /// A dialect is compatible only if it is one of the published ones. Anything
    /// else is a custom dialect, and a custom dialect owes the platform a stream
    /// adapter rather than an imitation of a compatible API.
    pub fn dialect(&self) -> Dialect {
        if is_compatible_dialect(&self.api_dialect) {
            Dialect::Compatible
        } else {
            Dialect::Custom
        }
    }

    /// The catalog key one of this provider's models is addressed by.
    pub fn catalog_key(
        &self,
        provider_generation: u64,
        vendor_model_id: impl Into<String>,
    ) -> ModelCatalogKey {
        ModelCatalogKey {
            provider_id: self.id.clone(),
            provider_generation,
            vendor_model_id: vendor_model_id.into(),
        }
    }

    /// Structural validation, including the rule that a custom dialect is not
    /// allowed to stand in for a compatible one.
    pub fn validate(&self) -> Result<(), ApplicationFailure> {
        if self.schema != crate::wire::PROVIDER {
            return Err(refusal::new("provider_config_invalid", STAGE).with_field("schema"));
        }
        if !is_namespaced(&self.id) {
            return Err(refusal::new("provider_config_invalid", STAGE).with_field("id"));
        }
        if self.display_name.is_empty() || self.config_revision < 1 {
            return Err(refusal::new("provider_config_invalid", STAGE).with_field("configRevision"));
        }
        if self.origin().is_none() {
            return Err(refusal::new("provider_base_url_invalid", STAGE).with_field("baseUrl"));
        }
        if let Some(reference) = &self.credential_ref
            && !credential_ref_is_handle(reference)
        {
            return Err(
                refusal::new("provider_credential_ref_invalid", STAGE).with_field("credentialRef")
            );
        }
        if self.dialect() == Dialect::Custom
            && self
                .stream_adapter
                .as_deref()
                .is_none_or(|adapter| adapter.is_empty())
        {
            return Err(refusal::actionable(
                "provider_custom_dialect_requires_adapter",
                STAGE,
                "streamAdapter",
            )
            .with_presentation_arg("apiDialect", &self.api_dialect));
        }
        for model in &self.models {
            model.validate()?;
        }
        Ok(())
    }

    /// Read one configuration from its wire form, refusing key material before it
    /// is ever deserialized into a structure the host would keep.
    pub fn from_value(value: Value) -> Result<Self, ApplicationFailure> {
        if let Some(field) = find_inline_secret(&value) {
            return Err(
                refusal::actionable("provider_inline_secret_refused", STAGE, &field)
                    .with_presentation_arg("expected", "credentialRef"),
            );
        }
        let config: Self = serde_json::from_value(value)
            .map_err(|_| refusal::new("provider_config_invalid", STAGE).with_field("provider"))?;
        config.validate()?;
        Ok(config)
    }
}

/// How well the host already understands a provider's dialect.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Dialect {
    /// A published compatible dialect, which needs no adapter code.
    Compatible,
    /// The provider's own API, which owes its own stream adapter.
    Custom,
}

/// The compatible dialects this contract publishes.
pub const COMPATIBLE_DIALECTS: &[&str] = &["openai-chat-compatible", "openai-responses-compatible"];

pub fn is_compatible_dialect(dialect: &str) -> bool {
    COMPATIBLE_DIALECTS.contains(&dialect)
}

/// The origin of a base URL — scheme, host and port, without path or query.
///
/// Only `http` and `https` are accepted: a provider endpoint the client is asked
/// to send credentials to is an HTTP endpoint, and anything else is a
/// configuration mistake worth naming.
///
/// Parsing is the standard URL parser's job. The origin is the scope a
/// credential handle was issued for, so a URL that only resembles one — a
/// malformed port, embedded user info, a bare word, a backslash an ad-hoc split
/// would have treated as a separator — is refused rather than silently
/// normalized into an origin the user never configured.
pub fn endpoint_origin(base_url: &str) -> Option<String> {
    // The published schema requires the `http://` or `https://` form verbatim
    // and a non-empty authority right after it, and a forgiving parser would
    // otherwise read `http:\host\path` or `http:///path` as an origin no schema
    // ever published.
    let rest = base_url
        .strip_prefix("http://")
        .or_else(|| base_url.strip_prefix("https://"))?;
    if rest.is_empty() || rest.starts_with(['/', '@', '?', '#']) {
        return None;
    }
    let parsed = url::Url::parse(base_url).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return None;
    }
    let mut origin = format!("{}://{}", parsed.scheme(), parsed.host_str()?);
    if let Some(port) = parsed.port() {
        origin.push_str(&format!(":{port}"));
    }
    Some(origin)
}

/// Whether `reference` is a host-issued credential handle rather than key
/// material pasted into a configuration file.
pub fn credential_ref_is_handle(reference: &str) -> bool {
    let Some(token) = reference.strip_prefix(CREDENTIAL_SCHEME) else {
        return false;
    };
    !token.is_empty()
        && token.len() <= MAX_CREDENTIAL_REFERENCE_BYTES
        && token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        && !token.to_ascii_lowercase().starts_with("sk")
}

/// The key material field names a configuration may not carry, in any spelling.
const SECRET_FIELD_NAMES: &[&str] = &[
    "apikey",
    "apisecret",
    "accesstoken",
    "authtoken",
    "authorization",
    "bearertoken",
    "clientsecret",
    "password",
    "refreshtoken",
    "secret",
    "secretkey",
    "token",
];

/// The dotted path of the first field in `value` that carries key material, if
/// any.
///
/// This is checked before deserialization: an unknown field is ignored by a
/// tolerant reader, and "ignored" is not a safe outcome for a secret that would
/// otherwise be logged, stored, or written into a graph.
pub fn find_inline_secret(value: &Value) -> Option<String> {
    fn walk(value: &Value, path: &str, found: &mut Option<String>) {
        if found.is_some() {
            return;
        }
        match value {
            Value::Object(map) => {
                for (key, nested) in map {
                    let normalized: String = key
                        .chars()
                        .filter(|character| *character != '_' && *character != '-')
                        .flat_map(char::to_lowercase)
                        .collect();
                    if SECRET_FIELD_NAMES.contains(&normalized.as_str()) {
                        *found = Some(format!("{path}{key}"));
                        return;
                    }
                    walk(nested, &format!("{path}{key}."), found);
                }
            }
            Value::Array(items) => {
                for (index, nested) in items.iter().enumerate() {
                    walk(nested, &format!("{path}{index}."), found);
                }
            }
            _ => {}
        }
    }

    let mut found = None;
    walk(value, "", &mut found);
    found
}

/// A readable name mapped onto a catalog key.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelAlias {
    pub name: String,
    pub target: ModelCatalogKey,
}

/// The alias table.
///
/// Aliases are how a user says "the fast one". Two providers may both want that
/// name; the table is what decides, and the only thing that changes the decision
/// is an explicit replacement. Removing a provider removes that provider's
/// aliases and leaves every other provider's alone, and restores no earlier
/// default: a target that is gone does not silently come back.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AliasTable {
    entries: BTreeMap<String, ModelCatalogKey>,
}

impl AliasTable {
    pub fn new() -> Self {
        Self::default()
    }

    /// Set or explicitly replace an alias. Replacement is the only way the future
    /// selection of that name changes.
    pub fn set(&mut self, name: impl Into<String>, target: ModelCatalogKey) {
        self.entries.insert(name.into(), target);
    }

    pub fn resolve(&self, name: &str) -> Option<&ModelCatalogKey> {
        self.entries.get(name)
    }

    /// Remove every alias pointing at one provider, and nothing else.
    pub fn remove_provider(&mut self, provider_id: &str) {
        self.entries
            .retain(|_, target| target.provider_id != provider_id);
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &ModelCatalogKey)> {
        self.entries
            .iter()
            .map(|(name, target)| (name.as_str(), target))
    }
}

/// The endpoint scope a credential handle was issued for.
///
/// A handle is resolved per provider *and* per endpoint origin. Pointing a
/// provider at a new base URL does not hand the new origin the old endpoint's
/// secret, which is the whole reason the origin is part of the scope.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialScope {
    pub reference: String,
    pub provider_id: String,
    pub origin: String,
}

impl CredentialScope {
    /// Whether this scope may be used for `config`.
    pub fn authorizes(&self, config: &ProviderConfig) -> bool {
        self.provider_id == config.id
            && config.origin().is_some_and(|origin| origin == self.origin)
            && config.credential_ref.as_deref() == Some(self.reference.as_str())
    }
}

/// Whether a value is an empty compatibility-option map, for readers that report
/// "no options declared" rather than "options are unsupported".
pub fn has_compat_options(compat: &BTreeMap<String, Value>) -> bool {
    !compat.is_empty()
}

/// A configuration object's fields, for callers that need the key set without a
/// second definition of it.
pub fn field_names(value: &Value) -> Vec<&str> {
    value
        .as_object()
        .map(|map: &Map<String, Value>| map.keys().map(String::as_str).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> ProviderConfig {
        ProviderConfig {
            schema: crate::wire::PROVIDER.to_owned(),
            id: "example.models.private".to_owned(),
            display_name: "Local endpoint".to_owned(),
            base_url: "http://127.0.0.1:8000/v1".to_owned(),
            api_dialect: "openai-chat-compatible".to_owned(),
            credential_ref: Some("credential:local-endpoint".to_owned()),
            config_revision: 1,
            catalog_source: CatalogSource::Static,
            models: vec![ProviderModel {
                id: "my-model".to_owned(),
                display_name: "My Model".to_owned(),
                input_modalities: vec!["text".to_owned()],
                output_modalities: vec!["text".to_owned()],
                tools: None,
                reasoning: None,
                context_tokens: None,
                pricing_source: None,
            }],
            compat: BTreeMap::new(),
            stream_adapter: None,
        }
    }

    #[test]
    fn unknown_model_facts_stay_unknown() {
        let model = &config().models[0];
        assert_eq!(model.tools, None);
        assert_eq!(model.reasoning, None);
        assert_eq!(model.context_tokens, None);
        assert_eq!(model.pricing_source, None);
    }

    #[test]
    fn compatible_dialects_need_no_adapter_and_custom_ones_do() {
        let compatible = config();
        assert_eq!(compatible.dialect(), Dialect::Compatible);
        assert!(compatible.validate().is_ok());

        let mut custom = config();
        custom.api_dialect = "vendor.example/native".to_owned();
        let failure = custom.validate().expect_err("custom dialect");
        assert_eq!(failure.code, "provider_custom_dialect_requires_adapter");

        custom.stream_adapter = Some("vendor.example/stream".to_owned());
        assert!(custom.validate().is_ok());
    }

    #[test]
    fn key_material_is_refused_before_it_reaches_a_record() {
        let wire = serde_json::json!({
            "schema": crate::wire::PROVIDER,
            "id": "example.models.private",
            "displayName": "Local endpoint",
            "baseUrl": "https://models.example.invalid/v1",
            "apiDialect": "openai-chat-compatible",
            "configRevision": 1,
            "catalogSource": "static",
            "models": [],
            "headers": { "Authorization": "Bearer placeholder" }
        });
        let failure = ProviderConfig::from_value(wire).expect_err("inline secret");
        assert_eq!(failure.code, "provider_inline_secret_refused");

        // Key material pasted into the field meant for a handle.
        let mut pasted = config();
        pasted.credential_ref = Some("raw-key-material".to_owned());
        assert!(pasted.validate().is_err());

        // A handle whose token still looks like key material is not a handle.
        assert!(!credential_ref_is_handle("credential:sk-example"));
        assert!(credential_ref_is_handle("credential:local-endpoint"));
    }

    #[test]
    fn an_origin_is_canonical_and_a_look_alike_url_is_refused() {
        assert_eq!(
            endpoint_origin("http://127.0.0.1:8000/v1").as_deref(),
            Some("http://127.0.0.1:8000")
        );
        assert_eq!(
            endpoint_origin("https://Models.Example.Invalid:443/v1").as_deref(),
            Some("https://models.example.invalid"),
            "a default port and host casing are normalized, not a second origin"
        );
        assert_eq!(
            endpoint_origin("http://[::1]:8080/v1").as_deref(),
            Some("http://[::1]:8080")
        );
        // This is intentionally malformed URL syntax, not a machine path.
        let backslash_url = ["http:", "models.example.invalid", "v1"].join("\\");
        for refused in [
            "http://user:placeholder@models.example.invalid/v1",
            "http://models.example.invalid:notaport/v1",
            "http://models.example.invalid:99999/v1",
            backslash_url.as_str(),
            "HTTP://models.example.invalid/v1",
            "ftp://models.example.invalid/v1",
            "http:///v1",
            "http:models.example.invalid",
            "models.example.invalid/v1",
            "",
        ] {
            assert_eq!(endpoint_origin(refused), None, "{refused} is not an origin");
        }
    }

    #[test]
    fn a_new_endpoint_does_not_inherit_the_old_secret() {
        let scope = CredentialScope {
            reference: "credential:local-endpoint".to_owned(),
            provider_id: "example.models.private".to_owned(),
            origin: "http://127.0.0.1:8000".to_owned(),
        };
        let original = config();
        assert!(scope.authorizes(&original));

        let mut moved = config();
        moved.base_url = "https://elsewhere.example.invalid/v1".to_owned();
        assert!(!scope.authorizes(&moved));
    }

    #[test]
    fn removing_a_provider_leaves_other_aliases_alone_and_restores_no_default() {
        let mut table = AliasTable::new();
        table.set("fast", config().catalog_key(1, "my-model"));
        table.set(
            "other",
            ModelCatalogKey {
                provider_id: "example.models.other".to_owned(),
                provider_generation: 4,
                vendor_model_id: "their-model".to_owned(),
            },
        );
        table.remove_provider("example.models.private");
        assert!(table.resolve("fast").is_none());
        assert!(table.resolve("other").is_some());
        assert_eq!(table.len(), 1);
    }

    #[test]
    fn catalog_keys_bind_provider_generation_and_vendor_id() {
        let key = config().catalog_key(7, "my-model");
        assert_eq!(key.provider_id, "example.models.private");
        assert_eq!(key.provider_generation, 7);
        assert_eq!(key.vendor_model_id, "my-model");
        assert_ne!(key, config().catalog_key(8, "my-model"));
    }
}
