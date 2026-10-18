//! C13: declarative interface contributions.
//!
//! A contribution is data. It names one of the host's precompiled primitives, the
//! resource or action it binds to, and the least profile it needs — and that is
//! the whole interface. There is no script field, no widget tree, no direct RPC
//! handle and no global store, because a downloaded package may not put code into
//! an already-shipped client, and because focus, input-method editing,
//! accessibility, theming and prepared-value consistency are the host's job.
//!
//! Three rules keep a contribution contained:
//!
//! - **It fails alone.** A contribution whose profile this host does not serve is
//!   not mounted; every other contribution, and every other feature of the
//!   client, mounts exactly as before ([`plan_mount`]).
//! - **A secret stays in the host control.** A field may declare that a secret is
//!   needed; it may not carry one. The host collects it and hands the package a
//!   `credential:` handle ([`Contribution::validate`]).
//! - **A stale preparation is refused by generation.** A prepared value computed
//!   for a generation that has been replaced is not installed, and no unrelated
//!   feature re-reads its source on its account
//!   ([`prepared_generation_accepted`]).

use crate::profile::ExtensionProfile;
use crate::refusal;
use licoup_application::{ApplicationFailure, is_namespaced};
use serde::{Deserialize, Serialize};

const STAGE: &str = "extension/ui";

/// The longest contribution identity accepted.
pub const MAX_CONTRIBUTION_ID_BYTES: usize = 160;

/// The versioned prepared graph resource view.
///
/// A resource view is not a new host primitive with new powers: it is one
/// bounded pure-data format a contributed `resource-view` may name and a host
/// build may compile a renderer for. The format carries stable
/// project/lane/node/gate identities, typed edges, plan/run revision, source
/// position, execution/acceptance/observation dimensions, ready/startable
/// reasons and opaque actions/results/evidence — and nothing executable.
pub const GRAPH_RESOURCE_V1: &str = "licoup.ui.graph-resource.v1";

/// Resource-view formats this client generation compiles a renderer for.
///
/// A contribution naming a format outside this set keeps its place in the
/// catalog and is refused locally: it is preserved for a newer host and no
/// other contribution is affected.
pub const RESOURCE_VIEW_FORMATS: [&str; 1] = [GRAPH_RESOURCE_V1];

/// What a contribution adds to the interface.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ContributionKind {
    Settings,
    Command,
    Navigation,
    MetricPanel,
    ResourceView,
}

impl ContributionKind {
    pub const ALL: [Self; 5] = [
        Self::Settings,
        Self::Command,
        Self::Navigation,
        Self::MetricPanel,
        Self::ResourceView,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Settings => "settings",
            Self::Command => "command",
            Self::Navigation => "navigation",
            Self::MetricPanel => "metric-panel",
            Self::ResourceView => "resource-view",
        }
    }
}

/// A host primitive a contribution may bind to.
///
/// These are compiled into the client. A contribution chooses among them; it does
/// not bring its own, and a capability that needs a genuinely new primitive
/// negotiates a core version instead of shipping one.
///
/// The wire names are the published vocabulary: a package manifest declares the
/// primitives a data package requires with exactly these strings, and the schema
/// under `schemas/extensions/manifest.schema.json` publishes the same set.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum HostPrimitive {
    Form,
    Table,
    Chart,
    Progress,
    Text,
    Action,
}

impl HostPrimitive {
    /// Every primitive this client generation compiles.
    pub const ALL: [Self; 6] = [
        Self::Form,
        Self::Table,
        Self::Chart,
        Self::Progress,
        Self::Text,
        Self::Action,
    ];

    /// The published wire name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Form => "form",
            Self::Table => "table",
            Self::Chart => "chart",
            Self::Progress => "progress",
            Self::Text => "text",
            Self::Action => "action",
        }
    }

    /// The primitive one wire name publishes, if this generation compiles it.
    pub fn from_wire(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|primitive| primitive.as_str() == name)
    }
}

