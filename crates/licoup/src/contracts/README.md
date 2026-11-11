# Contract Mirrors

Source of truth: `packages/contracts/client/'.

Do not hand-write DTOs. All Rust-side contract types MUST be generated from the
JSON Schema files in `packages/contracts/client/`. If you need a new contract
type, add the schema there first and regenerate.
