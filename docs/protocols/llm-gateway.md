# LicoUp Local LLM Gateway

English (normative) · [简体中文](llm-gateway.zh-CN.md)

The LLM Gateway is the **lower layer** of the Gateway Runtime. Authority for
this layer is `crates/licoup-private-gateway/src/model/llm_gateway.rs`,
`crates/licoup-private-gateway/src/http/transport.rs`, and the unified `lico-gateway`
process (`crates/licoup-private-gateway/src/runtime`). The layer
binds only to loopback and routes by the exact client-protocol/requested-model
pair; there is no global active provider. Messaging channels are documented in
[`gateway-runtime.md`](gateway-runtime.md).

## Capabilities

- Codex `/v1/responses` requests can target a Kimi-compatible OpenAI Chat
  Completions upstream, including tool calls, stream events, and bounded
  `previous_response_id` history.
- Claude Code `/v1/messages` requests can route each model independently to an
  Anthropic Messages or OpenAI Chat Completions upstream.
- Kimi, DeepSeek, and Kilo API keys are separate system-keyring items. The
  native host requires macOS owner authorization before exposing key material.
  Inventory responses contain no secret suffix or value.
- Owner authorization (Touch ID whenever usable, with a system password only
  when biometry is unavailable) unlocks
  credentials in the long-lived `licoup-cli` process. A cold Gateway start
  hands the unlocked session to the sidecar over an inherited file descriptor;
  the sidecar never reads the Keychain itself. While a managed Gateway is
  already running, authorize and clear hot-apply the updated lease over a
  private Unix control socket in the Gateway state directory (mode 0600, same
  uid) without restarting the process. A standalone sidecar launch without a
  handoff fd starts disconnected until a hot apply or a later start with
  handoff. The gateway keeps only a zeroizing in-memory lease and drops it on
  exit. The selectable 7/30/60/90/180/365-day period is an upper bound for that
  running process; it never bypasses the next owner authorization.
- Changing the selected period does not revoke a running Gateway. The current
  process keeps its existing lease; the new period applies on the next owner
  authorization that rebuilds the handoff (hot apply) or on the next cold start.
- Codex and Claude Code managed configuration points only to the loopback
  Gateway. Upstream provider API keys are never copied into agent files.
- Authenticated `GET /v1/models` and `GET /models` requests query each provider
  represented in the current in-memory credential lease and return its live
  `/models` response. No authorized provider credential means an empty list;
  an invalid authorized credential returns the upstream failure instead of a
  product-owned fallback catalog.

## Configuration

```json
{
  "schemaVersion": 1,
  "providers": [
    {
      "id": "kimi-chat",
      "baseUrl": "https://provider.example/v1",
      "protocol": "open_ai_chat_completions",
      "credentialProvider": "kimi",
      "credentialStyle": "bearer"
    }
  ],
  "routes": [
    {
      "clientProtocol": "open_ai_responses",
      "requestedModel": "kimi-for-codex",
      "providerId": "kimi-chat",
      "upstreamModel": "provider-model-id"
    }
  ]
}
```

The shipped configuration defines only the fixed Kimi, DeepSeek, and Kilo
provider boundaries. It contains no product-owned model inventory. A model
request uses `{provider}:{upstream-model-id}` (for example
`kimi:kimi-k3`, `deepseek:deepseek-v4-flash`, or
`kilo:anthropic/claude-sonnet-4.5`); the Gateway removes only the first provider
prefix before forwarding the request. Model-list responses preserve each
upstream model object and expose the namespaced id, plus `upstream_id` and
`gateway_provider`, so combined provider catalogs remain unambiguous.
If one authorized provider is unavailable while another succeeds, the healthy
models are returned with `partial: true` and a redacted
`failed_provider_count`; model discovery has one 45-second aggregate deadline.

OpenCode and Pi require explicit custom-provider model entries. Their
agent-config plan/apply commands therefore snapshot the live list from a
running Gateway. When the Gateway is stopped, the snapshot is empty; when it
is running, an upstream catalog or credential failure fails the plan rather
than substituting fixed model names. Codex and Claude Code continue to use the
Gateway endpoint directly without an embedded model list.
The one-click OpenCode/Pi helpers refuse to apply an empty snapshot.

The configuration must be an absolute regular file no larger than 1 MiB. Run
`lico-llm-gateway --config <absolute-path> --check` before starting the
sidecar. It listens on `127.0.0.1:15722` by default. Non-loopback HTTP
endpoints, redirects, unknown providers, unnamespaced dynamic models, and
unknown fields fail closed.

## Credential custody

