//! Per-Agent version domains and ranges, and the upgrade path out of them.
//!
//! One version is three things at once in this Hub: a *shape* the owning probe
//! reports, a *range* the recipe declares support for, and a *fact* the client
//! renders. This module owns all three, because a comparison nobody publishes and
//! a publication nobody compares are each half a decision.
//!
//! # The range table
//!
//! Every Agent whose version this Host probes has exactly one row, so "may this
//! version be driven?" is answered for every probed Agent rather than only for
//! the ones an author remembered. A row declares:
//!
//! * `schemes` — the version domains the Agent's own probe reports, as
//!   [`SCHEME_SEMVER`] or [`SCHEME_DATE_HASH`]. A value outside every declared
//!   domain is not comparable, and a version this Hub cannot compare is not one
//!   it drives.
//! * `supported` — the ranges that admit, in `semver` range syntax. An empty list
//!   declares no bound, which is what the shipped rows carry: a vendor minimum is
//!   added together with the evidence for it, never ahead of it.
//! * `unsupported` — the ranges refused, each with the reason code a caller
//!   renders.
//!
//! A version is reported by this Host's own probe, so an Agent that reported
//! nothing is [`VersionStatus::Absent`] — a fact about the probe, not a support
//! claim. An Agent that reported a version the table does not cover is
//! [`VersionStatus::Undeclared`]: undeclared support is refused like any other
//! unsupported version, because a version nobody declared support for is not one
//! this Hub may drive.
//!
//! # Supplying a table
//!
//! A caller may pass `versionRanges` in its params to replace the shipped rows
//! for one run, exactly as it already supplies `discoveryCandidates`,
//! `versionProbes` and `packageMetadata`. That is what lets a declared bound be
//! exercised end to end without writing an unverified vendor minimum into the
//! shipped table.
//!
//! # The upgrade path
//!
//! A refused version is only a refusal if the caller learns how to leave it.
//! [`upgrade_prompt`] publishes that: the reason, the version that was read, and
//! the update command the owning recipe already runs for that Agent. The command
//! is the channel's own `update_argv`, so the prompt and the operation that fixes
//! it cannot drift apart.

use super::contract::InstallChannel;
use super::version;
use super::version_check;
use anyhow::{Result, anyhow, ensure};
use semver::VersionReq;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::OnceLock;

/// A version the Agent's probe reports as a semantic version.
pub const SCHEME_SEMVER: &str = "semver";

/// A version the Agent's probe reports as a `YYYY.MM.DD-<hash>` build stamp.
pub const SCHEME_DATE_HASH: &str = "date-hash";

/// The Agent reported no version at all.
pub const ABSENT: &str = "agent_version_absent";

/// The reported version belongs to no declared version domain.
pub const NOT_COMPARABLE: &str = "agent_version_not_comparable";

/// The reported version satisfies none of the ranges that admit.
pub const OUTSIDE_SUPPORTED: &str = "agent_version_outside_supported";

/// The table declares no row for this Agent.
pub const UNDECLARED: &str = "agent_version_range_undeclared";

const RANGES_DOCUMENT: &str =
    include_str!("../../../resources/agent-hub/agent-version-ranges.json");

/// Whether this Host may drive the version it read.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VersionStatus {
    /// The version is comparable and every declared bound admits it.
    Supported,
    /// The version is comparable but a declared bound refuses it.
    Unsupported,
    /// The Agent reported no version: nothing was compared.
    Absent,
    /// The version is comparable and the table declares no row for this Agent.
    Undeclared,
}

impl VersionStatus {
    /// The stable wire name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Supported => "supported",
            Self::Unsupported => "unsupported",
            Self::Absent => "absent",
            Self::Undeclared => "undeclared",
        }
    }
}

/// One Agent's declared version domains and ranges.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentVersionRanges {
    pub agent_id: String,
    pub schemes: Vec<String>,
    #[serde(default)]
    pub supported: Vec<String>,
    #[serde(default)]
    pub unsupported: Vec<RefusedVersionRange>,
}

/// One range the table refuses, with the reason a caller renders.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefusedVersionRange {
    pub range: String,
    pub reason: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct VersionRangeDocument {
    schema_version: String,
    ranges: Vec<AgentVersionRanges>,
}

/// The verdict for one Agent version, with the reason behind it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VersionSupport {
    pub status: VersionStatus,
    /// The Agent's declared version domains, empty for an undeclared Agent.
    pub schemes: Vec<String>,
    /// The version that was read, empty when the Agent reported none.
    pub version: String,
    /// The range that decided, when one did.
    pub matched_range: String,
    /// The stable reason code, empty when the version is supported.
    pub reason_code: String,
}

impl VersionSupport {
    /// Whether the Hub may drive the version this verdict is about.
    pub fn allows_execution(&self) -> bool {
        self.status == VersionStatus::Supported
    }

