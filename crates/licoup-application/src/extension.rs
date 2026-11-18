//! Extension catalog: namespaced identity, the capability descriptor, and the
//! discovered-versus-required rules.
//!
//! The catalog itself belongs to the host that discovers it: the native owner
//! generates the tool catalog and everything installed with it, and an adapter
//! such as the MCP only projects what that owner published. This module
//! therefore holds no registry, no second tool list and no discovery state — it
//! holds the *rules* both interfaces apply to what the host discovered, so two
//! interfaces cannot disagree about whether a call is answerable.
//!
//! Three of those rules are structural rather than conventional:
//!
//! - Identity is namespaced. A plugin, a capability, a grant and an extension
//!   attribute are all namespaced strings (`vendor.example/render`), so a
//!   third party can publish one without waiting for a new enum to be
//!   regenerated, and two vendors cannot collide on a bare word.
//! - An unsupported **required** capability refuses only that call or that
//!   extension. An unknown **optional** attribute is preserved verbatim in a
//!   bucket the host never rules on, so no code can act on an attribute it does
//!   not understand.
//! - The authority fields — `principal`, `effectId`, `authorized`, `stateRoot` —
//!   cannot be supplied by a free-form attribute. They are refused by name, and
//!   the real ones are carried by typed values (see [`crate::AuthorityHandle`])
//!   that free-form input never reaches.
//!
//! Version negotiation is a version decision and nothing else: it can refuse an
//! extension, and it can never grant authority, identity or permission.
//! [`ContractCompatibility`] carries no identity at all, so no negotiation
//! result can be mistaken for one.

use crate::failure::{ApplicationFailure, RecoveryAction};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// The longest namespaced name accepted, so a name this crate admits cannot be
/// rejected later by a host that applies the same bound.
pub const MAX_NAMESPACED_NAME_BYTES: usize = 160;
/// The longest implementation version accepted.
pub const MAX_IMPLEMENTATION_VERSION_BYTES: usize = 64;

/// The authority fields free-form extension attributes may never supply.
///
/// These belong to the core's own records: who the effect is for, which effect
/// it is, whether it is authorized, and which state root it is bound to. An
/// extension that could set them through a plain attribute would be writing the
/// host's authority rather than describing itself.
pub const AUTHORITY_FIELDS: [&str; 4] = ["principal", "effectId", "authorized", "stateRoot"];

/// Whether `name` is a namespaced extension name, such as `vendor.example/render`
/// or `licoup.quota.tokens`.
///
/// The rule is the one the extension schemas already publish for namespaced
/// keys: two lowercase segments separated by `.` or `-`, optionally followed
/// by `.`/`-`/`/`-separated leaves that may keep uppercase letters. A bare word is not
/// namespaced, which is what keeps a vendor from claiming a name the product
/// itself publishes.
pub fn is_namespaced(name: &str) -> bool {
    if name.is_empty() || name.len() > MAX_NAMESPACED_NAME_BYTES {
        return false;
    }
    let bytes = name.as_bytes();
    let mut cursor = 0;
    if !take_lowercase_word(bytes, &mut cursor) {
        return false;
    }
    if cursor == bytes.len() || !matches!(bytes[cursor], b'.' | b'-') {
        return false;
    }
    cursor += 1;
    if !take_lowercase_word(bytes, &mut cursor) {
        return false;
    }
    while cursor < bytes.len() && matches!(bytes[cursor], b'.' | b'-' | b'/') {
        cursor += 1;
        if !take_while(bytes, &mut cursor, |byte| {
            byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-'
        }) {
            return false;
        }
    }
    cursor == bytes.len()
}

/// Whether `name` is one of the authority fields, in any of the spellings an
/// extension might offer it in.
///
/// Comparison ignores case and `_`, so `effectId`, `effect_id` and `EFFECTID`
/// are all refused: a spelling difference is not a different field.
pub fn is_authority_field(name: &str) -> bool {
    let normalized: String = name
        .chars()
        .filter(|character| *character != '_')
        .flat_map(char::to_lowercase)
        .collect();
    AUTHORITY_FIELDS
        .iter()
        .any(|field| field.eq_ignore_ascii_case(&normalized))
}

