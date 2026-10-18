# LicoUp Native Client

This Rust crate owns the native CLI, local state and platform adapters, agent
drivers, local Subagent MCP, and the
[current retiring endpoint-protection Preview](../../docs/STATUS.md)
implementation used by the Flutter client. It does not own stable endpoint
wire semantics: those belong to a pinned Lico Arc Protocol Line. The preview
is not a Lico Arc Profile and has no future compatibility promise.

- Module map: [`src/lib.rs`](src/lib.rs)
- Public CLI: [`src/bin/licoup.rs`](src/bin/licoup.rs)
- Package definition: [`Cargo.toml`](Cargo.toml)
- Architecture: [`../../docs/architecture/README.md`](../../docs/architecture/README.md)
- Protocols: [`../../docs/protocols/README.md`](../../docs/protocols/README.md)

Run the maintained native verification from the repository root:

```bash
npm run client:native:test
```

Do not place runtime state, local paths, credentials, raw process output, or
device information in this module's fixtures or documentation.
