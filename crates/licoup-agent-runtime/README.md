# licoup-agent-runtime

Extracted Agent Runtime crate.

## Migration Target

This crate will own the agent adapter layer and runtime host, extracted from
`licoup-native/src/platform/` agent-related modules and `licoup-native/src/domain/`
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
- Crypto/endpoint identity (→ `licoup-endpoint-core`)
- FFI entry points (→ `licoup-native`)
- Platform-specific OS APIs (→ `licoup-platform-bridges`)

## Migration Source

```
crates/licoup-native/src/platform/acp_driver_runtime/
crates/licoup-native/src/platform/codex_app_server/
crates/licoup-native/src/platform/claude_code_driver/
crates/licoup-native/src/platform/antigravity_driver/
crates/licoup-native/src/platform/hermes_driver/
crates/licoup-native/src/platform/kilo_code_driver/
crates/licoup-native/src/platform/pi_driver/
crates/licoup-native/src/platform/lico_agent_driver/
crates/licoup-native/src/platform/opencode_driver.rs
crates/licoup-native/src/platform/copilot_driver.rs
crates/licoup-native/src/platform/kimi_code_driver.rs
crates/licoup-native/src/platform/conversation_lane.rs
crates/licoup-native/src/platform/conversation_runtime.rs
crates/licoup-native/src/platform/runtime_adapters/
crates/licoup-native/src/domain/workflow_runtime/
crates/licoup-workflow-store/src/
crates/licoup-native/src/domain/agent_hub/
```
