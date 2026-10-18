# Native CLI

| Reference | Owner |
| --- | --- |
| Simplified Chinese | [NATIVE-CLI.zh-CN.md](NATIVE-CLI.zh-CN.md) |
| Architecture | [Architecture index](README.md) |
| Native frames and client transport | [Client-native interaction](CLIENT-NATIVE-INTERACTION.md) |
| Canonical Conversation | [Conversation domain](CONVERSATION-DOMAIN.md) |
| Subagents MCP adapter | [Subagent MCP](../protocols/subagent-mcp.md) |

This document owns the local native CLI entry and its discovery behavior.
`licoup` is the primary local facade for the native core. It admits commands,
starts or connects to the native host, and presents results without Flutter.
Flutter and the independent MCP process are clients of that native boundary.
The CLI does not replace direct approval, membership admission, platform
authentication, or protected-effect checks in the core.

## Discover and invoke

```sh
licoup help
licoup commands
```

Help and the `licoup.cli-catalog/v1` JSON catalog come from the same command
registry used for admission. The catalog includes exact command paths,
positional argument kinds, option kinds, required and repeatable options,
option constraints, and help text. Its `rpc.methods` list comes from the
generated native protocol. There is no separately maintained command list.

Use a listed command directly. Body-bearing commands accept `--stdin-json true`
to read one JSON object from private standard input. Keep user content and
credentials out of shell arguments and shell history.

```sh
licoup conversation execute --stdin-json true < request.json
licoup strategy execute --stdin-json true < request.json
licoup subagents execute --stdin-json true < invocation.json
```

`conversation execute` and `strategy execute` use the durable native host by
default, including work that continues after the invoking CLI exits. Their
single-result JSON bodies preserve the native operation result. The
`conversation execute --require-running-host` option connects only to an
existing host and fails without creating local state when none is available.
Typed `conversation list` and `conversation get` use the same host and retain
their application envelope. Conversation schemas and actions remain owned by
the Conversation domain.

## Persistent methods and streams

Any method in `licoup commands` → `rpc.methods` is callable through:

```sh
licoup rpc call METHOD --stdin-json true < params.json
```

The body is the method's native `params` object. For the generic `execute`
method it also contains `args`, the registered command argument array.
`rpc call` validates the generated method before opening a host connection.
It prints original `licoup.stdio.v1` NDJSON frames, flushes events as they
arrive, and finishes on the response or terminal frame. The existing per-frame
protocol bound applies; no total-output cap or task deadline is added.
Disconnecting an observer does not send an interrupt to the Agent.

For bidirectional clients, `licoup rpc conversation` connects to the durable
native host and carries the same generated method frames. `licoup rpc stdio`
serves the local command bridge. Persistent Agent dispatch, attach, active-turn
inspection, conversation actions, catalog observation, and strategy execution
remain native methods. The binary owns host lifetime and startup; Flutter is
not required to start or operate it. The internal `rpc conversation-host`
entry is used by this native process supervisor.


The local `agent.conversation.execution` stream inspects one exact dispatch.
Its record ownership, cursor and observation contract is defined by
[Local execution inspection](CONVERSATION-DOMAIN.md#13-local-execution-inspection).

## Independent MCP process

The local [model registry](MODEL-REGISTRY.md) owns public model-directory
refresh and canonical identity. Its commands are not remote MCP operations.

`subagents catalog` exposes the native caller and operation schemas;
`subagents execute` admits a local invocation with `name`, `arguments`, and
`caller`. Caller scope is checked by the native Subagents domain.
`mcp start`, `stop`, `reload`, and `status` manage the independent MCP process.
Development `start` and `reload` accept `--binary` for an explicit MCP
executable. The MCP adapter's narrower remote exposure is owned by its
[protocol document](../protocols/subagent-mcp.md).

## Implementation owners

| Concern | Source |
| --- | --- |
| Admission, help, and command catalog | `crates/licoup-native/src/ffi/commands/mod.rs` |
| Generated RPC method authority | `schemas/conversation_protocol/conversation_protocol.schema.json` |
| CLI request projection | `crates/licoup-native/src/ffi/commands/native_rpc.rs` |
| Native host frames and stream output | `crates/licoup-native/src/platform/conversation_host_client.rs` |
| Native executable and host startup | `crates/licoup-native/src/bin/licoup.rs` and `bin/licoup/conversation_host.rs` |
| Local Subagents and MCP lifecycle commands | `crates/licoup-native/src/ffi/commands/subagents.rs` |
| Local Subagents invocation and caller admission | `crates/licoup-native/src/domain/subagents/local.rs` |

## Custom operation properties

The accepted customization boundary extends the existing native catalog and tool
contracts. Custom properties have a namespace, type, default and capability-version
boundary. They cannot override the actor, authorization, idempotency identity or
canonical state. Unknown or invalid tool input receives a typed admission error;
ordinary Agent text remains original text, even when it resembles JSON. This is a
constraint on future catalog extensions, not a new customization command.
