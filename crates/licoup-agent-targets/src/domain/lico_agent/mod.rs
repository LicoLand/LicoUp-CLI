//! Lico-owned agent core: loop, tools, profiles, Gateway transport.

mod agent;
mod events;
mod loop_;
mod profiles;
mod tools;
mod transport;

pub use agent::{Agent, AgentConfig, AgentProfileKind};
pub use events::AgentEvent;
pub use profiles::{base_system_prompt, plan_system_prompt};
pub use tools::{Tool, ToolError, ToolRegistry, read_tool, write_plan_tool};
pub use transport::{GatewayChatTransport, LlmTransport, TransportError};