/// Whether `version` is a published implementation version, such as `1.4.0` or
/// `2.0.0-rc.1`.
pub fn is_semver(version: &str) -> bool {
    if version.is_empty() || version.len() > MAX_IMPLEMENTATION_VERSION_BYTES {
        return false;
    }
    let (core, prerelease) = match version.split_once('-') {
        Some((core, prerelease)) => (core, Some(prerelease)),
        None => (version, None),
    };
    let mut parts = core.split('.');
    let published = matches!(
        (parts.next(), parts.next(), parts.next(), parts.next()),
        (Some(major), Some(minor), Some(patch), None)
            if is_numeric_part(major) && is_numeric_part(minor) && is_numeric_part(patch)
    );
    published
        && prerelease.is_none_or(|prerelease| {
            !prerelease.is_empty()
                && prerelease
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
        })
}

/// How strongly a caller or an extension requires one namespaced attribute.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Requirement {
    /// Keep it if the host understands it; preserve or drop it if it does not.
    Optional,
    /// The host must understand it, or this call is refused.
    Required,
}

/// One namespaced attribute as it was declared, with how strongly it is
/// required.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeclaredAttribute {
    pub name: String,
    pub requirement: Requirement,
    /// The attribute's own value. A pure capability declaration carries
    /// [`Value::Null`].
    #[serde(default)]
    pub value: Value,
}

impl DeclaredAttribute {
    /// An attribute whose absence from the host is not fatal to the call.
    pub fn optional(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            requirement: Requirement::Optional,
            value: Value::Null,
        }
    }

    /// An attribute the host must understand for the call to proceed.
    pub fn required(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            requirement: Requirement::Required,
            value: Value::Null,
        }
    }

    pub fn with_value(mut self, value: Value) -> Self {
        self.value = value;
        self
    }
}

/// The namespaced attributes a host adopted for one admission.
///
/// The two buckets are the whole point of the split: `bound` is what the host
/// understood and may act on, and `preserved` is what it did not — kept so the
/// caller can be told what happened to it, and never consulted as though it had
/// been understood.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AdoptedAttributes {
    bound: BTreeMap<String, Value>,
    preserved: BTreeMap<String, Value>,
}

impl AdoptedAttributes {
    /// The attributes the host understood.
    pub fn bound(&self) -> &BTreeMap<String, Value> {
        &self.bound
    }

    /// The unknown optional attributes, kept verbatim.
    pub fn preserved(&self) -> &BTreeMap<String, Value> {
        &self.preserved
    }

    /// The value the host bound for one attribute, if it understood it.
    pub fn bound_value(&self, name: &str) -> Option<&Value> {
        self.bound.get(name)
    }

    /// Whether an attribute was kept without being understood.
    pub fn was_preserved(&self, name: &str) -> bool {
        self.preserved.contains_key(name)
    }

    pub fn is_empty(&self) -> bool {
        self.bound.is_empty() && self.preserved.is_empty()
    }
}

/// What the native catalog discovered and this host can actually serve.
///
/// This is a set the host fills from its own discovery, not a catalog: nothing
/// here reads, stores or publishes a tool list, so there is no second copy of
/// the registry to disagree with the owner's.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DiscoveredCapabilities {
    names: BTreeSet<String>,
}

impl DiscoveredCapabilities {
    pub fn new<S: Into<String>>(names: impl IntoIterator<Item = S>) -> Self {
        Self {
            names: names.into_iter().map(Into::into).collect(),
        }
    }

    /// Whether the host discovered this exact namespaced capability.
    pub fn supports(&self, capability: &str) -> bool {
        self.names.contains(capability)
    }

    /// Whether the host discovered nothing at all.
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    /// Adopt the namespaced attributes one caller or one extension declared.
    ///
    /// A required attribute this catalog cannot serve refuses *this* call (or
    /// this extension) locally; every other call keeps working, which is why the
    /// refusal is here rather than at the catalog. An optional attribute whose
    /// namespace the host does not understand is preserved instead.
    ///
    /// A name that is not namespaced at all, a name that borrows an authority
    /// field's, and the same name declared twice are all refused: none of them
    /// describes an extension attribute, and admitting one would create a second,
    /// competing namespace next to the authority the core already holds.
    pub fn admit(
        &self,
        declared: &[DeclaredAttribute],
    ) -> Result<AdoptedAttributes, ApplicationFailure> {
        let mut seen = BTreeSet::new();
        let mut adopted = AdoptedAttributes::default();
        for attribute in declared {
            if is_authority_field(&attribute.name) {
                return Err(refusal(
                    "capability_authority_field_reserved",
                    "capability/admit",
                    &attribute.name,
                ));
            }
            if !is_namespaced(&attribute.name) {
                return Err(refusal(
                    "capability_attribute_invalid",
                    "capability/admit",
                    &attribute.name,
                ));
            }
            if !seen.insert(attribute.name.as_str()) {
                return Err(refusal(
                    "capability_attribute_duplicate",
                    "capability/admit",
                    &attribute.name,
                ));
            }
            if self.supports(&attribute.name) {
                adopted
                    .bound
                    .insert(attribute.name.clone(), attribute.value.clone());
            } else if attribute.requirement == Requirement::Required {
                // The refusal is actionable — install or enable what serves this
                // capability — rather than "your request is malformed".
                return Err(refusal(
                    "capability_required_unsupported",
                    "capability/admit",
                    &attribute.name,
                )
                .with_recovery(RecoveryAction::InstallOrRetryRuntime));
            } else {
                adopted
                    .preserved
                    .insert(attribute.name.clone(), attribute.value.clone());
            }
        }
        Ok(adopted)
    }
}