The macOS custody executable uses the [app-like helper and permanent public CLI alias](../platforms/MACOS-DIRECT-DISTRIBUTION.md#local-custody-validation). Its profile and access-group authorization belong to the helper executable; the Gateway does not acquire Keychain custody.


A normal explicit authorize operation admits one exact operation or bounded batch,
with at most one native authentication. All selected keys share its retained
`LAContext`; an existing valid scoped grant needs no extra prompt. Cancellation,
rejected biometry, and biometric lockout do not trigger password fallback. The
sidecar receives the authorized handoff and does not authenticate again.
Pending authentication waits for the native result or explicit cancellation;
the client does not impose its own response deadline.

Data Protection Keychain calls reuse that context with interaction disabled.
Classic Keychain ACLs use a separate authorization mechanism: the adapter
serializes a temporary prohibition of classic Keychain UI and restores the
previous setting after each effect, including failed effects. A classic ACL
or locked keychain that cannot be accessed silently returns
`secure_mesh_keychain_classic_access_requires_user_action`. The batch does not publish a
partial handoff or change the denied item's ACL. Authorization does not rewrite
each credential. This error alone does not distinguish a locked keychain from
an ACL or signing-identity mismatch. Existing classic items remain usable when
their unchanged ACL allows silent access. A blocked read or migration is not a
successful single-prompt unlock.
The native authorize result exposes only this allowlisted reason with
`authorized: false`; existing grants remain intact. The desktop offers the protected migration action while preserving the failed
authorization state; a failed read does not grant access.

`SecAccessControl` does not convert classic ACLs. Data Protection access requires
the custody process's own valid access-group entitlements and provisioning;
signing a local executable alone does not provide it. The explicit
`llm-gateway credentials migrate` command is the one-time upgrade path for old
items, including expired credentials. It binds a separate migration scope and
allows macOS to request the old items' native ACL approval, which may require a
password. This exception applies only to migration; normal authorization keeps
its single-biometric contract. No API key re-entry, ACL relaxation, or plaintext
file export is involved. The application copies each item to Data Protection,
reads the destination back, and only then removes its classic source. Failures
preserve the source; retries resume cleanup without replacing a verified
current item. The batch is bounded by the credential inventory and retained
native session, with no client response deadline while the user authenticates.
Completion is recorded by the
[state migration owner](../architecture/CLIENT-UPDATE-AND-STATE-MIGRATION.md#startup-admission).
Migration does not enable credentials or start provider requests. See Apple's
[keychain implementations](https://developer.apple.com/documentation/technotes/tn3137-on-mac-keychains)
and [provisioning requirements](https://developer.apple.com/documentation/technotes/tn3125-inside-code-signing-provisioning-profiles).

The desktop settings page accepts multiple Kimi, DeepSeek, and Kilo keys. The API-key
field is obscured and sent to the native CLI over private stdin. After saving,
an entry can only be deleted; reveal, copy, and edit operations do not exist.
Each delete removes the corresponding Keychain item and revokes active process
leases through a local epoch marker.

The native surface is closed to these operations:

- `llm-gateway credentials status`
- `llm-gateway credentials list`
- `llm-gateway credentials migrate`
- `llm-gateway credentials create --stdin-json true`
- `llm-gateway credentials delete <credential-id>`
- `llm-gateway credentials lease <days>`
- `llm-gateway service status`
- `llm-gateway service start [--port]`
- `llm-gateway service stop [--port]`
- `llm-gateway agent-config plan <codex|claude-code|opencode|pi> <absolute-config-root>`
- `llm-gateway agent-config apply <codex|claude-code|opencode|pi> <absolute-config-root> --confirmation <digest> --confirmed [--stdin-json true]`

The agent configuration command produces a reviewable, secret-free plan. Codex
uses an official custom `model_providers` profile with a Responses API base URL;
Claude Code uses its official `ANTHROPIC_BASE_URL`/`apiKeyHelper` gateway fields.
OpenCode writes a sidecar `opencode.licoup-gateway.json` (OpenAI-compatible
Chat Completions provider) and never rewrites `opencode.json` / `opencode.jsonc`;
load the sidecar with `OPENCODE_CONFIG` so OpenCode merges it with the existing
global config. Pi writes a sidecar `models.licoup-gateway.json` (OpenAI
Completions provider) and never rewrites `models.json` wholesale; the helper
merges only `providers.licoup-gateway` into `~/.pi/agent/models.json`. Unknown
agents fail closed until a precise adapter is added to the canonical runtime
registry. OpenCode and Pi apply receive the plan's exact model snapshot through
private stdin, so upstream catalog drift after preview cannot change confirmed
content.

Developer one-click helpers (same sidecar semantics):

```bash
npm run client:opencode:add-gateway
npm run client:pi:add-gateway
```