/// Every primitive available to a contribution.
pub const HOST_PRIMITIVES: [HostPrimitive; 6] = HostPrimitive::ALL;

/// The kind of value one field holds.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum FieldType {
    Text,
    Number,
    Boolean,
    Select,
    /// A value the host collects in its own control and stores as a credential
    /// handle. The contribution never sees the secret.
    SecretRef,
}

/// One field of a form contribution.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Field {
    pub id: String,
    pub label: String,
    #[serde(rename = "type")]
    pub kind: FieldType,
    #[serde(default)]
    pub required: bool,
    /// The current value of an ordinary field. Always absent for
    /// [`FieldType::SecretRef`].
    #[serde(default)]
    pub value: Option<String>,
}

impl Field {
    pub fn new(id: impl Into<String>, label: impl Into<String>, kind: FieldType) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            kind,
            required: false,
            value: None,
        }
    }

    pub fn validate(&self) -> Result<(), ApplicationFailure> {
        if self.id.is_empty() || self.label.is_empty() {
            return Err(refusal::new("ui_contribution_invalid", STAGE).with_field("fields"));
        }
        if self.kind == FieldType::SecretRef && self.value.is_some() {
            return Err(
                refusal::actionable("ui_secret_inline_refused", STAGE, "fields.value")
                    .with_presentation_arg("expected", "credentialRef"),
            );
        }
        Ok(())
    }
}

/// One series of a metric panel.
///
/// A panel names the standardized metric it draws. It does not parse a vendor's
/// files and it does not carry the vendor's name into the chart.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Series {
    pub metric: String,
    pub label: String,
    pub unit: String,
}

impl Series {
    pub fn validate(&self) -> Result<(), ApplicationFailure> {
        if !is_namespaced(&self.metric) || self.label.is_empty() || self.unit.is_empty() {
            return Err(refusal::new("ui_contribution_invalid", STAGE).with_field("series"));
        }
        Ok(())
    }
}

/// What a contribution needs before it can be mounted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MountRequirement {
    /// It binds only to data the host already has.
    None,
    /// It needs a published profile.
    Served(ExtensionProfile),
    /// It needs a profile whose id this host does not publish, so it belongs to a
    /// newer host. It is preserved and not mounted.
    Unpublished,
}

/// One interface contribution.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Contribution {
    pub schema: String,
    /// The namespaced contribution identity.
    pub id: String,
    pub kind: ContributionKind,
    pub title: String,
    /// The least profile this contribution needs, as a profile id.
    #[serde(default)]
    pub required_profile: Option<String>,
    /// The resource the host prepares and hands back.
    #[serde(default)]
    pub resource_ref: Option<String>,
    /// The action this contribution may invoke. The host owns the action's
    /// authority; the contribution names it.
    #[serde(default)]
    pub action_ref: Option<String>,
    /// The bounded pure-data format a `resource-view` contribution wants
    /// rendered, such as [`GRAPH_RESOURCE_V1`]. Only a resource view may name
    /// one, and only a host that compiles a renderer for it mounts the
    /// contribution.
    #[serde(default)]
    pub resource_format: Option<String>,
    #[serde(default)]
    pub fields: Vec<Field>,
    #[serde(default)]
    pub series: Vec<Series>,
}

impl Contribution {
    /// What this contribution needs before it can mount.
    pub fn requirement(&self) -> MountRequirement {
        match self.required_profile.as_deref() {
            None => MountRequirement::None,
            Some(id) => match ExtensionProfile::from_id(id) {
                Some(profile) => MountRequirement::Served(profile),
                None => MountRequirement::Unpublished,
            },
        }
    }

