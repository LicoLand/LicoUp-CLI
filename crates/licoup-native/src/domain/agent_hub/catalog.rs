//! Desktop Agent Hub projection. Cards follow the single AgentCatalog.

use super::capabilities::capabilities_from_params;
use super::contract::{AgentRecipe, DiscoveryFact, HOST_SCOPE, ManifestAgent, OWNERSHIP_NONE};
use super::ownership::{self, store_from_params};
use super::package_versions::{self, ChannelVersions};
use super::recipes;
use super::selector;
use super::version;
use super::version_check;
use crate::domain::agent_catalog::{self, AgentCatalogEntry};
use crate::domain::targets::normalize_target;
use anyhow::{Result, anyhow};
use serde_json::{Value, json};

pub fn catalog(params: &Value) -> Result<Value> {
    let store = store_from_params(params)?;
    let capabilities = capabilities_from_params(params)?;
    let warehouse = recipes::warehouse()?;
    let members = agent_catalog::supported_membership(agent_catalog::extra_ids_from_params(params));
    let requested = requested_agent_id(params, &members)?;
    let ownerships = ownership::load(&store)?;
    // A full live lookup remains available to native callers. The desktop
    // schedules per-Agent catalog reads independently and publishes each card
    // as it arrives.
    let live_members = params.get("liveLookup").and_then(Value::as_bool) == Some(true);
    // A supplied snapshot already carries the facts; nothing is inspected on top
    // of it, batched or otherwise.
    let supplied_snapshot = params.get("discoveryCandidates").is_some();
    let live_lookup = (requested.is_some() || live_members) && !supplied_snapshot;
    let facts = if live_members && requested.is_none() && !supplied_snapshot {
        member_facts(params, &members)
    } else {
        discovery_facts(params, requested.as_deref())?
    };
    let package_roots = live_lookup
        .then(|| package_versions::package_roots(params))
        .unwrap_or_default();
    let cards = match requested.as_deref() {
        Some(id) => {
            let entry = members
                .iter()
                .find(|entry| entry.id == id)
                .ok_or_else(|| anyhow!("agent_not_found"))?;
            vec![project_member_card(
                entry,
                &warehouse,
                &facts,
                &ownerships,
                &capabilities,
                live_lookup,
                &package_roots,
                params,
                true,
            )]
        }
        // A batched live refresh resolves each member the way its own per-card
        // request would, so the resulting cards are the same ones the client
        // used to collect one command at a time.
        None if live_members => members
            .iter()
            .map(|entry| {
                project_member_card(
                    entry,
                    &warehouse,
                    &facts,
                    &ownerships,
                    &capabilities,
                    live_lookup,
                    &package_roots,
                    params,
                    true,
                )
            })
            .collect::<Vec<_>>(),
        None => members
            .iter()
            .map(|entry| {
                project_member_card(
                    entry,
                    &warehouse,
                    &facts,
                    &ownerships,
                    &capabilities,
                    false,
                    &[],
                    params,
                    false,
                )
            })
            .collect::<Vec<_>>(),
    };
    Ok(json!({
        "ok": true,
        "hostScope": HOST_SCOPE,
        "scanGeneration": capabilities.scan_generation,
        "platform": {
            "os": capabilities.os,
            "architecture": capabilities.architecture,
            "managers": capabilities.managers
        },
        "pluginManagementBoundary": warehouse.manifest.plugin_management_boundary,
        "cards": cards
    }))
}

/// Live facts for every member, in membership order.
///
/// One member's inspection failing must not fail the refresh: the per-card path
/// this replaces isolated that failure to the one card, so a member without a
/// fact simply projects from its static metadata.
fn member_facts(params: &Value, members: &[AgentCatalogEntry]) -> Vec<DiscoveryFact> {
    members
        .iter()
        .filter_map(|entry| live_fact(params, &entry.id).ok().flatten())
        .collect()
}

