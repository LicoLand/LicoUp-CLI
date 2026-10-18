use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::sync::OnceLock;

use super::taxonomy::{CapabilityScope, SecurityCapability};

pub(super) const CAPABILITY_CATALOG_JSON: &str =
    include_str!("../../../resources/secure-mesh-capability-catalog.json");
const CAPABILITY_CATALOG_SCHEMA_VERSION: u32 = 1;
const MAX_CAPABILITY_CATALOG_BYTES: usize = 256 * 1024;

/// Typed failure for canonical secure mesh capability catalog evaluation.
///
/// The embedded catalog is a host-facing invariant: every validation failure
/// is classified once (`from_json` and `require_complete`), stored in the
/// bounded `OnceLock` as this structured error instead of a stringly-typed
/// error, and only ever projected outward as a problem code.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum CapabilityCatalogError {
    #[error("secure mesh capability catalog exceeds its bounded size")]
    ExceedsBoundedSize,
    #[error("secure mesh capability catalog schema is invalid")]
    InvalidSchema,
    #[error("secure mesh capability catalog version is unsupported")]
    UnsupportedVersion,
    #[error("secure mesh capability catalog size is invalid")]
    InvalidSize,
    #[error("secure mesh capability catalog contains an unknown capability")]
    UnknownCapability,
    #[error("secure mesh capability catalog contains a duplicate identifier")]
    DuplicateIdentifier,
    #[error("secure mesh capability prerequisite set exceeds its bounded size")]
    PrerequisiteSetLimit,
    #[error("secure mesh capability cannot depend on itself")]
    SelfDependency,
    #[error("secure mesh capability contains a duplicate prerequisite")]
    DuplicatePrerequisite,
    #[error("secure mesh capability prerequisite is missing from the catalog")]
    MissingPrerequisite,
    #[error("only protocol capabilities may be mandatory")]
    InvalidMandatoryScope,
    #[error("secure mesh capability catalog contains a dependency cycle")]
    DependencyCycle,
    #[error("canonical secure mesh capability catalog is incomplete")]
    Incomplete,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityDefinition {
    pub capability: SecurityCapability,
    pub scope: CapabilityScope,
    pub mandatory: bool,
    pub derived: bool,
    pub prerequisites: Vec<SecurityCapability>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawCapabilityCatalog {
    schema_version: u32,
    capabilities: Vec<RawCapabilityDefinition>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCapabilityDefinition {
    id: String,
    scope: CapabilityScope,
    mandatory: bool,
    derived: bool,
    prerequisites: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct CapabilityCatalog {
    schema_version: u32,
    digest: String,
    definitions: Vec<Option<CapabilityDefinition>>,
    topological_order: Vec<SecurityCapability>,
    edge_count: usize,
}

impl CapabilityCatalog {
    pub fn from_json(source: &str) -> std::result::Result<Self, CapabilityCatalogError> {
        if source.len() > MAX_CAPABILITY_CATALOG_BYTES {
            return Err(CapabilityCatalogError::ExceedsBoundedSize);
        }
        let raw: RawCapabilityCatalog =
            serde_json::from_str(source).map_err(|_| CapabilityCatalogError::InvalidSchema)?;
        if raw.schema_version != CAPABILITY_CATALOG_SCHEMA_VERSION {
            return Err(CapabilityCatalogError::UnsupportedVersion);
        }
        if raw.capabilities.is_empty() || raw.capabilities.len() > SecurityCapability::COUNT {
            return Err(CapabilityCatalogError::InvalidSize);
        }

        let mut definitions = vec![None; SecurityCapability::COUNT];
        for raw_definition in raw.capabilities {
            let capability = SecurityCapability::from_id(&raw_definition.id)
                .map_err(|_| CapabilityCatalogError::UnknownCapability)?;
            if definitions[capability.index()].is_some() {
                return Err(CapabilityCatalogError::DuplicateIdentifier);
            }
            if raw_definition.prerequisites.len() > SecurityCapability::COUNT {
                return Err(CapabilityCatalogError::PrerequisiteSetLimit);
            }
            let prerequisites = raw_definition
                .prerequisites
                .iter()
                .map(|id| {
                    SecurityCapability::from_id(id)
                        .map_err(|_| CapabilityCatalogError::UnknownCapability)
                })
                .collect::<std::result::Result<Vec<_>, _>>()?;
            if prerequisites.contains(&capability) {
                return Err(CapabilityCatalogError::SelfDependency);
            }
            let unique_prerequisites = prerequisites.iter().copied().collect::<BTreeSet<_>>();
            if unique_prerequisites.len() != prerequisites.len() {
                return Err(CapabilityCatalogError::DuplicatePrerequisite);
            }
            definitions[capability.index()] = Some(CapabilityDefinition {
                capability,
                scope: raw_definition.scope,
                mandatory: raw_definition.mandatory,
                derived: raw_definition.derived,
                prerequisites,
            });
        }

        for definition in definitions.iter().flatten() {
            for prerequisite in &definition.prerequisites {
                if definitions[prerequisite.index()].is_none() {
                    return Err(CapabilityCatalogError::MissingPrerequisite);
                }
            }
            if definition.mandatory && definition.scope != CapabilityScope::ProtocolSession {
                return Err(CapabilityCatalogError::InvalidMandatoryScope);
            }
        }

        let (topological_order, edge_count) = validated_topological_order(&definitions)?;
        Ok(Self {
            schema_version: raw.schema_version,
            digest: sha256_hex(source.as_bytes()),
            definitions,
            topological_order,
            edge_count,
        })
    }

    fn require_complete(&self) -> std::result::Result<(), CapabilityCatalogError> {
        if self.definitions.iter().all(Option::is_some) {
            Ok(())
        } else {
            Err(CapabilityCatalogError::Incomplete)
        }
    }

    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }

    pub fn digest(&self) -> &str {
        &self.digest
    }

    pub fn definition(&self, capability: SecurityCapability) -> Option<&CapabilityDefinition> {
        self.definitions[capability.index()].as_ref()
    }

    pub fn definitions(&self) -> impl Iterator<Item = &CapabilityDefinition> {
        self.topological_order
            .iter()
            .filter_map(|capability| self.definition(*capability))
    }

    pub fn topological_order(&self) -> &[SecurityCapability] {
        &self.topological_order
    }

    pub fn edge_count(&self) -> usize {
        self.edge_count
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut encoded, "{byte:02x}").expect("writing to a string must succeed");
    }
    encoded
}

fn validated_topological_order(
    definitions: &[Option<CapabilityDefinition>],
) -> std::result::Result<(Vec<SecurityCapability>, usize), CapabilityCatalogError> {
    let mut indegree = [0usize; SecurityCapability::COUNT];
    let mut dependents = vec![Vec::<SecurityCapability>::new(); SecurityCapability::COUNT];
    let mut defined_count = 0usize;
    let mut edge_count = 0usize;
    for definition in definitions.iter().flatten() {
        defined_count = defined_count.saturating_add(1);
        indegree[definition.capability.index()] = definition.prerequisites.len();
        edge_count = edge_count.saturating_add(definition.prerequisites.len());
        for prerequisite in &definition.prerequisites {
            dependents[prerequisite.index()].push(definition.capability);
        }
    }
    for entries in &mut dependents {
        entries.sort_unstable();
    }

    let mut roots = definitions
        .iter()
        .flatten()
        .filter(|definition| indegree[definition.capability.index()] == 0)
        .map(|definition| definition.capability)
        .collect::<BTreeSet<_>>();
    let mut order = Vec::with_capacity(defined_count);
    while let Some(capability) = roots.pop_first() {
        order.push(capability);
        for dependent in &dependents[capability.index()] {
            indegree[dependent.index()] = indegree[dependent.index()].saturating_sub(1);
            if indegree[dependent.index()] == 0 {
                roots.insert(*dependent);
            }
        }
    }
    if order.len() != defined_count {
        return Err(CapabilityCatalogError::DependencyCycle);
    }
    Ok((order, edge_count))
}

static EMBEDDED_CAPABILITY_CATALOG: OnceLock<
    std::result::Result<CapabilityCatalog, CapabilityCatalogError>,
> = OnceLock::new();

pub fn capability_catalog() -> Result<&'static CapabilityCatalog> {
    EMBEDDED_CAPABILITY_CATALOG
        .get_or_init(|| {
            CapabilityCatalog::from_json(CAPABILITY_CATALOG_JSON).and_then(|catalog| {
                catalog.require_complete()?;
                Ok(catalog)
            })
        })
        .as_ref()
        .map_err(|error| anyhow!(error.to_string()))
}