/// The contract version range one side supports, in the shape the extension
/// schemas already publish (`major` plus the oldest minor it needs).
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContractRange {
    pub major: u32,
    pub minimum_minor: u32,
}

/// The outcome of negotiating two contract ranges.
///
/// It carries no identity, no principal and no grant, so a negotiation can never
/// be read as one: the only thing it can do is refuse the extension whose
/// contract does not fit, and it does so locally.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContractCompatibility {
    /// Same major, and the host provides at least the minor this side needs.
    Compatible,
    /// Same major, but this side needs features from a newer minor.
    RequiresNewerMinor,
    /// A different wire major. Only this extension is refused.
    MajorMismatch,
}

impl ContractRange {
    /// Negotiate this side's supported range against the host's.
    ///
    /// Within one major, additive minor fields on either side are ignored rather
    /// than fatal, so only a *required* newer minor is a refusal. A different
    /// major refuses this extension and nothing else.
    pub const fn negotiate(self, host: Self) -> ContractCompatibility {
        if self.major != host.major {
            ContractCompatibility::MajorMismatch
        } else if host.minimum_minor < self.minimum_minor {
            ContractCompatibility::RequiresNewerMinor
        } else {
            ContractCompatibility::Compatible
        }
    }
}

impl ContractCompatibility {
    pub const fn is_compatible(self) -> bool {
        matches!(self, Self::Compatible)
    }

    /// The local refusal for an extension this host cannot use as it stands, or
    /// `None` when it can.
    ///
    /// The refusal is actionable — install or enable a version whose contract
    /// fits — instead of being reported as a malformed request.
    pub fn refusal(self, plugin_id: &str) -> Option<ApplicationFailure> {
        let code = match self {
            Self::Compatible => return None,
            Self::RequiresNewerMinor => "extension_contract_minor_unavailable",
            Self::MajorMismatch => "extension_contract_major_mismatch",
        };
        Some(
            ApplicationFailure::permanent(code, "extension/negotiate")
                .with_component("extension_catalog")
                .with_field("supportedContractRange")
                .with_presentation_arg("pluginId", plugin_id)
                .with_recovery(RecoveryAction::InstallOrRetryRuntime),
        )
    }
}

/// How the host may activate an extension, in the manifest's own vocabulary.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ActivationMode {
    /// Start it when a call needs it.
    OnDemand,
    /// Start it only when the user asks. The conservative default: nothing is
    /// activated because a descriptor omitted the field.
    #[default]
    Explicit,
}

/// The quota dimensions one capability draws on.
///
/// Only the dimensions are named here. The amounts, units and cadence are
/// observations of a quota source and belong to the usage contract, so a host
/// that has no quota source reports `unknown` there rather than reading "no
/// dimensions declared" as "unlimited".
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaShape {
    #[serde(default)]
    pub dimensions: Vec<String>,
}

/// Which lifecycle operations an extension supports.
///
/// The operations are namespaced strings rather than an enum, so an extension
/// may support one this crate has never heard of and the host can still report
/// it faithfully without regenerating a list here. The concrete lifecycle method
/// catalog belongs to the extension SDK contract, not to this crate.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LifecycleSupport {
    #[serde(default)]
    pub activation: ActivationMode,
    #[serde(default)]
    pub operations: Vec<String>,
}

