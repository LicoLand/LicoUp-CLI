//! The resource mount plan: what an installed appearance package renders as,
//! published for the client interface to mount.
//!
//! The plan is a *document*, not a program. It carries what every resource kind
//! serves, the contributions the selected generations declare, and the plain
//! values those contributions render. It has no field for a widget, a builder,
//! a callback, a script or a client object, so nothing a package installs can
//! cross into the interface as code.
//!
//! Two owners meet here and neither absorbs the other:
//!
//! * [`ResourceHost`](super::resources::ResourceHost) decides what each kind
//!   serves, which package generation serves it, and when a kind falls back to
//!   the client's own system appearance.
//! * [`plan_mount_with_resource_formats`](licoup_extension_contracts::ui::plan_mount_with_resource_formats)
//!   decides which interface contributions this host may mount at all, and
//!   names the reason for every one it refuses.
//!
//! The document is the whole answer. A renderer mounts one committed revision:
//! it refuses a contribution naming a primitive or action this host did not
//! register, and renders the declared default while a kind is falling back.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use licoup_application::ApplicationFailure;
use licoup_extension_contracts::manifest::{CompositionComponent, ResourceDeclaration};
use licoup_extension_contracts::profile::ExtensionProfile;
use licoup_extension_contracts::ui::{
    Contribution, ContributionKind, HostPrimitive, plan_mount_with_resource_formats,
};
use serde_json::{Value, json};

use licoup_extension_contracts::manifest::ResourceKind;

use super::resources::{ResourceBinding, ResourceBindings, SystemDefault};
use super::{read_bounded_text, refusal, replace_file_atomically};

/// The document format this host publishes.
pub const MOUNT_PLAN_FORMAT: &str = "licoup.client.mount-plan.v1";

/// The document version this host publishes.
pub const MOUNT_PLAN_VERSION: u64 = 1;

/// Where the published document lives inside one data root.
///
/// The client reads exactly this path below the same data directory, so the
/// writer and the reader cannot drift into two locations. It is the client
/// state root the reader resolves, not a package-store path.
pub const MOUNT_PLAN_RECORD: &str = "client-state/presentation/mount-plan.json";

/// The largest published document this host reads back.
pub const MAX_MOUNT_PLAN_BYTES: usize = 1024 * 1024;

/// The stage a publication or a read-back refusal reports.
const MOUNT_PLAN_STAGE: &str = "extension/mount-plan";

/// The resource-view format a component renders, when it declares one.
///
/// A composition component names a compiled primitive, not a format: the
/// bounded pure-data format belongs to a resource-view contribution. The
/// planner therefore reads no format here, and a component is never refused for
/// one.
const fn component_resource_format(_component: &CompositionComponent) -> Option<String> {
    None
}

/// The profile ids this client serves.
///
/// A contribution that needs a profile outside this set is preserved and not
/// mounted, and the decision names why. The list is explicit rather than
/// derived: a client that serves a profile states it.
pub const SERVED_PROFILES: [&str; 0] = [];

/// The action names this client registered a handler for.
///
/// A contribution may invoke only an action named here. A name outside this set
/// is refused, so a package can never reach an operation the host did not
/// publish. The constant is empty because this client registers no composition
/// action yet; the shell names its own set at composition instead.
pub const HOST_ACTIONS: [&str; 0] = [];

/// The resource-view formats this interface compiles a renderer for.
///
/// A resource view naming a format outside this set is refused locally and
/// preserved, never drawn as another format.
pub const HOST_RESOURCE_VIEW_FORMATS: [&str; 0] = [];

/// One contribution the plan publishes.
///
/// The values are data: an identity, a primitive name, the references the host
/// already resolved, and plain values the primitive renders.
#[derive(Clone, Debug, PartialEq)]
pub struct PlannedContribution {
    /// The namespaced component identity.
    pub id: String,
    /// The compiled host primitive this component binds.
    pub primitive: HostPrimitive,
    /// The resource this component reads, when its declaration named one.
    pub resource_id: Option<String>,
    /// The bounded pure-data format a resource view renders.
    pub resource_format: Option<String>,
    /// The host-registered action this component may invoke.
    pub action_ref: Option<String>,
    /// Region ids the component occupies, in declaration order.
    pub regions: Vec<String>,
    /// Plain values the primitive renders; never code, never a widget.
    pub inputs: BTreeMap<String, Value>,
}