    /// Whether this verdict tells a caller to leave the version it has.
    pub fn needs_upgrade(&self) -> bool {
        matches!(
            self.status,
            VersionStatus::Unsupported | VersionStatus::Undeclared
        )
    }

    /// Whether running the Agent's own channel update is the way out of this
    /// verdict.
    ///
    /// It is true exactly for a refused version this Host could read: the channel
    /// update is the operation that moves such an Agent to a version the Hub
    /// drives. An Agent the table does not cover is not fixed by updating the
    /// Agent — the declaration is missing, which no channel operation changes —
    /// so it offers no such path.
    pub fn channel_update_is_the_way_out(&self) -> bool {
        self.status == VersionStatus::Unsupported
    }

    /// The projection the Agent Hub card and plan publish.
    pub fn projection(&self) -> Value {
        json!({
            "status": self.status.as_str(),
            "version": self.version,
            "schemes": self.schemes,
            "matchedRange": self.matched_range,
            "reasonCode": self.reason_code,
            "allowsExecution": self.allows_execution(),
            "needsUpgrade": self.needs_upgrade(),
        })
    }
}

/// Every Agent's declared version domains and ranges.
///
/// The shipped table is parsed once. A caller that supplies `versionRanges`
/// replaces it for that call, so a bound can be exercised without a vendor fact
/// this repository cannot state.
pub fn ranges(params: &Value) -> Result<Vec<AgentVersionRanges>> {
    let Some(supplied) = params.get("versionRanges") else {
        return Ok(shipped_ranges()?.clone());
    };
    let document: Vec<AgentVersionRanges> = serde_json::from_value(supplied.clone())
        .map_err(|error| anyhow!("agent version ranges are invalid: {error}"))?;
    validate(&document)?;
    Ok(document)
}

/// The table's own row for one Agent, when it declares one.
pub fn ranges_for(params: &Value, agent_id: &str) -> Result<Option<AgentVersionRanges>> {
    Ok(ranges(params)?
        .into_iter()
        .find(|row| row.agent_id == agent_id))
}

/// The verdict for one Agent version, read against the declared table.
///
/// It never fails on a version it cannot compare: an uncomparable version is a
/// refusal with a reason, which is the answer a caller can render.
pub fn support_for(params: &Value, agent_id: &str, installed: &str) -> VersionSupport {
    let read = installed.trim();
    let table = ranges(params).unwrap_or_default();
    let row = table.iter().find(|row| row.agent_id == agent_id);
    let schemes = row.map(|row| row.schemes.clone()).unwrap_or_default();
    if read.is_empty() {
        // Nothing was read, so nothing is refused: an absent Agent is not a
        // version this Hub decided against.
        return VersionSupport {
            status: VersionStatus::Absent,
            schemes,
            version: String::new(),
            matched_range: String::new(),
            reason_code: String::new(),
        };
    }
    let Some(row) = row else {
        return VersionSupport {
            status: VersionStatus::Undeclared,
            schemes,
            version: read.to_owned(),
            matched_range: String::new(),
            reason_code: UNDECLARED.to_owned(),
        };
    };
    let Some(comparable) = comparable(&row.schemes, read) else {
        return VersionSupport {
            status: VersionStatus::Unsupported,
            schemes: row.schemes.clone(),
            version: read.to_owned(),
            matched_range: String::new(),
            reason_code: NOT_COMPARABLE.to_owned(),
        };
    };
    if let Some(refused) = refused_range(&row.unsupported, &comparable) {
        return VersionSupport {
            status: VersionStatus::Unsupported,
            schemes: row.schemes.clone(),
            version: read.to_owned(),
            matched_range: refused.range.clone(),
            reason_code: refused.reason.clone(),
        };
    }
    if !row.supported.is_empty() {
        let matched = row
            .supported
            .iter()
            .find(|range| matches_range(range, &comparable));
        let Some(matched) = matched else {
            return VersionSupport {
                status: VersionStatus::Unsupported,
                schemes: row.schemes.clone(),
                version: read.to_owned(),
                matched_range: row.supported.join(", "),
                reason_code: OUTSIDE_SUPPORTED.to_owned(),
            };
        };
        return VersionSupport {
            status: VersionStatus::Supported,
            schemes: row.schemes.clone(),
            version: read.to_owned(),
            matched_range: matched.clone(),
            reason_code: String::new(),
        };
    }
    VersionSupport {
        status: VersionStatus::Supported,
        schemes: row.schemes.clone(),
        version: read.to_owned(),
        matched_range: String::new(),
        reason_code: String::new(),
    }
}

