//! Deterministic channel ranking for one Agent against one capability snapshot.
//!
//! Two orders are deliberate and distinct. `select_channel` keeps the recipe's
//! own preference order (official recommendation and verification first, then
//! the recipe's priority). `available_channels` is what the Hub *presents*, and
//! it puts a vendor binary first so a user is offered the channel that needs no
//! developer toolchain before one that does.

use super::contract::{
    AgentRecipe, InstallChannel, PlatformInstallCapabilities, channel_class_rank,
    channel_requires_developer_toolchain,
};
use anyhow::{Result, anyhow};

#[derive(Clone, Debug)]
pub struct SelectedChannel<'a> {
    pub channel: &'a InstallChannel,
}

pub fn select_channel<'a>(
    agent: &'a AgentRecipe,
    capabilities: &PlatformInstallCapabilities,
) -> Result<SelectedChannel<'a>> {
    if !super::capabilities::desktop_os(&capabilities.os) {
        return Err(anyhow!("unsupported_platform"));
    }
    if agent
        .unsupported
        .iter()
        .any(|item| item.oses.iter().any(|os| os == &capabilities.os))
    {
        return Err(anyhow!("unsupported_platform"));
    }
    let mut candidates = agent
        .channels
        .iter()
        .filter(|channel| channel_matches(channel, capabilities))
        .collect::<Vec<_>>();
    candidates.sort_by_key(|channel| rank_key(channel));
    let channel = candidates
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("channel_unavailable"))?;
    Ok(SelectedChannel { channel })
}

/// The channels the Hub presents for one Agent, in presentation order: a
/// vendor binary first, then an operating system package manager, then a
/// channel that needs a developer toolchain. Channels whose manager is not on
/// this machine are not offered at all.
pub fn available_channels<'a>(
    agent: &'a AgentRecipe,
    capabilities: &PlatformInstallCapabilities,
) -> Vec<&'a InstallChannel> {
    let mut candidates = agent
        .channels
        .iter()
        .filter(|channel| channel_matches(channel, capabilities))
        .collect::<Vec<_>>();
    candidates.sort_by_key(|channel| presentation_key(channel));
    candidates
}

/// Whether this recipe has no channel that avoids a developer toolchain.
///
/// This is a property of the recipe, not of the machine: a recipe whose only
/// selectable channels are language-runtime channels is toolchain-only even on
/// a host that happens to have that runtime installed, so the Hub never
/// presents it as an ordinary first-launch recommendation.
pub fn toolchain_only(agent: &AgentRecipe) -> bool {
    agent
        .channels
        .iter()
        .filter(|channel| channel.selectable)
        .all(channel_requires_developer_toolchain)
}

/// Whether first launch may recommend this Agent: a channel must be installable
/// on this machine, and the recipe must not be toolchain-only.
pub fn first_launch_eligible(
    agent: &AgentRecipe,
    capabilities: &PlatformInstallCapabilities,
) -> bool {
    !toolchain_only(agent) && !available_channels(agent, capabilities).is_empty()
}

pub fn channel_matches(
    channel: &InstallChannel,
    capabilities: &PlatformInstallCapabilities,
) -> bool {
    if !channel.selectable {
        return false;
    }
    if !channel.oses.iter().any(|os| os == &capabilities.os) {
        return false;
    }
    if !channel.architectures.is_empty()
        && !channel
            .architectures
            .iter()
            .any(|arch| arch == &capabilities.architecture)
    {
        return false;
    }
    if channel.elevation != "none" {
        return false;
    }
    match channel.requires_manager.as_str() {
        "none" | "" => true,
        manager => capabilities.managers.iter().any(|item| item == manager),
    }
}

fn rank_key(channel: &InstallChannel) -> (i32, i32, String) {
    let preferred = match (channel.official_recommended, channel.licoup_verified) {
        (true, true) => 0,
        (false, true) => 1,
        (true, false) => 2,
        (false, false) => 3,
    };
    (preferred, channel.priority, channel.id.clone())
}

/// The Hub's presentation order: class first, then the recipe's own preference.
fn presentation_key(channel: &InstallChannel) -> (i32, i32, i32, String) {
    let (preferred, priority, id) = rank_key(channel);
    (channel_class_rank(channel), preferred, priority, id)
}

pub fn channel_by_id<'a>(agent: &'a AgentRecipe, channel_id: &str) -> Result<&'a InstallChannel> {
    agent
        .channels
        .iter()
        .find(|channel| channel.id == channel_id)
        .ok_or_else(|| anyhow!("channel_unavailable"))
}

/// Recipe-preferred channel for Hub chips when no planned/detected channel exists.
pub fn preferred_channel(agent: &AgentRecipe) -> Option<&InstallChannel> {
    let mut candidates = agent
        .channels
        .iter()
        .filter(|channel| channel.selectable)
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        candidates = agent.channels.iter().collect();
    }
    candidates.sort_by_key(|channel| rank_key(channel));
    candidates.into_iter().next()
}

/// Chip source: detected/owned channel, else the planned OS channel, else recipe preferred.
pub fn chip_channel_kind(
    agent: &AgentRecipe,
    selected: Option<&SelectedChannel<'_>>,
    record: Option<&super::contract::InstallOwnership>,
) -> String {
    if let Some(kind) = record
        .map(|item| item.channel_kind.trim())
        .filter(|kind| !kind.is_empty())
    {
        return kind.to_string();
    }
    if let Some(selected) = selected {
        return selected.channel.kind.clone();
    }
    preferred_channel(agent)
        .map(|channel| channel.kind.clone())
        .unwrap_or_default()
}