/// What one extension says about itself, and what it needs from the host.
///
/// A descriptor is a claim, not a grant: `required_grants` asks, and the host's
/// permission decision is made elsewhere and is not data in here. Nothing in a
/// descriptor can set an authority field — there is no field for one, and a
/// free-form attribute carrying that name is refused by
/// [`DiscoveredCapabilities::admit`].
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityDescriptor {
    /// The namespaced extension identity, such as `vendor.example.render`.
    pub plugin_id: String,
    /// The implementation's own published version.
    pub implementation_version: String,
    pub supported_contract_range: ContractRange,
    /// The namespaced capabilities this extension offers.
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// The namespaced grants it needs from the host.
    #[serde(default)]
    pub required_grants: Vec<String>,
    #[serde(default)]
    pub quota_shape: QuotaShape,
    #[serde(default)]
    pub lifecycle: LifecycleSupport,
    /// The extension's own namespaced attributes, each optional or required.
    #[serde(default)]
    pub attributes: Vec<DeclaredAttribute>,
}

impl CapabilityDescriptor {
    /// Structural validation: identity is namespaced, the version is a version,
    /// and no extension attribute borrows an authority field's name.
    ///
    /// Validation is structural on purpose. Whether the grants are actually
    /// granted, whether the plugin is installed, and whether it may run are
    /// questions only the host can answer.
    pub fn validate(&self) -> Result<(), ApplicationFailure> {
        if !is_namespaced(&self.plugin_id) {
            return Err(descriptor_refused("pluginId"));
        }
        if !is_semver(&self.implementation_version) {
            return Err(descriptor_refused("implementationVersion"));
        }
        for capability in &self.capabilities {
            if !is_namespaced(capability) {
                return Err(descriptor_refused("capabilities"));
            }
        }
        for grant in &self.required_grants {
            if !is_namespaced(grant) {
                return Err(descriptor_refused("requiredGrants"));
            }
        }
        for dimension in &self.quota_shape.dimensions {
            if !is_namespaced(dimension) {
                return Err(descriptor_refused("quotaShape.dimensions"));
            }
        }
        for operation in &self.lifecycle.operations {
            if !is_namespaced(operation) {
                return Err(descriptor_refused("lifecycle.operations"));
            }
        }
        let mut names = BTreeSet::new();
        for attribute in &self.attributes {
            if is_authority_field(&attribute.name) || !is_namespaced(&attribute.name) {
                return Err(descriptor_refused("attributes"));
            }
            if !names.insert(attribute.name.as_str()) {
                return Err(descriptor_refused("attributes"));
            }
        }
        Ok(())
    }

    /// Negotiate this extension's contract against the host's.
    pub const fn compatible_with(&self, host: ContractRange) -> ContractCompatibility {
        self.supported_contract_range.negotiate(host)
    }

    /// Adopt this extension's own attributes against what the host discovered.
    ///
    /// A required attribute the host cannot serve refuses this extension alone.
    pub fn admit(
        &self,
        discovered: &DiscoveredCapabilities,
    ) -> Result<AdoptedAttributes, ApplicationFailure> {
        discovered.admit(&self.attributes)
    }
}

/// The local refusal for one namespaced attribute, naming the attribute as the
/// offending field so a caller can act on it.
fn refusal(code: &str, stage: &str, name: &str) -> ApplicationFailure {
    let failure = ApplicationFailure::permanent(code, stage)
        .with_component("extension_catalog")
        .with_field("attributes");
    // An invalid name is arbitrary input, possibly a path or technical text.
    // Only an admitted public identifier may enter the failure projection.
    if is_namespaced(name) {
        failure
            .with_field(name)
            .with_presentation_arg("attribute", name)
    } else {
        failure
    }
}

/// The refusal for a descriptor whose own shape is wrong.
fn descriptor_refused(field: &str) -> ApplicationFailure {
    ApplicationFailure::permanent("extension_descriptor_invalid", "extension/validate")
        .with_component("extension_catalog")
        .with_field(field)
}

fn is_numeric_part(part: &str) -> bool {
    !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit())
}

fn take_while(bytes: &[u8], cursor: &mut usize, predicate: impl Fn(u8) -> bool) -> bool {
    let start = *cursor;
    while *cursor < bytes.len() && predicate(bytes[*cursor]) {
        *cursor += 1;
    }
    *cursor > start
}

fn take_lowercase_word(bytes: &[u8], cursor: &mut usize) -> bool {
    take_while(bytes, cursor, |byte| {
        byte.is_ascii_lowercase() || byte.is_ascii_digit()
    })
}
