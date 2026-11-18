# licoup-agent-runtime

Extracted Agent Runtime crate.

## Migration Target

This crate will own the agent adapter layer and runtime host, extracted from
`licoup/src/platform/` agent-related modules and `licoup/src/domain/`
agent-related modules.

## Responsibilities

- Agent Conversation Host (persistent, GUI-independent)
- 13-agent adapter drivers (ACP, app-server, CLI, RPC variants)
- Transport & process supervision (PTY, subprocess lifecycle)
- Interaction routing (park-and-wait, approval broker)
- Capability registry and manifest intersection
- Turn settlement arbitration

## Does NOT Own

- Conversation domain state (→ `licoup-conversation`)
- Crypto/endpoint identity (→ `licoarc-client`)
- FFI entry points (→ `licoup`)
- Platform-specific OS APIs (→ `licoup-platform-bridges`)

## Migration Source

```
crates/licoup/src/platform/acp_driver_runtime/
crates/licoup/src/platform/codex_app_server/
crates/licoup/src/platform/claude_code_driver/
crates/licoup/src/platform/antigravity_driver/
crates/licoup/src/platform/hermes_driver/
crates/licoup/src/platform/kilo_code_driver/
crates/licoup/src/platform/pi_driver/
crates/licoup/src/platform/lico_agent_driver/
crates/licoup/src/platform/opencode_driver.rs
crates/licoup/src/platform/copilot_driver.rs
crates/licoup/src/platform/kimi_code_driver.rs
crates/licoup/src/platform/conversation_lane.rs
crates/licoup/src/platform/conversation_runtime.rs
crates/licoup/src/platform/runtime_adapters/
crates/licoup/src/domain/workflow_runtime/
crates/licoup-workflow-store/src/
crates/licoup/src/domain/agent_hub/
```
