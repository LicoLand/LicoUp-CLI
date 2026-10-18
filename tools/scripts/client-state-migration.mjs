#!/usr/bin/env node

// Operator-facing diagnosis and a single-step repair for client-state
// migration. Startup migration stays exclusively with the Rust admission in
// `crates/licoup-native/src/domain/client_state_migration.rs`; this tool reads
// the same ledger, probes the same durable shapes and never replaces it.
// The standalone migration program is `crates/licoup-migrate`, distributed as a
// separately downloadable client-release asset with its own command surface; it
// shares the admission owner and the archive owner and is not another converter.

import { runClientStateMigrationCli } from "./client-state-migration/cli.mjs";

runClientStateMigrationCli();