fn project_member_card(
    entry: &AgentCatalogEntry,
    warehouse: &recipes::Warehouse,
    facts: &[DiscoveryFact],
    ownerships: &[super::contract::InstallOwnership],
    capabilities: &super::contract::PlatformInstallCapabilities,
    live_lookup: bool,
    package_roots: &[std::path::PathBuf],
    params: &Value,
    requested: bool,
) -> Value {
    if requested {
        if let Ok(agent) = recipes::agent_recipe(&warehouse.registry, &entry.id) {
            return project_recipe_card(
                agent,
                facts,
                ownerships,
                capabilities,
                live_lookup,
                package_roots,
                params,
            );
        }
        return project_registry_card(entry, facts, ownerships, params);
    }
    if let Some(agent) = warehouse
        .manifest
        .agents
        .iter()
        .find(|agent| agent.id == entry.id)
    {
        return project_manifest_card(agent, facts, ownerships, params);
    }
    project_registry_card(entry, facts, ownerships, params)
}

fn project_manifest_card(
    agent: &ManifestAgent,
    facts: &[DiscoveryFact],
    ownerships: &[super::contract::InstallOwnership],
    params: &Value,
) -> Value {
    let fact = facts.iter().find(|item| item.agent_id == agent.id);
    let present = fact.map(|item| item.present).unwrap_or(false);
    let record = ownerships.iter().find(|item| item.agent_id == agent.id);
    let ownership = ownership::resolve_ownership(record, present);
    let metadata = package_versions::from_params(params, &agent.id);
    let probed = if present {
        version_check::injected_probe(params, &agent.id)
            .map(|raw| version_check::parse_output(&agent.id, &raw, ""))
            .unwrap_or_default()
    } else {
        String::new()
    };
    let installed_version =
        installed_version_of(present, probed.as_str(), fact, &metadata, "", record);
    let latest_version = first_concrete([
        fact.map(|item| item.latest_version.as_str()).unwrap_or(""),
        metadata.latest.as_str(),
        "",
    ]);
    card_json(
        agent.id.as_str(),
        agent.label.as_str(),
        agent.adaptation.as_str(),
        agent.protocol.as_str(),
        agent.license.as_str(),
        agent.summary.as_str(),
        agent.homepage.as_str(),
        &agent.connection_modes,
        agent.requires_login,
        present,
        fact,
        ownership,
        record,
        None,
        None,
        String::new(),
        primary_action(ownership, record.is_some(), false, true),
        false,
        installed_version,
        latest_version,
        Vec::new(),
        None,
        None,
    )
}

fn project_recipe_card(
    agent: &AgentRecipe,
    facts: &[DiscoveryFact],
    ownerships: &[super::contract::InstallOwnership],
    capabilities: &super::contract::PlatformInstallCapabilities,
    live_lookup: bool,
    package_roots: &[std::path::PathBuf],
    params: &Value,
) -> Value {
    let fact = facts.iter().find(|item| item.agent_id == agent.id);
    let present = fact.map(|item| item.present).unwrap_or(false);
    let record = ownerships.iter().find(|item| item.agent_id == agent.id);
    let ownership = ownership::resolve_ownership(record, present);
    let selected = selector::select_channel(agent, capabilities).ok();
    let channel_kind = selector::chip_channel_kind(agent, selected.as_ref(), record);
    let metadata = package_versions::from_params(params, &agent.id);
    let local = if live_lookup {
        selected
            .as_ref()
            .map(|item| package_versions::lookup_local(item.channel, package_roots))
            .unwrap_or_default()
    } else {
        ChannelVersions::default()
    };
    let probed = version_check::installed_version(
        agent,
        selected.as_ref().map(|item| item.channel),
        present,
        live_lookup,
        params,
        fact.and_then(|fact| fact.executable_binding.as_deref()),
    );
    let installed_version = installed_version_of(
        present,
        probed.as_str(),
        fact,
        &metadata,
        local.installed.as_str(),
        record,
    );
    let latest_version = first_concrete([
        fact.map(|item| item.latest_version.as_str()).unwrap_or(""),
        metadata.latest.as_str(),
        local.latest.as_str(),
    ]);
    let install_channels = selector::available_channels(agent, capabilities)
        .into_iter()
        .map(|channel| {
            json!({
                "id": channel.id,
                "kind": channel.kind,
                "class": super::contract::channel_class(channel),
                "requiresDeveloperToolchain":
                    super::contract::channel_requires_developer_toolchain(channel),
                "versionPolicy": channel.version_policy,
                "officialSource": channel.official_source,
                "commandPreview": command_preview(&capabilities.os, channel)
            })
        })
        .collect::<Vec<_>>();
    let toolchain_only = selector::toolchain_only(agent);
    let first_launch_eligible = selector::first_launch_eligible(agent, capabilities);
    card_json(
        agent.id.as_str(),
        agent.label.as_str(),
        agent.adaptation.as_str(),
        agent.protocol.as_str(),
        agent.license.as_str(),
        agent.summary.as_str(),
        agent.homepage.as_str(),
        &agent.connection_modes,
        agent.requires_login,
        present,
        fact,
        ownership,
        record,
        selected.as_ref().map(|item| item.channel.id.as_str()),
        selected.as_ref().map(|item| item.channel.kind.as_str()),
        channel_kind,
        primary_action(ownership, record.is_some(), selected.is_some(), false),
        selected.is_some() && ownership == OWNERSHIP_NONE,
        installed_version,
        latest_version,
        install_channels,
        Some(toolchain_only),
        Some(first_launch_eligible),
    )
}

