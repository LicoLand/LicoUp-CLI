# Trybuild Compile-Fail Harness

This Rust crate is the workspace-local compile-fail test harness used by the
client. It provides the minimal `TestCases::compile_fail` surface that
`licoup-native` UI tests rely on to prove that secret-handling and platform
capability misuse fail to compile. It is not published and not the upstream
`trybuild` crate.

- Package definition: [`Cargo.toml`](Cargo.toml)
- Module entry: [`src/lib.rs`](src/lib.rs)

Keep fixtures synthetic and keep compile-fail expectations inside the calling
crate's `tests/ui/` directory.
