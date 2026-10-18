# Lico Agent

English (normative) · [简体中文](lico-agent.zh-CN.md)

Authority: `licoup-agent-targets`' `domain/lico_agent/`,
`licoup-agent-lico-agent`'s `driver/`, the shared `platform/process_sandbox/`
primitive, and the `lico-agent` sibling binary. Update this projection when
those implementations or their verification change.

Lico Agent is a **first-party** LicoUp-owned runtime. It appears in the agent
list as one ordinary option (`lico-agent`), peer to third-party adapters such as
Pi or Codex. It is not a special Conversation, role, or orchestration owner.

## Capabilities

- Stdio JSONL RPC (`lico-agent-rpc-stdio-jsonl`): `get_state`, `prompt`,
  `steer`, `abort`.
- Models only through the local loopback [LLM Gateway](llm-gateway.md).
  Non-loopback base URLs fail closed.
- Profiles: `base` (read + optional future write tools) and `plan`
  (inheritance mode: `read` + `write_plan` only).
- Plan mode spawns under macOS seatbelt capability
  `platform-lico-agent-plan-isolated-v1`: filesystem write is limited to one
  absolute literal plan file
  (`{portable}/client-state/plans/active-plan.md` by default); network outbound
  is limited to the Gateway port. Non-macOS Plan mode fails closed.
- Session/transcript state is parent-owned under
  `{portable}/client-state/lico-agent/`. The child process does not persist
  its own session store.

## Relation to canonical Conversations

The canonical Conversation backend may admit Lico Agent through an ordinary
Agent Membership, exactly like any other runnable integration. A user-defined
Conversation Role may include that Membership, and an explicitly started
Adaptive Flywheel may select it from the Role's ordered candidate pool. There
is no built-in main-agent slot, automatic group turn-taking, or dedicated Lico
group identity. The current delivery does not claim a desktop group or
Flywheel editor.
