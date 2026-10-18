# Endpoint collaboration package

`org.licoland.feature.endpoint-collaboration` is the optional package that owns
paired-device pairing, secure mesh relay delivery and remote work control. It is
not a kernel prerequisite: a client without it keeps its local conversations, its
running work and its history.

| Path | Owner |
| --- | --- |
| `package/manifest.json` | Package identity, compatibility range, `endpoint.collaboration.v1` control profile, permissions and extensions |
| `package/package-release.json` | The release declaration the package set reads |
| `package/contributions/control-surface.json` | The declarative mapping onto the kernel's existing command routes |
| `src/lib.rs` | The boundary vocabulary: availability, outbound refusal, recovery report |
| `crates/licoup-native/src/platform/extension_packages/endpoint_collaboration.rs` | The kernel resolution of the installed package and the process-wide outbound gate |

## Boundary rules

- **Absent, disabled, capability-undeclared and unreadable are distinct answers.**
  The kernel resolves one of them from the package store and reports it; a caller
  that only knows how to hide a control cannot answer any of the first three.
- **Disable cuts outbound traffic.** Outbound endpoint sends are refused at the
  transport entry while the capability does not resolve active. The refusal is a
  stable reason (`endpoint_collaboration_package_disabled` and its peers), not a
  user-interface state.
- **The local client stays usable.** Every recovery report states
  `local_client_usable: true`, and disabling or uninstalling the package removes
  no user data and runs no conversion.
- **An announcement grants nothing.** Declaring the capability in a manifest makes
  the package eligible to own the outbound path; it never grants execution
  authority, which stays with the kernel's own ingress, authority and admission
  owners.

## Verification

```sh
cargo test --manifest-path components/endpoint-collaboration/Cargo.toml
cargo test --manifest-path crates/licoup-native/Cargo.toml --lib -j 2 -- \
  platform::extension_packages::endpoint_collaboration
```

## Current state

The package identity, its resolution, the outbound gate and the boundary
vocabulary are implemented and tested. The relay, pairing and secure mesh
implementation still links from the kernel's `domain::mobile_relay` module, and
the kernel declares the package as an off-by-default cargo edge
(`endpoint-collaboration`). Moving that implementation behind the package payload
belongs to the endpoint package milestone's remaining work; until then a default
build reports the pre-package path as `LegacyInKernel` rather than claiming the
package owns it.
