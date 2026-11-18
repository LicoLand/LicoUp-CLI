//! Plan/apply/update/uninstall lifecycle. Argv-only after one confirmation.
//!
//! Apply stages the vendor artifact an `official-artifact` channel installs
//! from before it runs that channel's install argv; [`super::acquisition`] owns
//! the fetch and the published-digest check, and writes only into the Hub's
//! private staging root.
//!
//! A channel that declares an install destination adds one more visible argv
//! step, which places the staged result at that destination. `{install}` is
//! derived from the declaration, so uninstall and verify name a path the recipe
//! stated instead of a path the caller had to supply or a name PATH happened to
//! answer; a caller-supplied `installRef` still wins for an install the caller
//! owns. The destination is expanded with the discovery owner's own roots and
//! must be a location that owner admits, so what the Hub installs is what the
//! next Agent scan finds.

use super::acquisition::{
    self, AcquisitionFailure, AcquisitionRequest, ArtifactFetcher, ArtifactRole,
    INSTALL_DESTINATION_UNADMITTED, INSTALL_DESTINATION_UNDECLARED, VendorArtifactFetcher,
};
use super::argv::{ArgvKind, ArgvRunner, ProcessArgvRunner, validate_program_args};
use super::capabilities::capabilities_from_params;
use super::confirmation::{self, install_argv_for};
use super::contract::{
    AgentRecipe, HubEvent, InstallChannel, InstallOwnership, LIFECYCLE_APPLYING,
    LIFECYCLE_AVAILABLE, LIFECYCLE_CONFIRMED, LIFECYCLE_FAILED, LIFECYCLE_NEEDS_LOGIN,
    LIFECYCLE_PLANNED, LIFECYCLE_RESCANNING, LIFECYCLE_VERIFYING, OWNERSHIP_EXTERNAL,
    OWNERSHIP_OWNED, PlatformInstallCapabilities, RecipeRegistryDocument,
};
use super::ownership::{self, store_from_params};
use super::recipes::{self, agent_recipe};
use super::selector;
use crate::platform::client_state::ClientStateStore;
use anyhow::{Result, anyhow, ensure};
use licoup_agent_targets::domain::targets::scan_paths;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;

/// Status of a plan admission refuses because its channel cannot be staged.
const STATUS_UNAVAILABLE: &str = "unavailable";

pub struct HubContext {
    pub store: ClientStateStore,
    pub capabilities: PlatformInstallCapabilities,
    pub runner: Arc<dyn ArgvRunner>,
    /// The bounded byte source acquisition stages from.
    fetcher: Arc<dyn ArtifactFetcher>,
    /// The recipe registry this lifecycle resolves against. The bundled
    /// warehouse by default; a test substitutes a synthetic document.
    registry: Arc<RecipeRegistryDocument>,
}

impl HubContext {
    pub fn from_params(params: &Value) -> Result<Self> {
        Self::build(
            params,
            Arc::new(ProcessArgvRunner),
            Arc::new(VendorArtifactFetcher),
        )
    }

    pub fn with_runner(params: &Value, runner: Arc<dyn ArgvRunner>) -> Result<Self> {
        Self::build(params, runner, Arc::new(VendorArtifactFetcher))
    }

    /// Explicit ports for a deterministic lifecycle test.
    #[cfg(test)]
    pub(crate) fn with_ports(
        params: &Value,
        runner: Arc<dyn ArgvRunner>,
        fetcher: Arc<dyn ArtifactFetcher>,
        registry: Arc<RecipeRegistryDocument>,
    ) -> Result<Self> {
        Ok(Self {
            store: store_from_params(params)?,
            capabilities: capabilities_from_params(params)?,
            runner,
            fetcher,
            registry,
        })
    }

    fn build(
        params: &Value,
        runner: Arc<dyn ArgvRunner>,
        fetcher: Arc<dyn ArtifactFetcher>,
    ) -> Result<Self> {
        Ok(Self {
            store: store_from_params(params)?,
            capabilities: capabilities_from_params(params)?,
            runner,
            fetcher,
            registry: Arc::new(recipes::registry()?.clone()),
        })
    }