impl PlannedContribution {
    /// The contribution's wire form.
    pub fn to_wire(&self) -> Value {
        let mut value = serde_json::Map::new();
        value.insert("id".to_owned(), json!(self.id));
        value.insert("primitive".to_owned(), json!(self.primitive.as_str()));
        if let Some(resource_id) = &self.resource_id {
            value.insert("resourceId".to_owned(), json!(resource_id));
        }
        if let Some(resource_format) = &self.resource_format {
            value.insert("resourceFormat".to_owned(), json!(resource_format));
        }
        if let Some(action_ref) = &self.action_ref {
            value.insert("actionRef".to_owned(), json!(action_ref));
        }
        if !self.regions.is_empty() {
            value.insert("regions".to_owned(), json!(self.regions));
        }
        if !self.inputs.is_empty() {
            value.insert(
                "inputs".to_owned(),
                Value::Object(self.inputs.clone().into_iter().collect()),
            );
        }
        Value::Object(value)
    }
}

/// One decision about one contribution.
#[derive(Clone, Debug, PartialEq)]
pub struct MountDecision {
    /// The contribution this decision is about.
    pub contribution: PlannedContribution,
    /// `None` when it mounts; otherwise the host's stable reason.
    pub blocked: Option<&'static str>,
}

impl MountDecision {
    pub fn is_mounted(&self) -> bool {
        self.blocked.is_none()
    }
}

/// One published mount plan.
#[derive(Clone, Debug, PartialEq)]
pub struct ResourceMountPlan {
    /// The binding revision this plan was published from.
    pub revision: u64,
    /// The committed bindings this plan publishes.
    pub bindings: ResourceBindings,
    /// The action names the shell that will render this plan registered.
    pub host_actions: Vec<String>,
    /// Every contribution the host considered, mounted and refused alike.
    pub decisions: Vec<MountDecision>,
}

impl ResourceMountPlan {
    /// The mounted contributions, in publication order.
    pub fn mounted(&self) -> impl Iterator<Item = &PlannedContribution> {
        self.decisions
            .iter()
            .filter(|decision| decision.is_mounted())
            .map(|decision| &decision.contribution)
    }

    /// The refused contributions with their stable reasons.
    pub fn refused(&self) -> impl Iterator<Item = (&PlannedContribution, &'static str)> {
        self.decisions.iter().filter_map(|decision| {
            decision
                .blocked
                .map(|reason| (&decision.contribution, reason))
        })
    }

    /// The wire document this plan publishes.
    ///
    /// Member order is fixed and collections are ordered, so the same published
    /// bindings always encode to the same document.
    pub fn to_document(&self) -> Value {
        // Only the mounted contributions are published: the interface cannot
        // mount what the native owner refused, so a refused contribution has no
        // member in the document at all.
        let mut contributions: Vec<Value> = self
            .decisions
            .iter()
            .filter(|decision| decision.is_mounted())
            .map(decisions)
            .collect();
        contributions.sort_by(|left, right| {
            left["id"]
                .as_str()
                .unwrap_or_default()
                .cmp(right["id"].as_str().unwrap_or_default())
        });

        let primitives: Vec<&str> = HostPrimitive::ALL
            .iter()
            .map(|primitive| primitive.as_str())
            .collect();
        let mut actions: Vec<&str> = self.host_actions.iter().map(String::as_str).collect();
        actions.sort_unstable();

        json!({
            "format": MOUNT_PLAN_FORMAT,
            "version": MOUNT_PLAN_VERSION,
            "revision": self.revision,
            "servedProfiles": SERVED_PROFILES,
            "hostPrimitives": primitives,
            "hostActions": actions,
            "bindings": wire_bindings(&self.bindings),
            "fallbacks": wire_fallbacks(&self.bindings),
            "contributions": contributions,
        })
    }

    /// The contributions this plan decided shall mount, as the interface reads
    /// them. A refused contribution is not published: the interface cannot
    /// mount what the native owner refused, whatever a renderer does.
    pub fn mounted_document(&self) -> Value {
        let contributions: Vec<Value> = self.mounted().map(PlannedContribution::to_wire).collect();
        json!({ "contributions": contributions })
    }
}

/// One mounted contribution's document member.
fn decisions(decision: &MountDecision) -> Value {
    decision.contribution.to_wire()
}

/// The declared default one kind falls back to, as the document names it.
///
/// The interface reader publishes the same three names, so a value that leaves
/// this host is the value a renderer compares against.
pub const fn system_default_name(system: SystemDefault) -> &'static str {
    match system {
        SystemDefault::Appearance => "appearance",
        SystemDefault::Font => "font",
        SystemDefault::Locale => "locale",
    }
}

/// The bindings a published snapshot serves, one member per kind.
fn wire_bindings(bindings: &ResourceBindings) -> Vec<Value> {
    ResourceKind::ALL
        .iter()
        .map(|kind| match bindings.binding(*kind) {
            ResourceBinding::Selected {
                resource_id,
                package_id,
                package_generation,
                ..
            } => json!({
                "kind": kind.as_str(),
                "resourceId": resource_id,
                "packageId": package_id,
                "packageGeneration": package_generation,
            }),
            ResourceBinding::Default { system } => json!({
                "kind": kind.as_str(),
                "system": system_default_name(*system),
            }),
        })
        .collect()
}