fn project_registry_card(
    entry: &AgentCatalogEntry,
    facts: &[DiscoveryFact],
    ownerships: &[super::contract::InstallOwnership],
    params: &Value,
) -> Value {
    let fact = facts.iter().find(|item| item.agent_id == entry.id);
    let present = fact.map(|item| item.present).unwrap_or(false);
    let record = ownerships.iter().find(|item| item.agent_id == entry.id);
    let ownership = ownership::resolve_ownership(record, present);
    let metadata = package_versions::from_params(params, &entry.id);
    let probed = if present {
        version_check::injected_probe(params, &entry.id)
            .map(|raw| version_check::parse_output(&entry.id, &raw, ""))
            .unwrap_or_default()
    } else {
        String::new()
    };
    let installed_version =
        installed_version_of(present, probed.as_str(), fact, &metadata, "", record);
    let latest_version = first_concrete([
        fact.map(|item| item.latest_version.as_str()).unwrap_or(""),
        metadata.latest.as_str(),
        "",
    ]);
    card_json(
        entry.id.as_str(),
        entry.label.as_str(),
        if entry.has_adapter {
            ""
        } else {
            "pending-evaluation"
        },
        "",
        "",
        entry.summary.as_str(),
        "",
        &["local".to_string()],
        false,
        present,
        fact,
        ownership,
        record,
        None,
        None,
        String::new(),
        primary_action(ownership, record.is_some(), false, false),
        false,
        installed_version,
        latest_version,
        Vec::new(),
        None,
        None,
    )
}

fn card_json(
    id: &str,
    label: &str,
    adaptation: &str,
    protocol: &str,
    license: &str,
    summary: &str,
    homepage: &str,
    connection_modes: &[String],
    requires_login: bool,
    present: bool,
    fact: Option<&DiscoveryFact>,
    ownership: &str,
    record: Option<&super::contract::InstallOwnership>,
    selected_channel_id: Option<&str>,
    selected_channel_kind: Option<&str>,
    channel_kind: String,
    primary_action: &'static str,
    installable: bool,
    installed_version: String,
    latest_version: String,
    install_channels: Vec<Value>,
    toolchain_only: Option<bool>,
    first_launch_eligible: Option<bool>,
) -> Value {
    let update_available = version::update_available(&installed_version, &latest_version);
    json!({
        "id": id,
        "label": label,
        "adaptation": adaptation,
        "protocol": protocol,
        "license": license,
        "summary": summary,
        "homepage": homepage,
        "connectionModes": connection_modes,
        "requiresLogin": requires_login,
        "present": present,
        "location": fact.map(|item| item.location.as_str()).unwrap_or("local"),
        "ownership": ownership,
        "lifecycle": record
            .map(|item| item.lifecycle.as_str())
            .unwrap_or(if present { "discovered" } else { "absent" }),
        "selectedChannelId": selected_channel_id,
        "selectedChannelKind": selected_channel_kind,
        "channelKind": channel_kind,
        "primaryAction": primary_action,
        "installable": installable,
        "installedVersion": installed_version.clone(),
        "latestVersion": latest_version,
        "updateAvailable": update_available,
        "version": installed_version,
        "installKinds": fact
            .map(|item| item.install_kinds.clone())
            .unwrap_or_default(),
        "toolchainOnly": toolchain_only,
        "firstLaunchEligible": first_launch_eligible,
        "installChannels": install_channels
    })
}

