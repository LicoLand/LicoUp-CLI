//! Which Agents exist on this machine, and where they live.
//!
//! This crate is the single authority for LicoUp's Agent inventory: the
//! thirteen Agent declarations and the target catalog they project, the local
//! binary, desktop-bundle, process, virtual-machine and scan-path discovery
//! that decides whether one is present and usable, the discovery cache and its
//! persisted records, the per-Agent model catalog and its normalisation, the
//! generic CLI registration document, the Lico-owned Agent core, the SSH
//! virtual-machine target, the user login-shell environment every Agent launch
//! observes, and the client autostart entries.
//!
//! `domain` holds three families. `targets` is the family root and declares
//! the whole inventory: `catalog`, the thirteen declarations and their
//! capability projections; `binaries`, `processes`, `platform_paths`,
//! `scan_paths` and `discovery`, the local presence discovery and its
//! execution-admission rule; `manual` and `parameters`, the caller-declared
//! target and the scan parameters; `probe_pool`, the bounded probe fan-out;
//! `target_cache`, the persisted discovery projection; `runtime_binding`, the
//! single local executable a conversation driver may launch;
//! `virtual_machine_discovery` and `scan_merge`, the remote-target and merge
//! projections; and `model_catalog`, which owns one Agent's declared model
//! facts, the packaged `builtin_catalog.json` overlay, the per-Agent config,
//! history and CLI readers, their normalisation and the ordering the renderers
//! preserve. `cli_registration` owns the generic CLI registration document
//! embedded from `resources/agent-hub/cli-registrations.toml`. `lico_agent`
//! owns the Lico-owned Agent core: its loop, tools, profiles, events and
//! Gateway transport.
//!
//! `platform` holds three. `virtual_machine` owns the SSH virtual-machine
//! target contract. `user_shell_environment` owns the login-shell environment
//! snapshot every Agent launch and every user-facing CLI catalog lookup starts
//! from. `client_autostart` owns the desktop, MCP and Gateway login autostart
//! entries and their status document.
//!
//! Two of the crate's inputs are embedded documents and travel with it:
//! `resources/agent-scan-paths.toml`, the scan-path manifest `scan_paths`
//! reads, and `resources/agent-hub/cli-registrations.toml`, the packaged
//! registration document `cli_registration` reads. `model_catalog`'s
//! `builtin_catalog.json` is its own sibling and travels with that tree.
//!
//! Nothing here reaches upward. Every fact this crate reads from a module
//! composed above it arrives through [`port::AgentTargetPort`], the port the
//! inventory declares and composition injects; no name in this crate refers to
//! the driver engines, the model facts, the conversation state or the local
//! model gateway. The LicoUp crates below are `licoup-foundation`, which owns
//! the path, IO, process, environment and user-shell primitives every scan
//! stands on, and `licoup-client-state`, which owns the portable client-state
//! store the discovery cache and target routes are written through.

pub mod domain;
pub mod platform;
pub mod port;