/// The fallbacks a published snapshot recorded, one member each.
fn wire_fallbacks(bindings: &ResourceBindings) -> Vec<Value> {
    bindings
        .fallbacks()
        .map(|fallback| {
            json!({
                "kind": fallback.kind.as_str(),
                "resourceId": fallback.resource_id,
                "packageId": fallback.package_id,
                "reason": fallback.reason.as_str(),
            })
        })
        .collect()
}

/// Publish the mount plan for one package generation.
///
/// Every composition the generation carries becomes one planned contribution
/// per component, decided through the contract's own planner. A generation this
/// host no longer serves contributes nothing: the binding switch already
/// published the whole set, and a late plan must not resurrect the generation
/// before it.
pub fn plan_generation_mount(
    bindings: &ResourceBindings,
    package_id: &str,
    package_generation: u64,
    resources: &[ResourceDeclaration],
) -> ResourceMountPlan {
    plan_generation_mount_with_actions(
        bindings,
        package_id,
        package_generation,
        resources,
        &HOST_ACTIONS,
    )
}

/// Publish the mount plan for a shell that registered its own action names.
///
/// The registered set is an argument rather than a global so a shell states
/// what it actually registered, and a plan is always decided against the shell
/// that will render it.
pub fn plan_generation_mount_with_actions(
    bindings: &ResourceBindings,
    package_id: &str,
    package_generation: u64,
    resources: &[ResourceDeclaration],
    host_actions: &[&str],
) -> ResourceMountPlan {
    let served = bindings.serves_package_generation(package_id, package_generation);
    let profiles: Vec<ExtensionProfile> = SERVED_PROFILES
        .iter()
        .filter_map(|id| ExtensionProfile::from_id(id))
        .collect();

    let mut decisions: Vec<MountDecision> = Vec::new();
    for resource in resources {
        let ResourceDeclaration::Composition { components, .. } = resource else {
            continue;
        };
        for component in components {
            decisions.push(decide(
                component,
                served,
                &profiles,
                HOST_RESOURCE_VIEW_FORMATS,
                host_actions,
            ));
        }
    }

    ResourceMountPlan {
        revision: bindings.revision(),
        bindings: bindings.clone(),
        host_actions: host_actions
            .iter()
            .map(|action| (*action).to_owned())
            .collect(),
        decisions,
    }
}

/// Decide one component through the contract's planner, then against this
/// host's own registration.
///
/// The two refusals stay distinguishable: a declaration the contract already
/// rejects is `contribution_invalid`, while a primitive this host build did not
/// compile is refused here rather than rendered as a neighbouring primitive.
fn decide(
    component: &CompositionComponent,
    generation_served: bool,
    profiles: &[ExtensionProfile],
    available_formats: [&'static str; 0],
    host_actions: &[&str],
) -> MountDecision {
    let contribution = contribution(component);
    let planned = planned(component);
    if !generation_served {
        return MountDecision {
            contribution: planned,
            blocked: Some("generation_not_served"),
        };
    }
    if let Some(refusal) = components_registration(component, host_actions) {
        return MountDecision {
            contribution: planned,
            blocked: Some(refusal),
        };
    }
    let mut formats: Vec<&str> = available_formats.to_vec();
    formats.sort_unstable();
    let candidates = [contribution];
    let mounts = plan_mount_with_resource_formats(&candidates, profiles, &formats);
    match mounts.first().and_then(|mount| mount.blocked) {
        Some(blocked) => MountDecision {
            contribution: planned,
            blocked: Some(blocked),
        },
        None => MountDecision {
            contribution: planned,
            blocked: None,
        },
    }
}

/// Why this host cannot mount a component whose declaration is well formed.
///
/// The action a component binds is a name the shell registered. A name outside
/// that set is refused here, so a package can never reach an operation the host
/// did not publish.
fn components_registration(
    component: &CompositionComponent,
    host_actions: &[&str],
) -> Option<&'static str> {
    if let Some(action_ref) = component.action_ref.as_deref() {
        if !host_actions.contains(&action_ref) {
            return Some("action_unregistered");
        }
    }
    None
}

/// One component as the contract's planner reads it.
///
/// A composition component binds a compiled primitive and, when it declares
/// one, a host-registered action; it never brings a primitive of its own and it
/// never names a callback. The component's own namespaced identity is the
/// contribution identity, so the planner's structural rules apply to it
/// unchanged and a refusal names the same thing the package declared.
fn contribution(component: &CompositionComponent) -> Contribution {
    Contribution {
        schema: licoup_extension_contracts::wire::UI.to_owned(),
        id: component.component.clone(),
        kind: contribution_kind(component.primitive),
        title: component.component.clone(),
        required_profile: None,
        resource_ref: None,
        action_ref: component.action_ref.clone(),
        resource_format: component_resource_format(component),
        fields: Vec::new(),
        series: Vec::new(),
    }
}