    /// Structural validation. It names the offending field and refuses only this
    /// contribution.
    pub fn validate(&self) -> Result<(), ApplicationFailure> {
        if self.schema != crate::wire::UI {
            return Err(refusal::new("ui_contribution_invalid", STAGE).with_field("schema"));
        }
        if !is_namespaced(&self.id) || self.id.len() > MAX_CONTRIBUTION_ID_BYTES {
            return Err(refusal::new("ui_contribution_invalid", STAGE).with_field("id"));
        }
        if self.title.is_empty() {
            return Err(refusal::new("ui_contribution_invalid", STAGE).with_field("title"));
        }
        match (&self.resource_format, self.kind) {
            (None, _) => {}
            (Some(format), ContributionKind::ResourceView) => {
                if !is_namespaced(format) || format.len() > MAX_CONTRIBUTION_ID_BYTES {
                    return Err(
                        refusal::new("ui_contribution_invalid", STAGE).with_field("resourceFormat")
                    );
                }
            }
            // A settings form, command, navigation entry or metric panel has no
            // format to render: naming one is a declaration error, not a newer
            // capability this host could preserve for later.
            (Some(_), _) => {
                return Err(
                    refusal::new("ui_contribution_invalid", STAGE).with_field("resourceFormat")
                );
            }
        }
        if self.kind == ContributionKind::MetricPanel {
            if self.series.is_empty() {
                return Err(refusal::new("ui_contribution_invalid", STAGE).with_field("series"));
            }
        } else if !self.series.is_empty() {
            return Err(refusal::new("ui_contribution_invalid", STAGE).with_field("series"));
        }
        for field in &self.fields {
            field.validate()?;
        }
        for series in &self.series {
            series.validate()?;
        }
        Ok(())
    }
}

/// One contribution and the decision made about it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mount<'a> {
    pub contribution: &'a Contribution,
    /// `None` when it mounts; otherwise why it does not.
    pub blocked: Option<&'static str>,
}

impl Mount<'_> {
    pub fn is_mounted(&self) -> bool {
        self.blocked.is_none()
    }
}

/// Decide each contribution against the profiles this host serves.
///
/// The decision is per contribution. A settings form that needs endpoint
/// collaboration is not mounted when collaboration is absent, and the local
/// conversation pages are not affected by it in any way.
pub fn plan_mount<'a>(
    contributions: &'a [Contribution],
    served: &[ExtensionProfile],
) -> Vec<Mount<'a>> {
    plan_mount_with_resource_formats(contributions, served, &RESOURCE_VIEW_FORMATS)
}

/// Decide each contribution against the profiles this host serves and the
/// resource-view formats this host build compiles a renderer for.
///
/// A resource-view contribution is mounted only when its format is in
/// `available_formats`. A view whose format this host does not compile is
/// refused locally and preserved: it belongs to a newer host, and the
/// refusal reaches only this contribution. A view without a format cannot be
/// rendered at all, so it is refused with its own reason.
pub fn plan_mount_with_resource_formats<'a>(
    contributions: &'a [Contribution],
    served: &[ExtensionProfile],
    available_formats: &[&str],
) -> Vec<Mount<'a>> {
    contributions
        .iter()
        .map(|contribution| {
            let blocked = if contribution.validate().is_err() {
                Some("contribution_invalid")
            } else {
                match contribution.requirement() {
                    MountRequirement::None => None,
                    MountRequirement::Served(profile) if served.contains(&profile) => None,
                    MountRequirement::Served(_) => Some("profile_not_installed"),
                    MountRequirement::Unpublished => Some("profile_unpublished"),
                }
            }
            .or_else(|| {
                match (contribution.kind, contribution.resource_format.as_deref()) {
                    (ContributionKind::ResourceView, Some(format)) => (!available_formats
                        .contains(&format))
                    .then_some("resource_format_unavailable"),
                    (ContributionKind::ResourceView, None) => Some("resource_format_missing"),
                    _ => None,
                }
            });
            Mount {
                contribution,
                blocked,
            }
        })
        .collect()
}