    fn agent(&self, agent_id: &str) -> Result<&AgentRecipe> {
        agent_recipe(&self.registry, agent_id)
    }
}

pub fn plan(params: &Value) -> Result<Value> {
    plan_with(&HubContext::from_params(params)?, params)
}

pub fn apply(params: &Value) -> Result<Value> {
    apply_with(&HubContext::from_params(params)?, params)
}

pub fn plan_with(ctx: &HubContext, params: &Value) -> Result<Value> {
    let operation = operation_of(params, "install");
    let agent_id = agent_id(params)?;
    let agent = ctx.agent(&agent_id)?;
    let present = discovery_present(params, &agent_id);
    let owned = ownership::get(&ctx.store, &agent_id)?;
    let ownership = ownership::resolve_ownership(owned.as_ref(), present);

    if operation == "install" && ownership == OWNERSHIP_EXTERNAL {
        return Ok(json!({
            "ok": true,
            "status": "external_protected",
            "operation": operation,
            "agentId": agent_id,
            "ownership": OWNERSHIP_EXTERNAL,
            "requiresConfirmation": false,
            "selectedChannel": Value::Null
        }));
    }
    if operation != "install" && ownership != OWNERSHIP_OWNED {
        return Ok(json!({
            "ok": false,
            "status": "external_protected",
            "code": "external_install_protected",
            "operation": operation,
            "agentId": agent_id,
            "ownership": ownership
        }));
    }

    let requested_channel = params
        .get("channelId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let selected = if let Some(requested) = requested_channel {
        let channel = selector::channel_by_id(agent, requested)?;
        if operation == "install" {
            ensure!(
                selector::channel_matches(channel, &ctx.capabilities),
                "channel_unavailable"
            );
        } else {
            let record = owned
                .as_ref()
                .ok_or_else(|| anyhow!("external_install_protected"))?;
            ensure!(requested == record.channel_id, "channel_mismatch");
        }
        channel
    } else if operation != "install" {
        let record = owned
            .as_ref()
            .ok_or_else(|| anyhow!("external_install_protected"))?;
        selector::channel_by_id(agent, &record.channel_id)?
    } else {
        selector::select_channel(agent, &ctx.capabilities)?.channel
    };
    let argv = argv_for(&operation, &ctx.capabilities.os, selected);
    argv_guard(&argv, selected)?;
    let placement = placement_argv(&operation, selected);
    if !placement.is_empty() {
        argv_guard(&placement, selected)?;
    }
    let needs = operation_needs(&operation, &ctx.capabilities.os, selected);
    let acquisition = match plan_acquisition(ctx, params, &agent_id, selected, &needs)? {
        PlanAdmission::Ready(descriptor) => descriptor,
        PlanAdmission::Refused(failure) => {
            return Ok(failure_json(
                STATUS_UNAVAILABLE,
                &failure,
                &operation,
                &agent_id,
                selected,
            ));
        }
    };
    let confirmation =
        confirmation::token(&operation, &agent_id, selected, &requested_version(params));
    Ok(json!({
        "ok": true,
        "status": LIFECYCLE_PLANNED,
        "operation": operation,
        "agentId": agent_id,
        "adaptation": agent.adaptation,
        "ownership": ownership,
        "requiresConfirmation": true,
        "confirmation": confirmation,
        "acquisition": acquisition,
        "selectedChannel": {
            "id": selected.id,
            "kind": selected.kind,
            "packageCoordinate": selected.package_coordinate,
            "officialSource": selected.official_source,
            "versionPolicy": selected.version_policy,
            "argv": argv,
            "placementArgv": placement
        }
    }))
}

/// Whether a plan can be staged, and what the caller is about to fetch.
///
/// This runs before any confirmation is issued and never touches the network:
/// a channel whose artifact declaration cannot produce a verified staged file
/// is refused here, so the user is not asked to confirm an install that must
/// fail.
enum PlanAdmission {
    Ready(Value),
    Refused(AcquisitionFailure),
}

