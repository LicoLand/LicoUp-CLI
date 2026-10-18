# ADR-0007: User-terminal Agent command identity

Status: accepted for implementation (2026-08-23); shared shell-environment snapshot implemented (2026-09-05), command-binding identity surfaces pending

## Context

An Agent can be installed more than once. The User may select the ordinary
terminal target through PATH order, a version-manager shim, an executable
wrapper, an alias, or a shell function. LicoUp's current manifest-first
discovery can instead bind a known absolute executable, so entering `codex` or
`claude` in the User's terminal and starting that Agent from LicoUp can select
different targets and different startup behavior.

Codex has one approved product-specific exception: when the official ChatGPT
desktop application is installed, its bundled Codex CLI is the preferred
automatic runtime. The User's terminal-selected Codex CLI remains the fallback
when that bundled runtime is unavailable, and a saved manual binary selection
remains authoritative.

An absolute leaf executable is not always the complete command identity. An
alias may contribute prefix arguments, and a function may contribute
environment, wrapper logic, or target selection. Resolving only the final
binary would erase the User's explicit command-line configuration while still
appearing to honor it.

## Decision

1. **The User's command-line result is the default authority.** Automatic Agent
   discovery asks the User's configured command-line environment to resolve
   each canonical Agent command. Without an Agent Center override, LicoUp's
   effective launch binding must be observationally equivalent at the command
   boundary to entering that command in the configured terminal environment,
   except for Codex when the official ChatGPT desktop bundle provides its CLI.
   In that case, the bundled Codex CLI is selected before the terminal result.
2. **Resolve commands in one shared environment snapshot.** A discovery batch
   initializes the selected platform shell environment once and resolves all
   relevant Agent commands against that snapshot. It does not start an Agent,
   send a prompt, or treat an Agent response as discovery evidence.
3. **Preserve command semantics, not only a path.** Discovery records whether
   the command resolved through an executable, script, wrapper, shim, alias, or
   function. It may store a concrete executable binding only when that is a
   lossless representation. Alias/function-supplied arguments, environment, or
   wrapper behavior remain part of a structured or shell-backed binding.
4. **Keep the scan manifest supplementary.** Named paths remain useful for
   configuration and history stores, additional installed versions, desktop
   applications, and LicoUp-managed runtimes. They must not silently outrank a
   valid command selected by the User's command-line environment, except for
   the approved Codex desktop-bundle priority stated above.
5. **Make every alternative visible in Agent Center.** The shell-observed
   binding is the default candidate, except that the approved Codex desktop
   bundle is the default when available. Additional detected and managed
   versions are selectable candidates with distinct provenance. An explicit
   User selection replaces the automatic default and remains authoritative
   until the User changes it or the selected target becomes unavailable.
6. **Layer customization after target selection.** Agent Center environment
   variables, arguments, Hooks, and supported version controls extend the
   selected binding through one visible launch profile with deterministic
   precedence. LicoUp must not claim terminal equivalence after an explicit
   override; it instead shows that the User-selected profile is active.
7. **Keep discovery local and bounded.** Command resolution may consult the
   configured shell startup environment because that environment defines the
   User's command. It does not recursively enumerate PATH directories, probe
   unrelated personal locations, upload paths or command definitions, or
   execute an Agent merely to prove installation.

## Alternatives considered

- **Always launch the first manifest path** — rejected because it can select a
  different installation from the User's terminal and bypass an intentional
  shim or wrapper.
- **Use PATH lookup but ignore aliases and functions** — rejected as the
  product default because it still discards explicit command configuration.
- **Always flatten to the final executable** — rejected because target path
  alone cannot represent arguments, environment, or wrapper behavior.
- **Always evaluate through an interactive shell** — rejected as a universal
  mechanism because ordinary executable, wrapper, and shim bindings can be
  represented and launched directly. A shell-backed binding remains required
  when flattening would change behavior.

## Rationale

The User already owns the command-selection decision in their terminal. Using
that decision as the default removes LicoUp's competing installation priority
and makes multiple installed Agent versions predictable. Representing the
result as a launch binding preserves intentional aliases, functions, wrappers,
arguments, and environment while still allowing direct process launch whenever
the binding is lossless. Agent Center then becomes the explicit place to
depart from, inspect, or extend the terminal default.

## Consequences

- The manifest-first absolute executable is no longer the approved default for
  User-installed Agent CLIs, except for the official ChatGPT desktop-bundled
  Codex CLI.
- Codex automatic resolution prefers the official ChatGPT bundle, then the
  terminal-selected CLI, then supplementary manifest locations; a saved manual
  Codex binary selection precedes all automatic candidates.
- Discovery and launch need a command-binding model that can distinguish direct
  executable and shell-backed semantics.
- A shared shell snapshot avoids one shell startup per Agent while keeping all
  commands in the same command-line environment.
- The UI must distinguish terminal-observed, additionally detected,
  LicoUp-managed, and explicitly selected targets.
- Existing implementation, architecture, status, and compatibility facts do
  not change until source and verification implement this decision.

## Implementation evidence

- 2026-09-05: Decision 2's shared environment snapshot is implemented.
  `crates/licoup-native/src/platform/user_shell_environment.rs` captures the
  user's login-shell environment once per process (bounded, sentinel-framed,
  process-environment fallback) and every conversation-execution launch site
  starts from that snapshot before applying its own functional injections;
  executable discovery consults the snapshot PATH before the scan manifest for
  ordinary targets. Codex first checks the approved desktop-bundled runtime,
  then uses the snapshot PATH as its terminal-selected fallback. Candidate
  priority does not modify the inherited launch environment.
  The command-binding model of decisions 1, 3, 5, and 6 (alias/function
  capture, structured bindings, Agent Center candidate surfaces, launch
  profiles) remains approved-but-unimplemented decision scope.
- 2026-09-28: The Codex desktop-bundle exception is implemented. A saved local
  manual binary path takes precedence; otherwise automatic selection checks
  the official ChatGPT bundle, the terminal PATH, supplementary manifest
  locations, and finally the prior automatic route cache.
