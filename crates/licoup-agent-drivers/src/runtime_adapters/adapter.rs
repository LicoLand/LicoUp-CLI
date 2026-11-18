use super::params::text_param;
use serde_json::Value;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeAdapter {
    Antigravity,
    ClaudeCode,
    Codex,
    Copilot,
    Cursor,
    Hermes,
    KiloCode,
    KimiCode,
    OpenClaw,
    OpenCode,
    Pi,
    LicoAgent,
    DeepSeekHarness,
}

/// Native delivery channels an agent itself ships, as opposed to a
/// LicoUp-installed adapter plugin or LicoUp-owned gateway. Detection of
/// `desktop` and `cli` is real filesystem detection;
/// `acp`, `rpc`, `gateway`, `local-server`, and `web-server` are capabilities
/// of the CLI/runtime itself, so their detection follows the CLI result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeCapabilityKind {
    Desktop,
    Cli,
    Acp,
    Rpc,
    AppServer,
    Gateway,
    LocalServer,
    WebServer,
    TuiGateway,
}

impl NativeCapabilityKind {
    pub fn wire_name(self) -> &'static str {
        match self {
            Self::Desktop => "desktop",
            Self::Cli => "cli",
            Self::Acp => "acp",
            Self::Rpc => "rpc",
            Self::AppServer => "app-server",
            Self::Gateway => "gateway",
            Self::LocalServer => "local-server",
            Self::WebServer => "web-server",
            Self::TuiGateway => "tui-gateway",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "desktop" => Some(Self::Desktop),
            "cli" => Some(Self::Cli),
            "acp" => Some(Self::Acp),
            "rpc" => Some(Self::Rpc),
            "app-server" => Some(Self::AppServer),
            "gateway" => Some(Self::Gateway),
            "local-server" => Some(Self::LocalServer),
            "web-server" => Some(Self::WebServer),
            "tui-gateway" => Some(Self::TuiGateway),
            _ => None,
        }
    }
}

/// Ability facts an Agent may declare. An ability is not a delivery channel:
/// it carries no transport or lane semantics, so it is named by its own kind
/// value instead of widening `NativeCapabilityKind`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeAbilityKind {
    /// The Agent's own lane accepts image input.
    ImageInput,
    /// A desktop surface of the Agent is present on this host. Presence is a
    /// declaration, not a claim that LicoUp can drive that surface.
    RealInterface,
}

impl NativeAbilityKind {
    pub fn wire_name(self) -> &'static str {
        match self {
            Self::ImageInput => "image-input",
            Self::RealInterface => "real-interface",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "image-input" => Some(Self::ImageInput),
            "real-interface" => Some(Self::RealInterface),
            _ => None,
        }
    }
}

/// Conversation-driver states the Membership Profile projection already
/// reported as bare strings. They keep their exact spelling and become
/// ordinary derived facts owned by the readiness entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeDriverStateKind {
    ConversationDriverSupported,
    ConversationDriverReady,
}

impl NativeDriverStateKind {
    pub fn wire_name(self) -> &'static str {
        match self {
            Self::ConversationDriverSupported => "conversationDriver:supported",
            Self::ConversationDriverReady => "conversationDriver:ready",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "conversationDriver:supported" => Some(Self::ConversationDriverSupported),
            "conversationDriver:ready" => Some(Self::ConversationDriverReady),
            _ => None,
        }
    }
}

/// The one closed set of capability fact names. A channel, an ability and a
/// conversation-driver state are the same kind of thing to a consumer: a named
/// fact with exactly one authoritative owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CapabilityFactName {
    Channel(NativeCapabilityKind),
    Ability(NativeAbilityKind),
    DriverState(NativeDriverStateKind),
}

impl CapabilityFactName {
    pub fn wire_name(self) -> &'static str {
        match self {
            Self::Channel(kind) => kind.wire_name(),
            Self::Ability(kind) => kind.wire_name(),
            Self::DriverState(kind) => kind.wire_name(),
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        NativeCapabilityKind::parse(value)
            .map(Self::Channel)
            .or_else(|| NativeAbilityKind::parse(value).map(Self::Ability))
            .or_else(|| NativeDriverStateKind::parse(value).map(Self::DriverState))
    }
}

/// Projection state of one capability fact. An unreadable owner and a
/// definitely absent fact are different answers and never collapse into one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CapabilityFactState {
    /// The owner affirms the fact.
    Declared,
    /// The owner was read and the fact is absent.
    NotDeclared,
    /// The owner could not be read, or the Agent is not in its inventory.
    Unknown,
}

impl CapabilityFactState {
    pub fn wire_name(self) -> &'static str {
        match self {
            Self::Declared => "declared",
            Self::NotDeclared => "not-declared",
            Self::Unknown => "unknown",
        }
    }

    pub fn from_declaration(declared: bool) -> Self {
        if declared {
            Self::Declared
        } else {
            Self::NotDeclared
        }
    }
}