fn plan_acquisition(
    ctx: &HubContext,
    params: &Value,
    agent_id: &str,
    channel: &InstallChannel,
    needs: &StagedNeeds,
) -> Result<PlanAdmission> {
    if let Err(failure) = verify_staged_requirements(params, channel, needs, &ctx.capabilities) {
        return Ok(PlanAdmission::Refused(failure));
    }
    if !needs.artifact && !needs.script && !needs.staging {
        return Ok(PlanAdmission::Ready(Value::Null));
    }
    let spec = match acquisition::artifact_spec(channel) {
        Ok(spec) => spec,
        Err(failure) => return Ok(PlanAdmission::Refused(failure)),
    };
    if let Err(failure) = acquisition::declared_integrity(spec) {
        return Ok(PlanAdmission::Refused(failure));
    }
    let version = match acquisition::requested_version(params) {
        Ok(version) => version,
        Err(failure) => return Ok(PlanAdmission::Refused(failure)),
    };
    let source_url = match acquisition::artifact_url(spec, &ctx.capabilities, &version) {
        Ok(url) => url,
        Err(error) => return Ok(PlanAdmission::Refused(failure_of(error))),
    };
    // Everything an acquisition can reject without the network is rejected
    // here, while the caller has not yet confirmed anything.
    if let Err(error) = acquisition::artifact_file_name(&source_url) {
        return Ok(PlanAdmission::Refused(failure_of(error)));
    }
    if let Err(error) = acquisition::digest_document_url(spec, &ctx.capabilities, &version) {
        return Ok(PlanAdmission::Refused(failure_of(error)));
    }
    Ok(PlanAdmission::Ready(json!({
        "sourceUrl": source_url,
        "integrity": "published-digest",
        "role": match needs.role() {
            Ok(role) => role.as_str(),
            Err(failure) => return Ok(PlanAdmission::Refused(failure)),
        },
        "staging": "hub-private-state",
        "officialSource": channel.official_source,
        "agentId": agent_id
    })))
}

/// The staged values one confirmed apply must resolve before it runs argv.
///
/// The receipt names the verified bytes without naming a local path, so a caller
/// can report what was staged.
fn resolve_staged_values(
    ctx: &HubContext,
    params: &Value,
    agent: &AgentRecipe,
    channel: &InstallChannel,
    needs: &StagedNeeds,
) -> Result<(StagedValues, Option<Value>), AcquisitionFailure> {
    verify_staged_requirements(params, channel, needs, &ctx.capabilities)?;
    let mut values = StagedValues::default();
    if needs.install {
        values.install = install_reference(params, channel, &ctx.capabilities)?;
    }
    if !needs.artifact && !needs.script && !needs.staging {
        values.version = requested_version(params);
        return Ok((values, None));
    }
    values.version = acquisition::requested_version(params)?;
    let request = AcquisitionRequest {
        agent,
        channel,
        capabilities: &ctx.capabilities,
    };
    if needs.artifact || needs.script {
        let staged = acquisition::stage(
            &ctx.store,
            params,
            &request,
            needs.role()?,
            ctx.fetcher.as_ref(),
        )
        .map_err(failure_of)?;
        values.staging = Some(staged.staging_dir.to_string_lossy().to_string());
        match staged.role {
            ArtifactRole::Archive => {
                values.artifact = Some(staged.file_path.to_string_lossy().to_string())
            }
            ArtifactRole::Script => {
                values.script = Some(staged.file_path.to_string_lossy().to_string())
            }
        }
        let receipt = json!({
            "role": staged.role.as_str(),
            "sha256": staged.sha256,
            "bytes": staged.bytes,
            "resumed": staged.resumed
        });
        return Ok((values, Some(receipt)));
    }
    let directory =
        acquisition::prepare_staging_dir(&ctx.store, params, &request).map_err(failure_of)?;
    values.staging = Some(directory.to_string_lossy().to_string());
    Ok((values, None))
}

