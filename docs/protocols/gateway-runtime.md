# LicoUp Gateway Runtime

English (normative) · [简体中文](gateway-runtime.zh-CN.md)

The Private Gateway Runtime is one local process (`lico-gateway`) owning one
layer:

1. **LLM Gateway** — loopback HTTP model-protocol routing and credential
   handoff. See [`llm-gateway.md`](llm-gateway.md).

The Communication Channel layer no longer lives inside this process: the
Telegram channel is the `org.licoland.channel.telegram` package's own process
(`lico-telegram-channel`), installed on demand and managed by the host.

## Process

- Binary: `lico-gateway` (legacy name `lico-llm-gateway` still runs the same
  runtime). Both are the optional gateway package's own programs
  (`crates/licoup-private-gateway/entry`), declared by the package manifest as
  `runtime.entry = "bin/lico-gateway"`; the kernel declares no gateway target.
- Start: the program itself. It accepts `--port`, `--config`, `--usage`,
  `--state-root`, `--credentials-fd`, `--credentials-control`,
  `--client-token-file` and `--check`, resolves its own
  state layout when a path is not named, materializes its default provider
  configuration when it has none, and mints the client token its authenticated
  endpoint needs. This is what the per-user login item starts, with `--port`
  alone; there is no CLI indirection at login.
- Lifecycle CLI: `gateway service {status,start,stop,initialize}` and the alias
  `llm-gateway service …`. These manage the same program: the client's service
  manager resolves the binary and spawns it with the explicit paths of the same
  layout.
- Inventory CLI: `gateway inventory reload --stdin-json true` performs a
  **partial** hot-reload of verified conversation readiness only (private
  `inventory.sock` under the LLM gateway state directory, plus a persisted
  overlay). The Gateway process is never restarted for inventory changes. When
  the control socket is unavailable, the overlay is written for the next start
  (`mode: overlay_pending`) and the running process is left untouched.
- Channel CLI: `gateway channel status`,
  `gateway channel telegram credentials|pairing|service …`. The channel
  commands manage the Telegram package's custody files and its own process;
  the gateway process carries no channel code.

## Telegram channel

- Transport: Telegram Bot API long polling (default).
- Access: DM pairing; session-level delegation after approval.
- Control: `/start`, `/pair`, `/unpair`, `/whoami`, `/status`, `/agent`,
  `/session`, `/new`, `/reset`, `/stop`, `/help`, `/commands` (aliases:
  `/id`, `/agents`, `/sessions`, `/revoke`).
- Inbound: text, captions, photo/document/video/animation/voice/audio/sticker/
  video_note, location/venue/contact/poll, reply quotes, forwards, and
  `edited_message`, normalized into an agent-facing envelope with media
  placeholders; slash commands parse from text or caption.
- Bridge: paired ordinary content → `conversation_lane` send → `sendMessage`
  reply (threaded to the triggering message).
- Agent admission: `/agent` lists and binds only agents with conversation
  readiness `ready` and a detected local executable, answered live through the
  host's lane port. Unverified inventory is not offered on the channel.
- State files remain under the portable `telegram-gateway/` directory; the
  channel runs as the `org.licoland.channel.telegram` package process
  (`lico-telegram-channel`), installed and downloaded on demand, with its own
  `gateway channel telegram service status|start|stop` lifecycle.

## Status schema

`gateway service status` reports `layers.llm` and `layers.channels` under
schema `licoup.gateway-runtime.v1`; the channel layer reflects the Telegram
package's custody files and ready marker, not an in-process thread. Inventory and status never return bot tokens
or model API keys.