/// One projected capability fact. Exactly these three allowlisted values cross
/// the projection boundary: the wire name, its state and the logical owner that
/// produced it. A local path, a process fact or a runtime value never does.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapabilityFact {
    pub name: &'static str,
    pub state: CapabilityFactState,
    pub source: &'static str,
}

pub fn adapter_for_agent_public(agent_id: &str) -> Option<RuntimeAdapter> {
    adapter_for_agent(agent_id)
}

pub fn text_param_public(params: &Value, keys: &[&str]) -> Option<String> {
    text_param(params, keys)
}

pub fn adapter_for_agent(agent_id: &str) -> Option<RuntimeAdapter> {
    match agent_id {
        "antigravity" => Some(RuntimeAdapter::Antigravity),
        "claude" | "claude-code" => Some(RuntimeAdapter::ClaudeCode),
        "codex" => Some(RuntimeAdapter::Codex),
        "copilot" | "github-copilot" => Some(RuntimeAdapter::Copilot),
        "cursor" | "cursor-agent" => Some(RuntimeAdapter::Cursor),
        "hermes" | "hermes-agent" => Some(RuntimeAdapter::Hermes),
        "kilo" | "kilocode" | "kilo-code" => Some(RuntimeAdapter::KiloCode),
        "kimi-code" | "kimicode" => Some(RuntimeAdapter::KimiCode),
        "openclaw" => Some(RuntimeAdapter::OpenClaw),
        "opencode" => Some(RuntimeAdapter::OpenCode),
        "pi" | "pi-agent" | "pi-coding-agent" => Some(RuntimeAdapter::Pi),
        "lico-agent" | "lico" => Some(RuntimeAdapter::LicoAgent),
        "deepseek-harness" | "dsh" => Some(RuntimeAdapter::DeepSeekHarness),
        _ => None,
    }
}

impl RuntimeAdapter {
    pub fn id(self) -> &'static str {
        match self {
            Self::Antigravity => "antigravity",
            Self::ClaudeCode => "claude-code",
            Self::Codex => "codex",
            Self::Copilot => "copilot",
            Self::Cursor => "cursor",
            Self::Hermes => "hermes",
            Self::KiloCode => "kilo-code",
            Self::KimiCode => "kimi-code",
            Self::OpenClaw => "openclaw",
            Self::OpenCode => "opencode",
            Self::Pi => "pi",
            Self::LicoAgent => "lico-agent",
            Self::DeepSeekHarness => "deepseek-harness",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Antigravity => "Antigravity CLI",
            Self::ClaudeCode => "Claude Code CLI",
            Self::Codex => "Codex CLI",
            Self::Copilot => "GitHub Copilot CLI",
            Self::Cursor => "Cursor CLI",
            Self::Hermes => "Hermes Agent CLI",
            Self::KiloCode => "Kilo Code CLI",
            Self::KimiCode => "Kimi Code CLI",
            Self::OpenClaw => "OpenClaw CLI",
            Self::OpenCode => "OpenCode CLI",
            Self::Pi => "Pi Agent CLI",
            Self::LicoAgent => "Lico Agent CLI",
            Self::DeepSeekHarness => "DeepSeek Harness",
        }
    }

    /// This Agent's driver identity, as the Agent's own engine reports it.
    ///
    /// The identity is the Agent's own fact, so it is read through the
    /// registration the composition supplies rather than restated here. A host
    /// that composed no driver for this Agent answers with the empty identity,
    /// which is what the fail-closed dispatch path already refuses on.
    pub fn driver_id(self) -> &'static str {
        super::port::registration_for_adapter(self)
            .map(|registration| registration.driver_id)
            .unwrap_or("")
    }

    /// The protocol name this Agent's driver speaks on the ordinary lane.
    ///
    /// Read through the same registration, for the same reason.
    pub fn runtime_protocol(self) -> &'static str {
        super::port::registration_for_adapter(self)
            .map(|registration| registration.runtime_protocol)
            .unwrap_or("")
    }

    pub fn default_binary(self) -> &'static str {
        match self {
            Self::Antigravity => "agy",
            Self::ClaudeCode => "claude",
            Self::Codex => "codex",
            Self::Copilot => "copilot",
            Self::Cursor => "cursor-agent",
            Self::Hermes => "hermes",
            Self::KiloCode => "kilo",
            Self::KimiCode => "kimi",
            Self::OpenClaw => "openclaw",
            Self::OpenCode => "opencode",
            Self::Pi => "pi",
            Self::LicoAgent => "lico-agent",
            Self::DeepSeekHarness => "dsh",
        }
    }

    /// The LicoUp-managed adapter plugin this agent supports, if any. Only
    /// managed plugins with real install management may be listed here.
    pub fn managed_adapter_plugin_id(self) -> Option<&'static str> {
        match self {
            Self::Antigravity => Some("acp-bridge"),
            _ => None,
        }
    }
}