/// The values one operation's argv substitutes. Nothing defaults: a placeholder
/// without a value is a refusal, because the literal placeholder name would
/// otherwise reach the install argv as a path that does not exist.
#[derive(Clone, Debug, Default)]
struct StagedValues {
    staging: Option<String>,
    artifact: Option<String>,
    script: Option<String>,
    install: Option<String>,
    version: String,
}

impl StagedValues {
    fn substitute(&self, arg: &str) -> Result<String, AcquisitionFailure> {
        let mut output = arg.to_string();
        for (placeholder, value) in [
            ("{staging}", self.staging.as_deref()),
            ("{artifact}", self.artifact.as_deref()),
            ("{script}", self.script.as_deref()),
            ("{install}", self.install.as_deref()),
        ] {
            if !output.contains(placeholder) {
                continue;
            }
            let value = value.ok_or_else(|| {
                AcquisitionFailure::with_detail(
                    acquisition::ARTIFACT_PLACEHOLDER_UNRESOLVED,
                    placeholder,
                )
            })?;
            output = output.replace(placeholder, value);
        }
        Ok(output.replace("{version}", &self.version))
    }
}

/// The placeholders one argv uses, and whether a plan can satisfy them.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct StagedNeeds {
    staging: bool,
    artifact: bool,
    script: bool,
    install: bool,
}

impl StagedNeeds {
    fn of(argv: &[String]) -> Self {
        let contains = |placeholder: &str| argv.iter().any(|arg| arg.contains(placeholder));
        Self {
            staging: contains("{staging}"),
            artifact: contains("{artifact}"),
            script: contains("{script}"),
            install: contains("{install}"),
        }
    }

    fn role(&self) -> Result<ArtifactRole, AcquisitionFailure> {
        match (self.artifact, self.script) {
            (true, false) => Ok(ArtifactRole::Archive),
            (false, true) => Ok(ArtifactRole::Script),
            _ => Err(AcquisitionFailure::new(
                acquisition::ARTIFACT_ROLE_AMBIGUOUS,
            )),
        }
    }
}

/// Every staged requirement of one argv must be satisfiable as declared.
fn verify_staged_requirements(
    params: &Value,
    channel: &InstallChannel,
    needs: &StagedNeeds,
    capabilities: &PlatformInstallCapabilities,
) -> Result<(), AcquisitionFailure> {
    if needs.install && install_reference(params, channel, capabilities)?.is_none() {
        return Err(AcquisitionFailure::new(
            acquisition::INSTALL_REFERENCE_UNRESOLVED,
        ));
    }
    if needs.artifact || needs.script || needs.staging {
        acquisition::artifact_spec(channel)?;
    }
    Ok(())
}

fn failure_json(
    status: &str,
    failure: &AcquisitionFailure,
    operation: &str,
    agent_id: &str,
    channel: &InstallChannel,
) -> Value {
    let mut payload = json!({
        "ok": false,
        "status": status,
        "code": failure.code,
        "operation": operation,
        "agentId": agent_id,
        "channelId": channel.id,
        "channelKind": channel.kind
    });
    if !failure.detail.is_empty() {
        payload["detail"] = Value::from(failure.detail.clone());
    }
    payload
}

/// Recovers the typed acquisition failure an `artifact_spec` refusal carries.
fn failure_of(error: anyhow::Error) -> AcquisitionFailure {
    match error.downcast::<AcquisitionFailure>() {
        Ok(failure) => failure,
        Err(error) => AcquisitionFailure::with_detail(
            acquisition::ARTIFACT_SOURCE_UNDECLARED,
            error.to_string(),
        ),
    }
}

