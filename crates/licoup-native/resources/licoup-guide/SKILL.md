---
name: licoup-guide
description: Use LicoUp conversations, active agent Memberships, delegation, and workflows when a user asks an agent to operate LicoUp; excludes developing LicoUp, general replies, and independent planning frameworks.
---

# LicoUp Guide

Complete the user's requested LicoUp operation through the available interfaces.
For a direct reply, answer directly. Read the current tool schemas before an
operation; do not infer an unavailable capability from this guide.

| Operation | Route |
| --- | --- |
| Find an available Agent | `lico_subagents_list`, then `lico_subagent_probe` for the selected target |
| Inspect the designated Assistant's current Membership Profiles | local `licoup subagents execute` with `name: lico_assistant_profiles` |
| Delegate bounded work or continue an existing delegation | `lico_subagent_delegate` or `lico_subagent_continue`, using the exact active `conversationId + membershipId` |
| Run coordinated work as the designated Assistant | Prepare the needed Graph and bindings, then use local `licoup subagents execute` with `name: lico_assistant_workflow_execute` within existing authorization |
| Observe a workflow | local `licoup subagents execute` with `name: lico_assistant_workflow_inspect`; reuse its run identity |
| Stop work at the user's request | `lico_subagent_cancel` or local CLI operation `lico_assistant_workflow_cancel` for the exact target or run |

Keep the designated Assistant and current Conversation. Never self-call,
cross a Conversation boundary, invent Memberships, or substitute native session
identifiers for LicoUp identities. Pass only the task context a target needs.
Report completed work and relevant failures through the owning Conversation.
An elapsed observation window does not prove failure or authorize cancellation.

A tool or Skill grants no new authority. Preserve the user's intent, host
permissions, protected-key authentication, and production, publication,
private-data transfer, and irreversible-action boundaries. Native sessions,
paths, endpoints, credentials, prompts, Agent output, and backend runtime data
remain private to LicoUp. Share only the minimum authorized result.

Use `licoup commands` and `licoup subagents catalog` to discover local syntax and schemas. Local Subagents calls use `--stdin-json true` with `{name, arguments, caller}`; bind `caller.providerId`, `caller.conversationId`, and `caller.membershipId` to the current native context. MCP exposes only list, probe, delegate, continue, and cancel; Assistant workflows and full Conversation commands use the local CLI.