fn primary_action(
    ownership: &str,
    has_record: bool,
    selected: bool,
    manifest_only: bool,
) -> &'static str {
    if ownership == OWNERSHIP_NONE {
        if manifest_only || selected {
            "install"
        } else {
            "unsupported"
        }
    } else if has_record {
        "manage"
    } else {
        "open"
    }
}

fn installed_version_of(
    present: bool,
    probed: &str,
    fact: Option<&DiscoveryFact>,
    metadata: &ChannelVersions,
    local: &str,
    record: Option<&super::contract::InstallOwnership>,
) -> String {
    if present {
        first_concrete([
            probed,
            fact.map(|item| item.installed_version.as_str())
                .unwrap_or(""),
            metadata.installed.as_str(),
            local,
            record
                .map(|item| item.installed_version.as_str())
                .unwrap_or(""),
        ])
    } else {
        String::new()
    }
}

fn requested_agent_id(params: &Value, members: &[AgentCatalogEntry]) -> Result<Option<String>> {
    let Some(id) = params
        .get("agentId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(normalize_target)
    else {
        return Ok(None);
    };
    if !members.iter().any(|entry| entry.id == id) {
        return Err(anyhow!("agent_not_found"));
    }
    Ok(Some(id))
}

pub fn discovery_facts(params: &Value, agent_id: Option<&str>) -> Result<Vec<DiscoveryFact>> {
    if let Some(items) = params.get("discoveryCandidates").and_then(Value::as_array) {
        return Ok(items
            .iter()
            .filter_map(fact_from_value)
            .filter(|fact| agent_id.is_none_or(|id| fact.agent_id == id))
            .collect());
    }
    let Some(agent_id) = agent_id else {
        return Ok(Vec::new());
    };
    live_fact(params, agent_id).map(|fact| fact.into_iter().collect())
}

fn live_fact(params: &Value, agent_id: &str) -> Result<Option<DiscoveryFact>> {
    let mut inspect_params = params.clone();
    inspect_params["target"] = json!(agent_id);
    let inspected = crate::domain::targets::inspect_target_with_params(
        &crate::target_port::agent_target_port(),
        &inspect_params,
    )?;
    Ok(inspected.get("target").and_then(fact_from_value))
}

fn command_preview(os: &str, channel: &super::contract::InstallChannel) -> String {
    super::confirmation::install_argv_for(os, channel)
        .into_iter()
        .filter(|arg| !arg.trim().is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn first_concrete<const N: usize>(values: [&str; N]) -> String {
    values
        .into_iter()
        .map(|value| {
            let concrete = version::concrete_display(value);
            if concrete.is_empty() && version_check::is_cursor_date_hash_version(value.trim()) {
                value.trim().to_owned()
            } else {
                concrete
            }
        })
        .find(|value| !value.is_empty())
        .unwrap_or_default()
}

fn fact_from_value(item: &Value) -> Option<DiscoveryFact> {
    let agent_id = item
        .get("target")
        .or_else(|| item.get("agentId"))
        .and_then(Value::as_str)
        .map(normalize_target)
        .filter(|value| !value.is_empty())?;
    let present = item.get("present").and_then(Value::as_bool) == Some(true)
        || item
            .get("status")
            .and_then(Value::as_str)
            .is_some_and(|status| {
                status == "detected" || status == "configured" || status == "available"
            })
        || item
            .get("binaryPath")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.trim().is_empty());
    Some(DiscoveryFact {
        agent_id,
        present,
        location: item
            .get("location")
            .and_then(Value::as_str)
            .unwrap_or("local")
            .to_string(),
        scan_source: item
            .get("scanSource")
            .and_then(Value::as_str)
            .unwrap_or("target-adapters")
            .to_string(),
        installed_version: text_field(item, &["installedVersion", "version"]),
        latest_version: text_field(item, &["latestVersion"]),
        install_kinds: item
            .get("installKinds")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::trim)
                    .filter(|kind| !kind.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
        executable_binding: item
            .get("binaryPath")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(std::path::PathBuf::from),
    })
}

fn text_field(item: &Value, keys: &[&str]) -> String {
    keys.iter()
        .filter_map(|key| item.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .find(|value| !value.is_empty())
        .unwrap_or_default()
        .to_string()
}