/// The published contribution kind one compiled primitive composes.
///
/// The kind decides which structural rules apply, so a primitive with no
/// declared shape of its own is read as the closest published one rather than
/// as a shape the contract does not have.
const fn contribution_kind(primitive: HostPrimitive) -> ContributionKind {
    match primitive {
        HostPrimitive::Form => ContributionKind::Settings,
        HostPrimitive::Table => ContributionKind::ResourceView,
        HostPrimitive::Chart => ContributionKind::MetricPanel,
        HostPrimitive::Progress => ContributionKind::ResourceView,
        HostPrimitive::Text => ContributionKind::Navigation,
        HostPrimitive::Action => ContributionKind::Command,
    }
}

/// The planned contribution one component publishes.
///
/// The values are the component's own declaration: its identity as the label
/// the primitive renders. No value here is a widget, a callback or a host
/// object.
fn planned(component: &CompositionComponent) -> PlannedContribution {
    let mut inputs = BTreeMap::new();
    inputs.insert("label".to_owned(), json!(component.component));
    PlannedContribution {
        id: component.component.clone(),
        primitive: component.primitive,
        resource_id: None,
        resource_format: component_resource_format(component),
        action_ref: component.action_ref.clone(),
        regions: Vec::new(),
        inputs,
    }
}

// ---------------------------------------------------------------------------
// Publication
// ---------------------------------------------------------------------------

/// One package generation a host publishes, with the resources it carries.
///
/// The value is assembled by the owner that holds the published generations, so
/// a plan is always decided against the generations this host actually serves
/// rather than against a caller's idea of them.
pub struct PublishedGeneration<'a> {
    /// The package that published the generation.
    pub package_id: &'a str,
    /// Which prepared release of that package the resources came from.
    pub package_generation: u64,
    /// Every typed resource that release carries.
    pub resources: &'a [ResourceDeclaration],
}

/// The published document's location inside one data root.
pub fn mount_plan_path(data_home: &Path) -> PathBuf {
    data_home.join(MOUNT_PLAN_RECORD)
}

/// The plan over every generation a host publishes, as one document.
///
/// Each generation is decided by the contract's own planner against the same
/// published bindings, so a component of a generation this host does not serve
/// is still considered and refused with its own reason rather than dropped.
pub fn plan_published_generations(
    bindings: &ResourceBindings,
    generations: &[PublishedGeneration<'_>],
    host_actions: &[&str],
) -> ResourceMountPlan {
    let mut decisions = Vec::new();
    for generation in generations {
        let plan = plan_generation_mount_with_actions(
            bindings,
            generation.package_id,
            generation.package_generation,
            generation.resources,
            host_actions,
        );
        decisions.extend(plan.decisions);
    }
    ResourceMountPlan {
        revision: bindings.revision(),
        bindings: bindings.clone(),
        host_actions: host_actions
            .iter()
            .map(|action| (*action).to_owned())
            .collect(),
        decisions,
    }
}

/// Publish one plan where the client reads it.
///
/// The write goes through this module's own atomic replacement: a temporary file
/// inside the same directory is renamed over the document, so a reader sees the
/// previous document or the new one and never a half-written one.
pub fn publish_mount_plan(
    data_home: &Path,
    plan: &ResourceMountPlan,
) -> Result<(), ApplicationFailure> {
    let document = plan.to_document();
    let text = serde_json::to_string_pretty(&document)
        .map_err(|_| refusal("mount_plan_unserializable", MOUNT_PLAN_STAGE))?;
    replace_file_atomically(&mount_plan_path(data_home), &text)
}

/// The document this host published, or `None` while it published none.
///
/// A document that is not this format, not this version, or not readable as one
/// plan is refused rather than partially understood: a renderer must never act
/// on half a plan.
pub fn read_published_mount_plan(data_home: &Path) -> Result<Option<Value>, ApplicationFailure> {
    let Some(text) = read_bounded_text(&mount_plan_path(data_home), MAX_MOUNT_PLAN_BYTES)? else {
        return Ok(None);
    };
    let document: Value = serde_json::from_str(&text)
        .map_err(|_| refusal("mount_plan_invalid", MOUNT_PLAN_STAGE).with_field("document"))?;
    if document.get("format").and_then(Value::as_str) != Some(MOUNT_PLAN_FORMAT)
        || document.get("version").and_then(Value::as_u64) != Some(MOUNT_PLAN_VERSION)
    {
        return Err(refusal("mount_plan_invalid", MOUNT_PLAN_STAGE).with_field("document"));
    }
    Ok(Some(document))
}