/// Refuse a prepared value computed for a generation that is no longer current.
///
/// A late preparation is not installed and no unrelated feature re-reads its
/// source because of it. The comparison is by generation, not by time: a value
/// that arrives after a registry switch belongs to the old one whatever its
/// timestamp says.
pub fn prepared_generation_accepted(
    current_generation: u64,
    prepared_generation: u64,
) -> Result<(), ApplicationFailure> {
    if current_generation == prepared_generation {
        return Ok(());
    }
    Err(refusal::new("ui_prepared_generation_stale", STAGE)
        .with_field("generation")
        .with_presentation_arg("current", &current_generation.to_string())
        .with_presentation_arg("prepared", &prepared_generation.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn form(id: &str) -> Contribution {
        Contribution {
            schema: crate::wire::UI.to_owned(),
            id: id.to_owned(),
            kind: ContributionKind::Settings,
            title: "Endpoint".to_owned(),
            required_profile: None,
            resource_ref: Some("resource.example/endpoint".to_owned()),
            action_ref: Some("action.example/save".to_owned()),
            resource_format: None,
            fields: vec![Field::new("endpoint", "Endpoint", FieldType::Text)],
            series: Vec::new(),
        }
    }

    fn graph_view(id: &str, format: Option<&str>) -> Contribution {
        Contribution {
            kind: ContributionKind::ResourceView,
            resource_ref: Some("resource.example/project-collaboration".to_owned()),
            action_ref: Some("action.example/takeover".to_owned()),
            fields: Vec::new(),
            resource_format: format.map(str::to_owned),
            ..form(id)
        }
    }

    #[test]
    fn a_resource_view_mounts_only_with_a_compiled_renderer() {
        let served = [graph_view("example.view/graph", Some(GRAPH_RESOURCE_V1))];
        let mounts = plan_mount(&served, &[]);
        assert!(mounts[0].is_mounted(), "the compiled v1 renderer mounts it");

        let without_renderer = plan_mount_with_resource_formats(&served, &[], &[]);
        assert_eq!(
            without_renderer[0].blocked,
            Some("resource_format_unavailable"),
            "a host without the renderer refuses only this contribution"
        );

        let future = [graph_view(
            "example.view/graph-v2",
            Some("licoup.ui.graph-resource.v2"),
        )];
        let future_mounts = plan_mount_with_resource_formats(&future, &[], &[GRAPH_RESOURCE_V1]);
        assert_eq!(
            future_mounts[0].blocked,
            Some("resource_format_unavailable")
        );

        let unversioned = [graph_view("example.view/graph", None)];
        assert_eq!(
            plan_mount(&unversioned, &[])[0].blocked,
            Some("resource_format_missing")
        );
    }

    #[test]
    fn an_unavailable_view_never_blocks_its_siblings() {
        let contributions = [
            graph_view("example.view/future", Some("licoup.ui.graph-resource.v9")),
            graph_view("example.view/graph", Some(GRAPH_RESOURCE_V1)),
            form("example.settings/local"),
        ];
        let plan = plan_mount_with_resource_formats(&contributions, &[], &[GRAPH_RESOURCE_V1]);
        assert_eq!(plan[0].blocked, Some("resource_format_unavailable"));
        assert!(plan[1].is_mounted());
        assert!(plan[2].is_mounted(), "the settings form is untouched");
    }

    #[test]
    fn only_a_resource_view_may_name_a_format() {
        let mut overreach = form("example.settings/endpoint");
        overreach.resource_format = Some(GRAPH_RESOURCE_V1.to_owned());
        let failure = overreach.validate().expect_err("format on a settings form");
        assert_eq!(failure.code, "ui_contribution_invalid");
        assert_eq!(failure.field.as_deref(), Some("resourceFormat"));

        let unruly = graph_view("example.view/graph", Some("plain"));
        assert!(unruly.validate().is_err());
    }

    #[test]
    fn a_secret_field_carries_no_secret() {
        assert!(form("example.settings/endpoint").validate().is_ok());

        let mut with_secret = form("example.settings/endpoint");
        with_secret.fields = vec![Field {
            value: Some("placeholder-secret".to_owned()),
            ..Field::new("key", "API key", FieldType::SecretRef)
        }];
        let failure = with_secret.validate().expect_err("inline secret");
        assert_eq!(failure.code, "ui_secret_inline_refused");
        let contributions = [with_secret, form("example.settings/local")];
        let mounts = plan_mount(&contributions, &[]);
        assert_eq!(mounts[0].blocked, Some("contribution_invalid"));
        assert!(mounts[1].is_mounted());
        assert_eq!(
            failure.recovery,
            licoup_application::RecoveryAction::InstallOrRetryRuntime
        );
        assert_eq!(
            failure.presentation_args.get("expected"),
            Some("credentialRef")
        );
    }

    #[test]
    fn a_contribution_that_cannot_mount_blocks_only_itself() {
        let mut needs_pairing = form("example.settings/pairing");
        needs_pairing.required_profile = Some(ExtensionProfile::AgentExecution.id().to_owned());
        let mut needs_future = form("example.settings/future");
        needs_future.required_profile = Some("future-profile".to_owned());
        let plain = form("example.settings/local");
        let contributions = vec![needs_pairing, needs_future, plain];

        let plan = plan_mount(&contributions, &[]);
        assert_eq!(plan[0].blocked, Some("profile_not_installed"));
        assert_eq!(plan[1].blocked, Some("profile_unpublished"));
        assert!(plan[2].is_mounted(), "the plain contribution still mounts");

        let plan = plan_mount(&contributions, &[ExtensionProfile::AgentExecution]);
        assert!(plan[0].is_mounted());
        assert!(!plan[1].is_mounted());
    }

    #[test]
    fn a_late_preparation_is_refused_by_generation_not_by_time() {
        assert!(prepared_generation_accepted(7, 7).is_ok());
        let failure = prepared_generation_accepted(8, 7).expect_err("stale");
        assert_eq!(failure.code, "ui_prepared_generation_stale");
    }

    #[test]
    fn a_metric_panel_names_standardized_metrics() {
        let mut panel = form("example.panel/usage");
        panel.kind = ContributionKind::MetricPanel;
        panel.series = vec![Series {
            metric: "licoup.tokens.input".to_owned(),
            label: "Input tokens".to_owned(),
            unit: "tokens".to_owned(),
        }];
        assert!(panel.validate().is_ok());

        let mut empty = panel.clone();
        empty.series = Vec::new();
        assert!(empty.validate().is_err());

        let mut unruly = panel.clone();
        unruly.series[0].metric = "tokens".to_owned();
        assert!(unruly.validate().is_err());

        let mut overreach = form("example.settings/plain");
        overreach.series = panel.series;
        assert!(overreach.validate().is_err());
    }

    #[test]
    fn the_host_primitive_vocabulary_is_the_published_wire_set() {
        let names: Vec<&str> = HOST_PRIMITIVES
            .iter()
            .map(|primitive| primitive.as_str())
            .collect();
        assert_eq!(
            names,
            ["form", "table", "chart", "progress", "text", "action"],
            "the manifest schema publishes exactly this set"
        );
        for primitive in HOST_PRIMITIVES {
            let wire = serde_json::to_value(primitive).expect("serialize");
            assert_eq!(wire, serde_json::json!(primitive.as_str()));
            assert_eq!(
                serde_json::from_value::<HostPrimitive>(wire).expect("deserialize"),
                primitive
            );
            assert_eq!(
                HostPrimitive::from_wire(primitive.as_str()),
                Some(primitive)
            );
        }
        assert_eq!(HostPrimitive::from_wire("canvas"), None);
    }

    #[test]
    fn a_contribution_has_no_place_to_put_code() {
        let wire = serde_json::to_value(form("example.settings/endpoint")).expect("serialize");
        for forbidden in [
            "code",
            "script",
            "handler",
            "widget",
            "source",
            "repository",
        ] {
            assert!(
                wire.get(forbidden).is_none(),
                "{forbidden} must not be here"
            );
        }
        assert_eq!(HOST_PRIMITIVES.len(), ContributionKind::ALL.len() + 1);
    }
}