pub fn apply_with(ctx: &HubContext, params: &Value) -> Result<Value> {
    let planned = plan_with(ctx, params)?;
    if planned.get("status").and_then(Value::as_str) == Some("external_protected")
        || planned.get("ok") == Some(&json!(false))
    {
        return Ok(planned);
    }
    let confirmation = planned
        .get("confirmation")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    confirmation::require(params, &confirmation)?;
    if params.get("cancel").and_then(Value::as_bool) == Some(true) {
        return Ok(json!({
            "ok": true,
            "status": "cancelled",
            "operation": planned["operation"],
            "agentId": planned["agentId"],
            "events": events(&["planned", "confirmed", "cancelled"])
        }));
    }
    let operation = planned
        .get("operation")
        .and_then(Value::as_str)
        .unwrap_or("install")
        .to_string();
    let agent_id = planned
        .get("agentId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let agent = ctx.agent(&agent_id)?;
    let channel_id = planned["selectedChannel"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let channel = selector::channel_by_id(agent, &channel_id)?;
    let argv_template = planned["selectedChannel"]["argv"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|value| value.as_str().map(str::to_string))
        .collect::<Vec<_>>();
    let placement_template = planned["selectedChannel"]["placementArgv"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|value| value.as_str().map(str::to_string))
        .collect::<Vec<_>>();
    let needs = operation_needs(&operation, &ctx.capabilities.os, channel);
    // Acquisition writes only into the Hub's private staging root; the install
    // argv below is what may touch the machine's own software locations.
    let (values, staged_receipt) = match resolve_staged_values(ctx, params, agent, channel, &needs)
    {
        Ok(resolved) => resolved,
        Err(failure) => {
            return Ok(apply_failure_json(&operation, &agent_id, &failure, channel));
        }
    };
    let argv = match resolved_step(&values, &argv_template) {
        Ok(argv) => argv,
        Err(failure) => {
            return Ok(apply_failure_json(&operation, &agent_id, &failure, channel));
        }
    };
    ensure!(!argv.is_empty(), "argv_forbidden");
    let program = argv[0].clone();
    let args = argv[1..].to_vec();
    validate_program_args(&program, &args, ArgvKind::Lifecycle)?;
    let mut lifecycle = vec![LIFECYCLE_PLANNED, LIFECYCLE_CONFIRMED, LIFECYCLE_APPLYING];
    let outcome = ctx.runner.run(&program, &args)?;
    if outcome.status != 0 {
        lifecycle.push(LIFECYCLE_FAILED);
        return Ok(json!({
            "ok": false,
            "status": LIFECYCLE_FAILED,
            "operation": operation,
            "agentId": agent_id,
            "events": events(&lifecycle),
            "runner": super::argv::outcome_json(&outcome)
        }));
    }
    // The placement step is part of the confirmed plan, and its only write
    // target is the destination `{install}` already names.
    if !placement_template.is_empty() {
        let placement = match resolved_step(&values, &placement_template) {
            Ok(placement) => placement,
            Err(failure) => {
                return Ok(apply_failure_json(&operation, &agent_id, &failure, channel));
            }
        };
        let outcome = ctx.runner.run(&placement[0], &placement[1..])?;
        if outcome.status != 0 {
            lifecycle.push(LIFECYCLE_FAILED);
            return Ok(json!({
                "ok": false,
                "status": LIFECYCLE_FAILED,
                "operation": operation,
                "agentId": agent_id,
                "events": events(&lifecycle),
                "runner": super::argv::outcome_json(&outcome)
            }));
        }
    }
    lifecycle.push(LIFECYCLE_VERIFYING);
    if !channel.verify_argv.is_empty() {
        let verify = match substitute_argv(&channel.verify_argv, &values) {
            Ok(verify) => verify,
            Err(error) => {
                return Ok(apply_failure_json(
                    &operation,
                    &agent_id,
                    &failure_of(error),
                    channel,
                ));
            }
        };
        let _ = ctx.runner.run(&verify[0], &verify[1..])?;
    }
    lifecycle.push(LIFECYCLE_RESCANNING);
    if operation == "uninstall" {
        ownership::remove(&ctx.store, &agent_id)?;
        lifecycle.push(LIFECYCLE_AVAILABLE);
        return Ok(json!({
            "ok": true,
            "status": "uninstalled",
            "operation": operation,
            "agentId": agent_id,
            "ownership": "none",
            "events": events(&lifecycle)
        }));
    }
    ownership::save(
        &ctx.store,
        InstallOwnership {
            agent_id: agent_id.clone(),
            channel_id: channel.id.clone(),
            channel_kind: channel.kind.clone(),
            package_coordinate: channel.package_coordinate.clone(),
            installed_version: super::version::concrete_display(&requested_version(params)),
            ownership: OWNERSHIP_OWNED.to_string(),
            lifecycle: if agent.requires_login {
                LIFECYCLE_NEEDS_LOGIN.to_string()
            } else {
                LIFECYCLE_AVAILABLE.to_string()
            },
        },
    )?;
    let status = if agent.requires_login {
        LIFECYCLE_NEEDS_LOGIN
    } else {
        LIFECYCLE_AVAILABLE
    };
    lifecycle.push(status);
    let mut result = json!({
        "ok": true,
        "status": status,
        "operation": operation,
        "agentId": agent_id,
        "ownership": OWNERSHIP_OWNED,
        "channelId": channel.id,
        "channelKind": channel.kind,
        "events": events(&lifecycle)
    });
    if let Some(receipt) = staged_receipt {
        result["stagedArtifact"] = receipt;
    }
    Ok(result)
}

fn argv_for(operation: &str, os: &str, channel: &InstallChannel) -> Vec<String> {
    match operation {
        "update" => channel.update_argv.clone(),
        "uninstall" => channel.uninstall_argv.clone(),
        _ => install_argv_for(os, channel),
    }
}

/// Every argv step one operation runs, in order.
///
/// An install or an update of a channel that declares where its executable
/// belongs runs the placement step after the operation's own argv, so the copy
/// into the declared destination is part of what the caller confirms. Uninstall
/// removes the declared destination and places nothing.
fn operation_steps(operation: &str, os: &str, channel: &InstallChannel) -> Vec<Vec<String>> {
    let mut steps = vec![argv_for(operation, os, channel)];
    if operation != "uninstall"
        && let Some(install) = channel.install.as_ref()
        && !install.argv.is_empty()
    {
        steps.push(install.argv.clone());
    }
    steps
}

/// The placement step one operation runs, which is empty when it places nothing.
fn placement_argv(operation: &str, channel: &InstallChannel) -> Vec<String> {
    if operation == "uninstall" {
        return Vec::new();
    }
    channel
        .install
        .as_ref()
        .map(|install| install.argv.clone())
        .unwrap_or_default()
}

/// The placeholders every step of one operation needs.
fn operation_needs(operation: &str, os: &str, channel: &InstallChannel) -> StagedNeeds {
    let steps = operation_steps(operation, os, channel);
    let mut combined = Vec::new();
    for step in steps {
        combined.extend(step);
    }
    StagedNeeds::of(&combined)
}

/// The install destination a channel declares for this host, resolved.
///
/// The directory template is expanded by the discovery owner, in that owner's
/// own root vocabulary, and the resolved path must be one that owner admits:
/// the Hub installs only where the next Agent scan looks, so an install cannot
/// land somewhere discovery will never find it. `Ok(None)` means the channel
/// declares no destination at all; an OS the declaration does not cover is a
/// typed refusal rather than an empty path.
fn declared_install_path(
    channel: &InstallChannel,
    capabilities: &PlatformInstallCapabilities,
) -> Result<Option<PathBuf>, AcquisitionFailure> {
    let Some(install) = channel.install.as_ref() else {
        return Ok(None);
    };
    let binary = install
        .binary
        .get(&capabilities.os)
        .ok_or_else(|| AcquisitionFailure::with_detail(INSTALL_DESTINATION_UNDECLARED, "binary"))?;
    let template = install.dir.get(&capabilities.os).ok_or_else(|| {
        AcquisitionFailure::with_detail(INSTALL_DESTINATION_UNDECLARED, "directory")
    })?;
    let roots = scan_paths::HostRoots::from_environment();
    let directory = scan_paths::expand_path_template(template, &roots)
        .ok_or_else(|| AcquisitionFailure::with_detail(INSTALL_DESTINATION_UNDECLARED, "root"))?;
    let path = directory.join(binary);
    if !scan_paths::install_destination_admitted(&path, &capabilities.os, &roots) {
        return Err(AcquisitionFailure::with_detail(
            INSTALL_DESTINATION_UNADMITTED,
            template,
        ));
    }
    Ok(Some(path))
}

/// The install reference one operation resolves, caller-supplied first.
///
/// A caller that owns an install states its own reference. A channel that
/// declares where its executable belongs derives the reference from that
/// declaration, so an install the Hub performed can be removed without the
/// caller having to know where it landed.
fn install_reference(
    params: &Value,
    channel: &InstallChannel,
    capabilities: &PlatformInstallCapabilities,
) -> Result<Option<String>, AcquisitionFailure> {
    if let Some(supplied) = params
        .get("installRef")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Ok(Some(supplied.to_string()));
    }
    Ok(
        declared_install_path(channel, capabilities)?
            .map(|path| path.to_string_lossy().to_string()),
    )
}

fn argv_guard(argv: &[String], channel: &InstallChannel) -> Result<()> {
    if argv.is_empty() {
        return Err(anyhow!("argv_forbidden"));
    }
    validate_program_args(&argv[0], &argv[1..], ArgvKind::for_channel(&channel.kind))
}

fn substitute_argv(argv: &[String], values: &StagedValues) -> Result<Vec<String>> {
    let mut substituted = Vec::with_capacity(argv.len());
    for arg in argv {
        substituted.push(values.substitute(arg)?);
    }
    Ok(substituted)
}

/// One argv step with every placeholder of the plan resolved.
fn resolved_step(
    values: &StagedValues,
    template: &[String],
) -> Result<Vec<String>, AcquisitionFailure> {
    let mut step = Vec::with_capacity(template.len());
    for arg in template {
        step.push(values.substitute(arg)?);
    }
    Ok(step)
}

/// The lifecycle result of a confirmed apply whose staging phase refused.
fn apply_failure_json(
    operation: &str,
    agent_id: &str,
    failure: &AcquisitionFailure,
    channel: &InstallChannel,
) -> Value {
    let mut payload = failure_json(LIFECYCLE_FAILED, failure, operation, agent_id, channel);
    payload["events"] = json!(events(&[
        LIFECYCLE_PLANNED,
        LIFECYCLE_CONFIRMED,
        LIFECYCLE_APPLYING,
        LIFECYCLE_FAILED,
    ]));
    payload
}

fn requested_version(params: &Value) -> String {
    params
        .get("version")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("latest")
        .to_string()
}

fn agent_id(params: &Value) -> Result<String> {
    params
        .get("agentId")
        .or_else(|| params.get("agent"))
        .or_else(|| params.get("target"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| anyhow!("recipe_not_found"))
}

fn operation_of(params: &Value, default: &str) -> String {
    params
        .get("operation")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(default)
        .to_string()
}

fn discovery_present(params: &Value, agent_id: &str) -> bool {
    params
        .get("discoveryCandidates")
        .and_then(Value::as_array)
        .and_then(|items| {
            items.iter().find(|item| {
                item.get("target")
                    .or_else(|| item.get("agentId"))
                    .and_then(Value::as_str)
                    == Some(agent_id)
            })
        })
        .map(fact_present)
        .unwrap_or(false)
}

fn fact_present(item: &Value) -> bool {
    if let Some(present) = item.get("present").and_then(Value::as_bool) {
        return present;
    }
    item.get("binaryPath")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.is_empty())
        || item
            .get("status")
            .and_then(Value::as_str)
            .is_some_and(|status| {
                status == "detected" || status == "configured" || status == "available"
            })
}

fn events(phases: &[&str]) -> Vec<HubEvent> {
    phases
        .iter()
        .map(|phase| HubEvent {
            phase: (*phase).to_string(),
            code: (*phase).to_string(),
        })
        .collect()
}