/// The verdict one observation recorded, read back from a cache entry.
///
/// A record whose status is not one this module publishes is refused rather than
/// read as support: a corrupted record is not evidence that this Host may drive
/// the Agent.
pub fn recorded(
    version: &str,
    schemes: &[String],
    status: &str,
    reason_code: &str,
    matched_range: &str,
) -> VersionSupport {
    let status = match status {
        "supported" => VersionStatus::Supported,
        "unsupported" => VersionStatus::Unsupported,
        "undeclared" => VersionStatus::Undeclared,
        "absent" => VersionStatus::Absent,
        _ => VersionStatus::Unsupported,
    };
    let reason_code = if status == VersionStatus::Supported {
        String::new()
    } else if reason_code.trim().is_empty() {
        NOT_COMPARABLE.to_owned()
    } else {
        reason_code.to_owned()
    };
    VersionSupport {
        status,
        schemes: schemes.to_vec(),
        version: version.to_owned(),
        matched_range: matched_range.to_owned(),
        reason_code,
    }
}

/// The upgrade path out of a refused version, or `None` when there is none.
///
/// It is published only when running the Agent's own channel update is the way
/// out ([`VersionSupport::channel_update_is_the_way_out`]). The command is that
/// channel's own update argv — the same argv the Hub's `update` operation runs —
/// so the prompt names an operation this Hub really performs rather than an
/// instruction of its own invention.
pub fn upgrade_prompt(channel: Option<&InstallChannel>, support: &VersionSupport) -> Option<Value> {
    if !support.channel_update_is_the_way_out() {
        return None;
    }
    let (channel_id, channel_kind, command) = match channel {
        Some(channel) => (
            channel.id.clone(),
            channel.kind.clone(),
            prompt_command(channel).join(" "),
        ),
        None => (String::new(), String::new(), String::new()),
    };
    Some(json!({
        "reasonCode": support.reason_code,
        "installedVersion": support.version,
        "channelId": channel_id,
        "channelKind": channel_kind,
        "command": command,
        "operation": "update",
    }))
}

fn prompt_command(channel: &InstallChannel) -> &[String] {
    if channel.update_argv.is_empty() {
        &channel.install_argv
    } else {
        &channel.update_argv
    }
}

fn comparable(schemes: &[String], read: &str) -> Option<semver::Version> {
    let declares = |scheme: &str| schemes.iter().any(|declared| declared == scheme);
    if version_check::is_cursor_date_hash_version(read) {
        // A build stamp belongs to the domain that reports build stamps, even
        // though the loose reader can also read one as a pre-release semver.
        return declares(SCHEME_DATE_HASH)
            .then(|| version::parse_comparable(read))
            .flatten();
    }
    if declares(SCHEME_SEMVER) {
        return version::parse_comparable(read);
    }
    None
}

fn refused_range<'a>(
    refused: &'a [RefusedVersionRange],
    version: &semver::Version,
) -> Option<&'a RefusedVersionRange> {
    refused
        .iter()
        .find(|entry| matches_range(&entry.range, version))
}

fn matches_range(range: &str, version: &semver::Version) -> bool {
    VersionReq::parse(range.trim()).is_ok_and(|requirement| requirement.matches(version))
}

fn shipped_ranges() -> Result<&'static Vec<AgentVersionRanges>> {
    static RANGES: OnceLock<Vec<AgentVersionRanges>> = OnceLock::new();
    if let Some(ranges) = RANGES.get() {
        return Ok(ranges);
    }
    let document: VersionRangeDocument = serde_json::from_str(RANGES_DOCUMENT)
        .map_err(|error| anyhow!("agent version ranges are invalid: {error}"))?;
    ensure!(
        document
            .schema_version
            .starts_with("v0.0.1:client-agent-version-ranges-"),
        "agent version ranges schema is unsupported"
    );
    validate(&document.ranges)?;
    Ok(RANGES.get_or_init(|| document.ranges))
}

fn validate(ranges: &[AgentVersionRanges]) -> Result<()> {
    for row in ranges {
        ensure!(
            !row.agent_id.trim().is_empty(),
            "agent version range row is missing its agent"
        );
        ensure!(
            !row.schemes.is_empty(),
            "agent version range row declares no version domain: {}",
            row.agent_id
        );
        for scheme in &row.schemes {
            ensure!(
                matches!(scheme.as_str(), SCHEME_SEMVER | SCHEME_DATE_HASH),
                "agent version range row declares an unknown version domain: {scheme}"
            );
        }
        for range in row.supported.iter() {
            ensure!(
                VersionReq::parse(range.trim()).is_ok(),
                "agent version range is invalid: {range}"
            );
        }
        for refused in &row.unsupported {
            ensure!(
                VersionReq::parse(refused.range.trim()).is_ok(),
                "agent version range is invalid: {}",
                refused.range
            );
            ensure!(
                !refused.reason.trim().is_empty(),
                "a refused agent version range states no reason: {}",
                refused.range
            );
        }
    }
    Ok(())
}
