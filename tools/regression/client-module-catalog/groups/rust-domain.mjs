import { rustCrateLayer, command, rustLayer, rustIntegrationTest, rustCrateIntegrationTest, statisticsLayer, defineModule } from "../helpers.mjs";

export const RUST_DOMAIN_MODULES = Object.freeze([
  defineModule({
    id: "rust.domain.agent-usage-metering-wiring",
    kind: "rust-domain",
    summary: "Native agent usage metering wiring",
    inputs: ["crates/licoup-native/tests/agent_usage_metering_wiring.rs"],
    command: rustIntegrationTest("agent_usage_metering_wiring"),
  }),
  defineModule({
    id: "rust.domain.cross-device-entry",
    kind: "rust-domain",
    summary: "Native cross-device entry contract",
    inputs: ["crates/licoup-native/tests/cross_device_entry.rs"],
    command: rustIntegrationTest("cross_device_entry"),
  }),
  defineModule({
    id: "rust.domain.package-route-registry",
    kind: "rust-domain",
    summary: "Package routing and registry composition",
    inputs: ["crates/licoup-native/tests/package_route_registry.rs"],
    command: rustIntegrationTest("package_route_registry"),
  }),
  defineModule({
    id: "rust.domain.project-dependency-commands",
    kind: "rust-domain",
    summary: "Project dependency command integration",
    inputs: ["crates/licoup-native/tests/project_dependency_commands.rs"],
    command: rustIntegrationTest("project_dependency_commands"),
  }),
  defineModule({
      id: "rust.domain.adaptive-flywheel",
      kind: "rust-domain",
      summary: "Immutable strategy packages, compiled Graphs, durable reducer/outbox, and authorized effects",
      inputs: [
        "crates/licoup-native/src/domain/workflow_runtime/**",
        "crates/licoup-workflow-store/**",
        "crates/licoup-workflow/**",
        "crates/licoup-foundation/src/core/safe_archive.rs",
        "crates/licoup-foundation/src/core/safe_archive/**",
        "crates/licoup-native/src/platform/process_sandbox/strategy.rs",
        "crates/licoup-native/src/platform/strategy_runtime/**",
      ],
      command: rustLayer("domain::workflow_"),
    }),
  defineModule({
      id: "rust.domain.adaptive-flywheel.effect-port",
      kind: "rust-domain",
      summary: "Native work-context effect port and strategy runtime dispatch integration",
      inputs: [
        "crates/licoup-native/src/platform/work_context_ports/**",
        "crates/licoup-native/src/platform/strategy_runtime/**",
        "crates/licoup-native/tests/effect_adapter.rs",
      ],
      command: rustIntegrationTest("effect_adapter"),
    }),
  defineModule({
      id: "rust.domain.subagents",
      kind: "rust-domain",
      summary: "Native Assistant and Subagent application with exact Membership delegation",
      inputs: [
        "crates/licoup-native/src/domain/subagents/**",
      ],
      command: rustLayer("domain::subagents::"),
    }),
  defineModule({
      id: "rust.domain.agent-intelligence-catalog",
      kind: "rust-domain",
      summary: "Built-in local intelligence catalog and bounded product metadata",
      inputs: [
        "crates/licoup-native/src/domain/agent_intelligence_catalog.rs",
        "crates/licoup-native/src/domain/agent_intelligence_catalog/**",
      ],
      command: rustLayer("domain::agent_intelligence_catalog::tests::"),
    }),
  defineModule({
      id: "rust.domain.agent-resource-usage",
      kind: "rust-domain",
      summary: "Local agent process attribution, sampling, and resource usage projection",
      inputs: [
        "crates/licoup-native/src/domain/agent_resource_usage.rs",
        "crates/licoup-native/src/domain/agent_resource_usage/**",
      ],
      command: rustLayer("domain::agent_resource_usage::"),
    }),
  defineModule({
      id: "rust.domain.lico-agent",
      kind: "rust-domain",
      summary: "First-party local agent loop, profiles, tools, events, and transport",
      inputs: ["crates/licoup-agent-targets/src/domain/lico_agent/**"],
      command: rustCrateLayer("licoup-agent-targets", "domain::lico_agent::"),
    }),
  defineModule({
      id: "rust.domain.llm-gateway",
      kind: "rust-domain",
      summary: "Local LLM gateway policy, credentials, catalog, configuration, and streaming",
      inputs: [
        "crates/licoup-vault/src/api_key.rs",
        "crates/licoup-private-gateway/src/model/llm_gateway.rs",
        "crates/licoup-private-gateway/src/model/llm_gateway_agent_config.rs",
        "crates/licoup-private-gateway/src/model/llm_gateway_stream.rs",
      ],
      command: rustCrateLayer("licoup-private-gateway", ""),
    }),
  defineModule({
      id: "rust.domain.model-planning",
      kind: "rust-domain",
      summary: "Local model planning, canonical model registry, and selection",
      inputs: [
        "crates/licoup-native/src/domain/model_planning.rs",
        "crates/licoup-native/src/domain/model_registry/**",
      ],
      command: rustLayer("domain::model_"),
    }),
  defineModule({
      id: "rust.domain.client-conversations",
      kind: "rust-domain",
      summary: "Canonical Conversation messaging, membership, indexed events, direct mentions, and migration",
      inputs: [
        "crates/licoup-conversation/Cargo.toml",
        "crates/licoup-conversation/build.rs",
        "crates/licoup-conversation/resources/state-machines.json",
        "crates/licoup-conversation/src/client_conversation/mod.rs",
        "crates/licoup-conversation/src/state_machine/mod.rs",
        "crates/licoup-conversation/src/store/mod.rs",
        "crates/licoup-conversation/src/store/dispatches.rs",
        "crates/licoup-conversation/src/store/events.rs",
        "crates/licoup-conversation/src/store/execution.rs",
        "crates/licoup-conversation/src/store/lifecycle.rs",
        "crates/licoup-conversation/src/store/native_sessions.rs",
        "crates/licoup-conversation/src/store/recovery.rs",
        "crates/licoup-conversation/src/store/schema.rs",
        "crates/licoup-conversation/src/store/schema/**",
        "tests/fixtures/client_state_migration/continuity_layout.rs",
        "tests/fixtures/client_state_migration/retained_related_tables.sql",
        "crates/licoup-conversation/tests/cold_recovery.rs",
        "crates/licoup-conversation/tests/local_conversation.rs",
        "crates/licoup-conversation/src/lib.rs",
        "crates/licoup-native/src/domain/application_port.rs",
        "crates/licoup-native/src/domain/client_conversation/**",
      ],
      command: command(
        "cargo",
        [
          "test",
          "--manifest-path",
          "crates/licoup-native/Cargo.toml",
          "-p",
          "licoup-native",
          "-p",
          "licoup-conversation",
          "--lib",
          "--",
          "domain::client_conversation::",
          "store::execution::tests::",
          "store::native_sessions::tests::",
          "store::schema::tests::",
          "store::tests::open_",
        ],
          10 * 60_000,
      ),
    }),
  defineModule({
      id: "rust.domain.conversation-continuity-store",
      kind: "rust-domain",
      summary: "Conversation continuity store, schema migration, and persisted adoption policy",
      inputs: [
        "crates/licoup-conversation/src/continuity/**",
        "crates/licoup-conversation/tests/continuity_store.rs",
      ],
      command: rustCrateIntegrationTest("licoup-conversation", "continuity_store"),
    }),
  defineModule({
      id: "rust.domain.assistant-continuity",
      kind: "rust-domain",
      summary: "Host-private continuous-assistant continuity module and native work-context proofs",
      inputs: [
        "crates/licoup-native/src/domain/assistant_continuity/**",
        "crates/licoup-native/tests/continuity_native.rs",
      ],
      command: rustCrateIntegrationTest("licoup-native", "continuity_native", ["test-support"]),
    }),
  defineModule({
      id: "rust.domain.assistant-continuity-context",
      kind: "rust-domain",
      summary: "Admitted context composition proofs for the continuous-assistant host",
      inputs: [
        "crates/licoup-native/tests/continuity_context.rs",
      ],
      command: rustCrateIntegrationTest("licoup-native", "continuity_context", ["test-support"]),
    }),
  defineModule({
      id: "rust.domain.assistant-continuity-qualification",
      kind: "rust-domain",
      summary: "Named qualification admission proofs without duplicating the catalog source owner",
      inputs: [
        "crates/licoup-native/tests/continuity_qualification.rs",
      ],
      command: rustCrateIntegrationTest("licoup-native", "continuity_qualification", ["test-support"]),
    }),
  defineModule({
      id: "rust.domain.assistant-continuity-observation",
      kind: "rust-domain",
      summary: "Durable continuity observation facts, scoped index, idempotent replay, and stale proofs",
      inputs: [
        "crates/licoup-native/tests/continuity_observation.rs",
      ],
      command: rustCrateIntegrationTest("licoup-native", "continuity_observation", ["test-support"]),
    }),
  defineModule({
      id: "rust.domain.trusted-history-recovery",
      kind: "rust-domain",
      summary: "Provider-authorized retained history with default readable storage, opt-in encryption, and atomic complete recovery",
      inputs: [
        "crates/licoup-native/src/domain/history_backup/**",
        "crates/licoup-native/src/domain/conversation/history_backup_catalog.rs",
      ],
      command: rustLayer("domain::history_backup::"),
    }),
  defineModule({
      id: "rust.domain.mcp-adapter",
      kind: "rust-domain",
      summary: "Exact-scope MCP preview-to-authorization, one-shot execution, response validation, and projection",
      inputs: [
        "crates/licoup-native/src/domain/mcp_adapter.rs",
        "crates/licoup-native/src/domain/mcp_adapter/approval.rs",
        "crates/licoup-native/src/domain/mcp_adapter/execution.rs",
        "crates/licoup-native/src/domain/mcp_adapter/plan.rs",
        "crates/licoup-native/src/domain/mcp_adapter/sse.rs",
        "crates/licoup-native/src/domain/mcp_adapter/tests.rs",
      ],
      command: rustLayer("domain::mcp_adapter::tests::"),
    }),
  defineModule({
      id: "rust.domain.secure-mesh-command-runtime",
      kind: "rust-domain",
      summary: "Authorized Secure Mesh command composition over local agent runtime ports",
      inputs: [
        "crates/licoup-native/src/domain/secure_mesh_command_runtime.rs",
      ],
      command: rustLayer("domain::secure_mesh_command_runtime::tests::"),
    }),
  defineModule({
      id: "rust.domain.agent-hub",
      kind: "rust-domain",
      summary: "Agent Hub catalog, package recipes, confirmation, version checks, and ownership",
      inputs: [
        "crates/licoup-native/src/domain/agent_hub/**",
        "crates/licoup-native/src/domain/agent_catalog.rs",
        "crates/licoup-native/src/domain/cli_registration.rs",
        "crates/licoup-native/src/domain/native_roles.rs",
      ],
      command: rustLayer("domain::agent_hub::"),
    }),
  defineModule({
      id: "rust.domain.client-authority-registry",
      kind: "rust-domain",
      summary: "Canonical client authority registration and conflict policy",
      inputs: ["crates/licoup-native/src/domain/client_authority_registry.rs"],
      command: rustLayer("domain::client_authority_registry::tests::"),
    }),
  defineModule({
      id: "rust.domain.client-runtime",
      kind: "rust-domain",
      summary: "Bounded client runtime ABI, arena, stream, spool, and agent IPC",
      inputs: ["crates/licoup-native/src/domain/client_runtime/**"],
      command: rustLayer("domain::client_runtime::"),
    }),
  defineModule({
      id: "rust.domain.protocol-input-admission",
      kind: "rust-domain",
      summary: "Protocol input size and structure admission",
      inputs: ["crates/licoup-native/src/domain/protocol_input_admission.rs"],
      command: rustLayer("domain::protocol_input_admission::tests::"),
    }),
  defineModule({
      id: "rust.domain.release-receipts",
      kind: "rust-domain",
      summary: "Typed release receipt validation and projection",
      inputs: ["crates/licoup-native/src/domain/release_receipts/**"],
      command: rustLayer("domain::release_receipts::"),
    }),
  defineModule({
      id: "rust.domain.resource-bounds",
      kind: "rust-domain",
      summary: "Bounded history and search resource policy",
      inputs: ["crates/licoup-native/src/domain/resource_bounds/**"],
      command: rustLayer("domain::resource_bounds::"),
    }),
  defineModule({
      id: "rust.domain.session-policy",
      kind: "rust-domain",
      summary: "Session capability, review, and admission policy",
      inputs: [
        "crates/licoup-native/src/domain/session_policy/**",
        "crates/licoup-native/src/domain/dispatch_timeout_policy.rs",
      ],
      command: rustLayer("domain::session_policy::"),
    }),
  defineModule({
      id: "rust.domain.provider-quota",
      kind: "rust-domain",
      summary: "Provider-quota snapshot contract, per-provider sources, refresh scheduler, and retained store",
      targetEvidenceHosts: ["darwin", "linux", "win32"],
      inputs: [
        "crates/licoup-native/src/domain/provider_quota.rs",
        "crates/licoup-native/src/domain/provider_quota/**",
      ],
      command: rustLayer("domain::provider_quota::"),
    }),
  defineModule({
      id: "rust.domain.agent-usage",
      kind: "rust-domain",
      summary: "Native agent-usage facade, host-port composition, and product readers",
      inputs: [
        "crates/licoup-native/src/domain/agent_usage.rs",
        "crates/licoup-native/src/domain/agent_usage/host.rs",
        "crates/licoup-native/src/domain/agent_usage/tests.rs",
      ],
      command: rustLayer("domain::agent_usage::"),
    }),
  defineModule({
      id: "rust.statistics",
      kind: "rust-domain",
      summary: "Local usage measurement engine: journal, incremental projection, attribution, retention and source-host composition",
      inputs: [
        "crates/licoup-statistics/**",
      ],
      command: statisticsLayer("::tests::"),
    }),
  defineModule({
      id: "rust.domain.agent-usage.window",
      kind: "rust-domain",
      summary: "Default 30-day and explicitly selected local-calendar usage windows",
      inputs: [
        "crates/licoup-statistics/src/window.rs",
      ],
      command: statisticsLayer("window::tests::"),
    }),
  defineModule({
      id: "rust.domain.agent-usage.native-cache",
      kind: "rust-domain",
      summary: "Hosted Cursor ledger plus exact native metadata readers with append cursors and immutable historical day/model rollups",
      inputs: [
        "crates/licoup-native/src/domain/agent_usage/agent_usage_native.rs",
        "crates/licoup-native/src/domain/agent_usage/agent_usage_native/cache.rs",
        "crates/licoup-native/src/domain/agent_usage/agent_usage_native/cache_variant_tests.rs",
        "crates/licoup-native/src/domain/agent_usage/agent_usage_native/cursor.rs",
        "crates/licoup-native/src/domain/agent_usage/agent_usage_native/deepseek.rs",
        "crates/licoup-native/src/domain/agent_usage/agent_usage_native/deepseek_reader.mjs",
        "crates/licoup-native/src/domain/agent_usage/agent_usage_native/files.rs",
        "crates/licoup-native/src/domain/agent_usage/agent_usage_native/identity_refresh.rs",
        "crates/licoup-native/src/domain/agent_usage/agent_usage_native/models.rs",
        "crates/licoup-native/src/domain/agent_usage/agent_usage_native/migration_tests.rs",
        "crates/licoup-native/src/domain/agent_usage/agent_usage_native/parser.rs",
        "crates/licoup-native/src/domain/agent_usage/agent_usage_native/parser/hermes.rs",
        "crates/licoup-native/src/domain/agent_usage/agent_usage_native/parser/openagent.rs",
        "crates/licoup-native/src/domain/agent_usage/agent_usage_native/openclaw.rs",
        "crates/licoup-native/src/domain/agent_usage/agent_usage_native/runtime.rs",
        "crates/licoup-native/src/domain/agent_usage/agent_usage_native/snapshot_cursor.rs",
        "crates/licoup-native/src/domain/agent_usage/agent_usage_native/watermark.rs",
      ],
      command: rustLayer("domain::agent_usage::agent_usage_native::"),
    }),
  defineModule({
      id: "rust.domain.agent-usage.deepseek-reader",
      kind: "rust-domain",
      summary: "Official DeepSeek session persistence usage extraction and attempt accounting",
      inputs: [
        "crates/licoup-native/src/domain/agent_usage/agent_usage_native/deepseek_reader.mjs",
        "crates/licoup-native/src/domain/agent_usage/agent_usage_native/deepseek_reader.test.mjs",
      ],
      command: command("node", ["--test", "crates/licoup-native/src/domain/agent_usage/agent_usage_native/deepseek_reader.test.mjs"], 60_000),
    }),
  defineModule({
      id: "rust.domain.agent-usage-cache",
      kind: "rust-domain",
      summary: "Codex usage scan orchestration and incremental-cache integration scenarios",
      inputs: [
        "crates/licoup-native/src/domain/agent_usage/agent_usage_codex.rs",
        "crates/licoup-native/src/domain/agent_usage/agent_usage_codex/scan.rs",
        "crates/licoup-native/tests/agent_usage_incremental_cache.rs",
        "crates/licoup-native/tests/agent_usage_cache_cases/mod.rs",
        "crates/licoup-native/tests/agent_usage_cache_cases/support.rs",
      ],
      command: rustIntegrationTest(
        "agent_usage_incremental_cache",
        "agent_usage_cache_cases::",
      ),
    }),
  defineModule({
      id: "rust.domain.agent-usage-cache.adapter-coverage",
      kind: "rust-domain",
      summary: "Adapter coverage for supported local usage-history sources",
      inputs: [
        "crates/licoup-native/tests/agent_usage_cache_cases/adapter_coverage.rs",
      ],
      command: rustIntegrationTest(
        "agent_usage_incremental_cache",
        "agent_usage_cache_cases::adapter_coverage::",
      ),
    }),
  defineModule({
      id: "rust.domain.agent-usage-cache.append-refresh",
      kind: "rust-domain",
      summary: "Append-only reuse and forced-rescan cache integrity scenarios",
      inputs: [
        "crates/licoup-native/tests/agent_usage_cache_cases/append_refresh.rs",
      ],
      command: rustIntegrationTest(
        "agent_usage_incremental_cache",
        "agent_usage_cache_cases::append_refresh::",
      ),
    }),
  defineModule({
      id: "rust.domain.agent-usage-cache.runtime",
      kind: "rust-domain",
      summary: "Incomplete tails, root isolation, and busy-snapshot cache scenarios",
      inputs: [
        "crates/licoup-native/tests/agent_usage_cache_cases/cache_runtime.rs",
      ],
      command: rustIntegrationTest(
        "agent_usage_incremental_cache",
        "agent_usage_cache_cases::cache_runtime::",
      ),
    }),
  defineModule({
      id: "rust.domain.agent-usage-cache.cumulative-resume",
      kind: "rust-domain",
      summary: "Cross-day cumulative watermarks for resumed historical sessions",
      inputs: [
        "crates/licoup-native/tests/agent_usage_cache_cases/cumulative_resume.rs",
      ],
      command: rustIntegrationTest(
        "agent_usage_incremental_cache",
        "agent_usage_cache_cases::cumulative_resume::",
      ),
    }),
  defineModule({
      id: "rust.domain.agent-usage-cache.dedup-lineage",
      kind: "rust-domain",
      summary: "Copy identity, fork lineage, and independent-session scenarios",
      inputs: [
        "crates/licoup-native/tests/agent_usage_cache_cases/dedup_lineage.rs",
      ],
      command: rustIntegrationTest(
        "agent_usage_incremental_cache",
        "agent_usage_cache_cases::dedup_lineage::",
      ),
    }),
  defineModule({
      id: "rust.domain.agent-usage-cache.fallback-coverage",
      kind: "rust-domain",
      summary: "Fallback coverage for unavailable and unsupported local usage-history sources",
      inputs: [
        "crates/licoup-native/tests/agent_usage_cache_cases/fallback_coverage.rs",
      ],
      command: rustIntegrationTest(
        "agent_usage_incremental_cache",
        "agent_usage_cache_cases::fallback_coverage::",
      ),
    }),
  defineModule({
      id: "rust.domain.agent-usage-cache.generic-usage",
      kind: "rust-domain",
      summary: "Generic local history token normalization scenarios",
      inputs: [
        "crates/licoup-native/tests/agent_usage_cache_cases/generic_usage.rs",
      ],
      command: rustIntegrationTest(
        "agent_usage_incremental_cache",
        "agent_usage_cache_cases::generic_usage::",
      ),
    }),
  defineModule({
      id: "rust.domain.agent-usage-cache.native-rollup",
      kind: "rust-domain",
      summary: "Native append reuse, day finalization, and current-day mutable-row scenario",
      inputs: [
        "crates/licoup-native/tests/agent_usage_cache_cases/native_rollup.rs",
      ],
      command: rustIntegrationTest(
        "agent_usage_incremental_cache",
        "agent_usage_cache_cases::native_rollup::",
      ),
    }),
  defineModule({
      id: "rust.domain.agent-usage-cache.reconciliation",
      kind: "rust-domain",
      summary: "Codex cumulative, duplicate, and divergent total reconciliation scenario",
      inputs: [
        "crates/licoup-native/tests/agent_usage_cache_cases/reconciliation.rs",
      ],
      command: rustIntegrationTest(
        "agent_usage_incremental_cache",
        "agent_usage_cache_cases::reconciliation::",
      ),
    }),
  defineModule({
      id: "rust.domain.agent-usage-cache.retained-reports",
      kind: "rust-domain",
      summary: "Current retained-report contract pruning and ordering scenario",
      inputs: [
        "crates/licoup-native/tests/agent_usage_cache_cases/retained_reports.rs",
      ],
      command: rustIntegrationTest(
        "agent_usage_incremental_cache",
        "agent_usage_cache_cases::retained_reports::",
      ),
    }),
  defineModule({
      id: "rust.domain.agent-usage-cache.two-phase",
      kind: "rust-domain",
      summary: "Bounded native usage connection reuse and unstable-source abort",
      inputs: [
        "crates/licoup-native/tests/agent_usage_cache_cases/two_phase.rs",
      ],
      command: rustIntegrationTest(
        "agent_usage_incremental_cache",
        "agent_usage_cache_cases::two_phase::",
      ),
    }),
  defineModule({
      id: "rust.domain.agent-usage-cache.windows",
      kind: "rust-domain",
      summary: "Local calendar and historical timezone-transition scenarios",
      inputs: [
        "crates/licoup-native/tests/agent_usage_cache_cases/windows.rs",
      ],
      command: rustIntegrationTest(
        "agent_usage_incremental_cache",
        "agent_usage_cache_cases::windows::",
      ),
    }),
  defineModule({
      id: "rust.domain.agent-usage.codex-models",
      kind: "rust-domain",
      summary: "Codex token totals, parser state, and scan metrics",
      inputs: [
        "crates/licoup-native/src/domain/agent_usage/agent_usage_codex/constants.rs",
        "crates/licoup-native/src/domain/agent_usage/agent_usage_codex/models.rs",
        "crates/licoup-native/src/domain/agent_usage/agent_usage_codex/tests/models.rs",
      ],
      command: rustLayer("domain::agent_usage::agent_usage_codex::tests::models::"),
    }),
  defineModule({
      id: "rust.domain.agent-usage.codex-utils",
      kind: "rust-domain",
      summary: "Codex numeric storage and turn-identity helpers",
      inputs: [
        "crates/licoup-native/src/domain/agent_usage/agent_usage_codex/models.rs",
        "crates/licoup-native/src/domain/agent_usage/agent_usage_codex/utils.rs",
        "crates/licoup-native/src/domain/agent_usage/agent_usage_codex/tests/utils.rs",
      ],
      command: rustLayer("domain::agent_usage::agent_usage_codex::tests::utils::"),
    }),
  defineModule({
      id: "rust.domain.agent-usage.codex-scan-params",
      kind: "rust-domain",
      summary: "Codex local-root discovery parameters and path-safe cache identities",
      inputs: [
        "crates/licoup-native/src/domain/agent_usage/agent_usage_codex/scan_params.rs",
        "crates/licoup-native/src/domain/agent_usage/agent_usage_codex/tests/scan_params.rs",
      ],
      command: rustLayer("domain::agent_usage::agent_usage_codex::tests::scan_params::"),
    }),
  defineModule({
      id: "rust.domain.agent-usage.codex-files",
      kind: "rust-domain",
      summary: "Iterative local Codex history discovery and portable file metadata",
      inputs: [
        "crates/licoup-native/src/domain/agent_usage/agent_usage_codex/file_collection.rs",
        "crates/licoup-native/src/domain/agent_usage/agent_usage_codex/tests/file_collection.rs",
      ],
      command: rustLayer("domain::agent_usage::agent_usage_codex::tests::file_collection::"),
    }),
  defineModule({
      id: "rust.domain.agent-usage.codex-append-guard",
      kind: "rust-domain",
      summary: "Codex append-only prefix integrity and incremental guard extension",
      inputs: [
        "crates/licoup-native/src/domain/agent_usage/agent_usage_codex/append_guard.rs",
        "crates/licoup-native/src/domain/agent_usage/agent_usage_codex/constants.rs",
        "crates/licoup-native/src/domain/agent_usage/agent_usage_codex/tests/append_guard.rs",
      ],
      command: rustLayer("domain::agent_usage::agent_usage_codex::tests::append_guard::"),
    }),
  defineModule({
      id: "rust.domain.agent-usage.codex-event-hash",
      kind: "rust-domain",
      summary: "Canonical Codex event-chain identities for copy and fork reconciliation",
      inputs: [
        "crates/licoup-native/src/domain/agent_usage/agent_usage_codex/event_hash.rs",
        "crates/licoup-native/src/domain/agent_usage/agent_usage_codex/tests/event_hash.rs",
      ],
      command: rustLayer("domain::agent_usage::agent_usage_codex::tests::event_hash::"),
    }),
  defineModule({
      id: "rust.domain.agent-usage.codex-lineage",
      kind: "rust-domain",
      summary: "Codex fork lineage reconciliation and deterministic cycle handling",
      inputs: [
        "crates/licoup-native/src/domain/agent_usage/agent_usage_codex/lineage.rs",
        "crates/licoup-native/src/domain/agent_usage/agent_usage_codex/tests/lineage.rs",
      ],
      command: rustLayer("domain::agent_usage::agent_usage_codex::tests::lineage::"),
    }),
  defineModule({
      id: "rust.domain.agent-usage.codex-cache-database",
      kind: "rust-domain",
      summary: "Private SQLite cache schema, freshness, locking, and indexed source lookup",
      inputs: [
        "crates/licoup-native/src/domain/agent_usage/agent_usage_codex/cache.rs",
        "crates/licoup-native/src/domain/agent_usage/agent_usage_codex/cache_cleanup.rs",
        "crates/licoup-native/src/domain/agent_usage/agent_usage_codex/constants.rs",
        "crates/licoup-native/src/domain/agent_usage/agent_usage_codex/tests/cache.rs",
      ],
      command: rustLayer("domain::agent_usage::agent_usage_codex::tests::cache::"),
    }),
  defineModule({
      id: "rust.domain.agent-usage.codex-cache-batch",
      kind: "rust-domain",
      summary: "Prepared cache-state load, save, reset, and delete transaction batch",
      inputs: [
        "crates/licoup-native/src/domain/agent_usage/agent_usage_codex/cache_batch.rs",
        "crates/licoup-native/src/domain/agent_usage/agent_usage_codex/tests/cache_batch.rs",
      ],
      command: rustLayer("domain::agent_usage::agent_usage_codex::tests::cache_batch::"),
    }),
  defineModule({
      id: "rust.domain.agent-usage.codex-parser",
      kind: "rust-domain",
      summary: "Incremental JSONL parsing and exact provider token deltas",
      inputs: [
        "crates/licoup-native/src/domain/agent_usage/agent_usage_codex/parser.rs",
        "crates/licoup-native/src/domain/agent_usage/agent_usage_codex/tests/parser.rs",
      ],
      command: rustLayer("domain::agent_usage::agent_usage_codex::tests::parser::"),
    }),
  defineModule({
      id: "rust.domain.agent-usage.codex-rollup",
      kind: "rust-domain",
      summary: "Immutable historical day/model/session reduction with current-day detail retention",
      inputs: [
        "crates/licoup-native/src/domain/agent_usage/agent_usage_codex/rollup.rs",
      ],
      command: rustLayer("domain::agent_usage::agent_usage_codex::tests::parser::"),
    }),
  defineModule({
      id: "rust.domain.agent-usage.codex-identity-refresh",
      kind: "rust-domain",
      summary: "Current-day model identity correction with unchanged source and token evidence",
      inputs: [
        "crates/licoup-native/src/domain/agent_usage/agent_usage_codex/identity_refresh.rs",
      ],
      command: rustLayer("domain::agent_usage::agent_usage_codex::identity_refresh::"),
    }),
  defineModule({
      id: "rust.domain.agent-usage.codex-aggregation",
      kind: "rust-domain",
      summary: "Windowed agent/model projection with copy, coverage, and lineage deduplication",
      inputs: [
        "crates/licoup-native/src/domain/agent_usage/agent_usage_codex/aggregation.rs",
        "crates/licoup-native/src/domain/agent_usage/agent_usage_codex/tests/aggregation.rs",
      ],
      command: rustLayer("domain::agent_usage::agent_usage_codex::tests::aggregation::"),
    }),
  defineModule({
      id: "rust.domain.agent-usage.codex-test-support",
      kind: "rust-domain",
      summary: "Shared synthetic fixtures and ordinary unit-test module composition",
      inputs: [
        "crates/licoup-native/src/domain/agent_usage/agent_usage_codex/tests/mod.rs",
        "crates/licoup-native/src/domain/agent_usage/agent_usage_codex/tests/support.rs",
      ],
      command: rustLayer("domain::agent_usage::agent_usage_codex::tests::"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.composition",
      kind: "rust-domain",
      summary: "Conversation facade, split module graph, and shared test composition",
      inputs: [
        "crates/licoup-native/src/domain/conversations.rs",
        "crates/licoup-native/src/domain/conversation/mod.rs",
        "crates/licoup-native/src/domain/conversation/history/mod.rs",
        "crates/licoup-native/src/domain/conversation/history/tests.rs",
        "crates/licoup-native/src/domain/conversation/history/tests/test_support.rs",
        "crates/licoup-native/src/domain/conversation/history/delegated_transcripts.rs",
        "crates/licoup-native/src/domain/conversation/history/project_workspace.rs",
        "crates/licoup-native/src/domain/conversation/history/projection_cache.rs",
      ],
      command: rustLayer(
        "domain::conversation::history::tests::split_history_module_composition_keeps_the_public_schema",
      ),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.parser-dispatch",
      kind: "rust-domain",
      summary: "Narrow source-to-parser routing port",
      inputs: [
        "crates/licoup-native/src/domain/conversation/adapter_dispatch.rs",
        "crates/licoup-native/src/domain/conversation/parser_port.rs",
      ],
      command: rustLayer("domain::conversation::adapter_dispatch::tests"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.discovery",
      kind: "rust-domain",
      summary: "Bounded local history file discovery",
      inputs: [
        "crates/licoup-native/src/domain/conversation/history_discovery.rs",
      ],
      command: rustLayer("domain::conversation::history_discovery::tests"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.parameters",
      kind: "rust-domain",
      summary: "Conversation command parameter normalization",
      inputs: [
        "crates/licoup-native/src/domain/conversation/parameters.rs",
      ],
      command: rustLayer("domain::conversation::parameters::tests"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.paths",
      kind: "rust-domain",
      summary: "Cross-platform local history path resolution",
      inputs: [
        "crates/licoup-native/src/domain/conversation/paths.rs",
      ],
      command: rustLayer("domain::conversation::paths::tests"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.source-catalog",
      kind: "rust-domain",
      summary: "Agent history adapter and source-root catalog",
      inputs: [
        "crates/licoup-native/src/domain/conversation/source_catalog.rs",
        "crates/licoup-agent-targets/src/domain/targets/scan_paths.rs",
        "crates/licoup-native/resources/agent-scan-paths.toml",
      ],
      command: rustLayer("domain::conversation::source_catalog::tests"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.query",
      kind: "rust-domain",
      summary: "Conversation query filters, pagination, model catalog, and exact execution provenance",
      inputs: [
        "crates/licoup-native/src/domain/conversation/history/query.rs",
        "crates/licoup-native/src/domain/conversation/history/query_filter.rs",
        "crates/licoup-native/src/domain/conversation/history/execution_provenance.rs",
        "crates/licoup-native/src/domain/conversation/history/tests/query.rs",
      ],
      command: rustLayer(
        "domain::conversation::history::execution_provenance::tests::",
        ["domain::conversation::history::tests::query"],
      ),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.catalog",
      kind: "rust-domain",
      summary: "Bounded browse catalog, page hydration, and native runtime facts",
      inputs: [
        "crates/licoup-native/src/domain/conversation/history/catalog.rs",
        "crates/licoup-native/src/domain/conversation/history/tests/catalog.rs",
        "crates/licoup-native/tests/conversation_catalog_invariants.rs",
      ],
      command: rustLayer("domain::conversation::history::tests::catalog::"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.parser-kimi",
      kind: "rust-domain",
      summary: "Kimi Code wire parser and usage records",
      inputs: [
        "crates/licoup-native/src/domain/conversation/history/kimi.rs",
        "crates/licoup-native/src/domain/conversation/history/tests/kimi.rs",
      ],
      command: rustLayer("domain::conversation::history::tests::kimi"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.parser-codex",
      kind: "rust-domain",
      summary: "Codex rollout parser, grouped events, and native usage metadata",
      inputs: [
        "crates/licoup-native/src/domain/conversation/history/codex.rs",
        "crates/licoup-native/src/domain/conversation/history/tests/codex.rs",
      ],
      command: rustLayer("domain::conversation::history::tests::codex"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.parser-pi-copilot",
      kind: "rust-domain",
      summary: "Pi JSONL and Copilot transcript parsers",
      inputs: [
        "crates/licoup-native/src/domain/conversation/history/pi_copilot.rs",
        "crates/licoup-native/src/domain/conversation/history/tests/pi_copilot.rs",
      ],
      command: rustLayer("domain::conversation::history::tests::pi_copilot"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.parser-cursor-openagent.composition",
      kind: "rust-domain",
      summary: "Thin parser facade, read-only connection, precise-adapter routing, and generic fallback",
      inputs: [
        "crates/licoup-native/src/domain/conversation/history/cursor_openagent.rs",
        "crates/licoup-native/src/domain/conversation/history/cursor_openagent/composition.rs",
        "crates/licoup-native/src/domain/conversation/history/cursor_openagent/cursor_cli.rs",
        "crates/licoup-native/src/domain/conversation/history/cursor_openagent/tests/mod.rs",
        "crates/licoup-native/src/domain/conversation/history/cursor_openagent/tests/composition.rs",
      ],
      command: rustLayer("domain::conversation::history::cursor_openagent::tests::composition::"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.parser-cursor-openagent.codec",
      kind: "rust-domain",
      summary: "Read-only SQLite access and bounded field, value, row, and source-field projection",
      inputs: [
        "crates/licoup-native/src/domain/conversation/history/cursor_openagent/codec.rs",
        "crates/licoup-native/src/domain/conversation/history/cursor_openagent/tests/codec.rs",
      ],
      command: rustLayer("domain::conversation::history::cursor_openagent::tests::codec::"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.parser-cursor-openagent.cursor",
      kind: "rust-domain",
      summary: "Cursor composer and bubble SQLite record parsing",
      inputs: [
        "crates/licoup-native/src/domain/conversation/history/cursor_openagent/cursor.rs",
        "crates/licoup-native/src/domain/conversation/history/cursor_openagent/tests/cursor.rs",
      ],
      command: rustLayer("domain::conversation::history::cursor_openagent::tests::cursor::"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.parser-cursor-openagent.cursor-projection",
      kind: "rust-domain",
      summary: "Cursor bubble role, selected-model, timestamp, and usage projection",
      inputs: [
        "crates/licoup-native/src/domain/conversation/history/cursor_openagent/cursor_projection.rs",
        "crates/licoup-native/src/domain/conversation/history/cursor_openagent/tests/cursor_projection.rs",
      ],
      command: rustLayer("domain::conversation::history::cursor_openagent::tests::cursor_projection::"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.parser-cursor-openagent.openagent",
      kind: "rust-domain",
      summary: "OpenAgent session, message, part, time, and additive usage SQLite projection",
      inputs: [
        "crates/licoup-native/src/domain/conversation/history/cursor_openagent/openagent.rs",
        "crates/licoup-native/src/domain/conversation/history/cursor_openagent/tests/openagent.rs",
      ],
      command: rustLayer("domain::conversation::history::cursor_openagent::tests::openagent::"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.parser-cursor-openagent.fallback",
      kind: "rust-domain",
      summary: "Paged generic SQLite fallback with adapter row admission and bounded codec reuse",
      inputs: [
        "crates/licoup-native/src/domain/conversation/history/cursor_openagent/fallback.rs",
        "crates/licoup-native/src/domain/conversation/history/cursor_openagent/tests/fallback.rs",
      ],
      command: rustLayer("domain::conversation::history::cursor_openagent::tests::fallback::"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.parser-cursor-openagent.integration",
      kind: "rust-domain",
      summary: "Cursor, OpenCode, Kilo Code, usage, archive-row, and generic-record integration",
      inputs: [
        "crates/licoup-native/src/domain/conversation/history/tests/cursor_openagent.rs",
      ],
      command: rustLayer("domain::conversation::history::tests::cursor_openagent"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.parser-generic",
      kind: "rust-domain",
      summary: "Generic JSON, JSONL, embedded JSON, and text transcript parsers",
      inputs: [
        "crates/licoup-native/src/domain/conversation/history/generic.rs",
        "crates/licoup-native/src/domain/conversation/history/tests/generic.rs",
      ],
      command: rustLayer("domain::conversation::history::tests::generic"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.session-merge.composition",
      kind: "rust-domain",
      summary: "Thin session-merge facade and final filtering composition",
      inputs: [
        "crates/licoup-native/src/domain/conversation/history/session_merge.rs",
        "crates/licoup-native/src/domain/conversation/history/session_merge/composition.rs",
        "crates/licoup-native/src/domain/conversation/history/session_merge/tests/mod.rs",
        "crates/licoup-native/src/domain/conversation/history/session_merge/tests/composition.rs",
      ],
      command: rustLayer("domain::conversation::history::session_merge::tests::composition::"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.session-merge.codex-lineage",
      kind: "rust-domain",
      summary: "Cycle-safe Codex rollout lineage roots, replay dedupe, and deterministic collapse",
      inputs: [
        "crates/licoup-native/src/domain/conversation/history/session_merge/codex_lineage.rs",
        "crates/licoup-native/src/domain/conversation/history/session_merge/tests/codex_lineage.rs",
      ],
      command: rustLayer("domain::conversation::history::session_merge::tests::codex_lineage::"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.session-merge.delegated",
      kind: "rust-domain",
      summary: "Leaf-to-root delegated-subagent graph closure and bounded nearest-session fallback",
      inputs: [
        "crates/licoup-native/src/domain/conversation/history/session_merge/delegated_merge.rs",
        "crates/licoup-native/src/domain/conversation/history/session_merge/tests/delegated_merge.rs",
      ],
      command: rustLayer("domain::conversation::history::session_merge::tests::delegated_merge::"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.session-merge.dedupe-paging",
      kind: "rust-domain",
      summary: "Adapter-aware session identity deduplication and bounded offset paging",
      inputs: [
        "crates/licoup-native/src/domain/conversation/history/session_merge/dedupe_paging.rs",
        "crates/licoup-native/src/domain/conversation/history/session_merge/tests/dedupe_paging.rs",
      ],
      command: rustLayer("domain::conversation::history::session_merge::tests::dedupe_paging::"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.session-merge.model-names",
      kind: "rust-domain",
      summary: "Depth-bounded history model discovery and bounded model-name sanitation",
      inputs: [
        "crates/licoup-native/src/domain/conversation/history/session_merge/model_names.rs",
        "crates/licoup-native/src/domain/conversation/history/session_merge/tests/model_names.rs",
      ],
      command: rustLayer("domain::conversation::history::session_merge::tests::model_names::"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.session-merge.session-index",
      kind: "rust-domain",
      summary: "Local Codex session-index candidate discovery, latest-title parsing, and file IO",
      inputs: [
        "crates/licoup-native/src/domain/conversation/history/session_merge/session_index.rs",
        "crates/licoup-native/src/domain/conversation/history/session_merge/tests/session_index.rs",
      ],
      command: rustLayer("domain::conversation::history::session_merge::tests::session_index::"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.session-merge.stable-order",
      kind: "rust-domain",
      summary: "RFC3339 and numeric time keys with deterministic newest-first session ordering",
      inputs: [
        "crates/licoup-native/src/domain/conversation/history/session_merge/stable_order.rs",
        "crates/licoup-native/src/domain/conversation/history/session_merge/tests/stable_order.rs",
      ],
      command: rustLayer("domain::conversation::history::session_merge::tests::stable_order::"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.session-merge.integration",
      kind: "rust-domain",
      summary: "Adapter-level delegated, Codex lineage, and active/archive dedupe integration",
      inputs: [
        "crates/licoup-native/src/domain/conversation/history/tests/session_merge.rs",
      ],
      command: rustLayer("domain::conversation::history::tests::session_merge"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.session-metadata",
      kind: "rust-domain",
      summary: "Session construction, titles, source identity, and evidence metadata",
      inputs: [
        "crates/licoup-native/src/domain/conversation/history/session_metadata.rs",
        "crates/licoup-native/src/domain/conversation/history/tests/session_metadata.rs",
      ],
      command: rustLayer("domain::conversation::history::tests::session_metadata"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.message-projection.composition",
      kind: "rust-domain",
      summary: "Thin message-projection root and adapter-level stable semantic projection",
      inputs: [
        "crates/licoup-native/src/domain/conversation/history/mod.rs",
        "crates/licoup-native/src/domain/conversation/history/message_projection.rs",
        "crates/licoup-native/src/domain/conversation/history/message_projection/tests/mod.rs",
        "crates/licoup-native/src/domain/conversation/history/tests/message_projection.rs",
      ],
      command: rustLayer("domain::conversation::history::tests::message_projection"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.message-projection.projection",
      kind: "rust-domain",
      summary: "Plain, structured, delegated, and Antigravity-cleaned message layer projection",
      inputs: [
        "crates/licoup-native/src/domain/conversation/history/message_projection/projection.rs",
        "crates/licoup-native/src/domain/conversation/history/message_projection/tests/projection.rs",
      ],
      command: rustLayer("domain::conversation::history::message_projection::tests::projection::"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.message-projection.structured-privacy",
      kind: "rust-domain",
      summary: "Cached structured-event redaction, bounded provider summaries, and raw-payload rejection",
      inputs: [
        "crates/licoup-native/src/domain/conversation/history/message_projection/structured_privacy.rs",
        "crates/licoup-native/src/domain/conversation/history/message_projection/tests/structured_privacy.rs",
      ],
      command: rustLayer("domain::conversation::history::message_projection::tests::structured_privacy::"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.message-projection.antigravity",
      kind: "rust-domain",
      summary: "Antigravity user-request extraction, system stripping, gutter handling, and artifact rejection",
      inputs: [
        "crates/licoup-native/src/domain/conversation/history/message_projection/antigravity.rs",
        "crates/licoup-native/src/domain/conversation/history/message_projection/tests/antigravity.rs",
      ],
      command: rustLayer("domain::conversation::history::message_projection::tests::antigravity::"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.message-projection.generated-context",
      kind: "rust-domain",
      summary: "Generated context block removal and fail-closed control/background prompt filtering",
      inputs: [
        "crates/licoup-native/src/domain/conversation/history/message_projection/generated_context.rs",
        "crates/licoup-native/src/domain/conversation/history/message_projection/tests/generated_context.rs",
      ],
      command: rustLayer("domain::conversation::history::message_projection::tests::generated_context::"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.message-projection.json-extract",
      kind: "rust-domain",
      summary: "Depth-bounded JSON and embedded-text extraction with stable role, time, and session projection",
      inputs: [
        "crates/licoup-native/src/domain/conversation/history/message_projection/json_extract.rs",
        "crates/licoup-native/src/domain/conversation/history/message_projection/tests/json_extract.rs",
      ],
      command: rustLayer("domain::conversation::history::message_projection::tests::json_extract::"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.message-projection.semantic",
      kind: "rust-domain",
      summary: "Message semantic normalization, structured kind classification, and delegated prompt title policy",
      inputs: [
        "crates/licoup-native/src/domain/conversation/history/message_projection/semantic.rs",
        "crates/licoup-native/src/domain/conversation/history/message_projection/tests/semantic.rs",
      ],
      command: rustLayer("domain::conversation::history::message_projection::tests::semantic::"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.snapshots.composition",
      kind: "rust-domain",
      summary: "Conversation snapshot public facade, module graph, and shared test support",
      inputs: [
        "crates/licoup-native/src/domain/conversation_snapshots.rs",
        "crates/licoup-native/src/domain/conversation/snapshots/mod.rs",
        "crates/licoup-native/src/domain/conversation/snapshots/tests.rs",
        "crates/licoup-native/src/domain/conversation/snapshots/tests/test_support.rs",
      ],
      command: rustLayer(
        "domain::conversation::snapshots::tests::split_snapshot_module_composition_keeps_the_public_facade",
      ),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.snapshots.settings",
      kind: "rust-domain",
      summary: "Snapshot roots, collection settings, destinations, and archive profiles",
      inputs: [
        "crates/licoup-native/src/domain/conversation/snapshots/settings.rs",
        "crates/licoup-native/src/domain/conversation/snapshots/tests/settings.rs",
      ],
      command: rustLayer("domain::conversation::snapshots::tests::settings"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.snapshots.support",
      kind: "rust-domain",
      summary: "Bounded snapshot parameters, paths, filesystem writes, hashes, and time helpers",
      inputs: [
        "crates/licoup-native/src/domain/conversation/snapshots/support.rs",
        "crates/licoup-native/src/domain/conversation/snapshots/tests/support.rs",
      ],
      command: rustLayer("domain::conversation::snapshots::tests::support"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.snapshots.discovery",
      kind: "rust-domain",
      summary: "Supported-agent, target-scan, and local history source discovery",
      inputs: [
        "crates/licoup-native/src/domain/conversation/snapshots/discovery.rs",
        "crates/licoup-native/src/domain/conversation/snapshots/tests/discovery.rs",
      ],
      command: rustLayer("domain::conversation::snapshots::tests::discovery"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.snapshots.selection",
      kind: "rust-domain",
      summary: "Keyword normalization, deterministic profiles, and candidate filtering",
      inputs: [
        "crates/licoup-native/src/domain/conversation/snapshots/selection.rs",
        "crates/licoup-native/src/domain/conversation/snapshots/tests/selection.rs",
      ],
      command: rustLayer("domain::conversation::snapshots::tests::selection"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.snapshots.selection-plan",
      kind: "rust-domain",
      summary: "All-conversation and exact-keyword backup preview and collection plans",
      inputs: [
        "crates/licoup-native/src/domain/conversation/snapshots/selection_plan.rs",
        "crates/licoup-native/src/domain/conversation/snapshots/tests/selection_plan.rs",
      ],
      command: rustLayer("domain::conversation::snapshots::tests::selection_plan::"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.snapshots.orchestration",
      kind: "rust-domain",
      summary: "Archive collection orchestration and bounded parallel keyword fan-out",
      inputs: [
        "crates/licoup-native/src/domain/conversation/snapshots/orchestration.rs",
        "crates/licoup-native/src/domain/conversation/snapshots/tests/orchestration.rs",
      ],
      command: rustLayer("domain::conversation::snapshots::tests::orchestration"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.snapshots.privacy-projection",
      kind: "rust-domain",
      summary: "Privacy-bounded search text and archive-relative semantic evidence projection",
      inputs: [
        "crates/licoup-native/src/domain/conversation/snapshots/privacy_projection.rs",
        "crates/licoup-native/src/domain/conversation/snapshots/tests/privacy_projection.rs",
      ],
      command: rustLayer("domain::conversation::snapshots::tests::privacy_projection"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.snapshots.materialization",
      kind: "rust-domain",
      summary: "Parallel snapshot materialization and incremental archive index persistence",
      inputs: [
        "crates/licoup-native/src/domain/conversation/snapshots/materialization.rs",
        "crates/licoup-native/src/domain/conversation/snapshots/tests/materialization.rs",
      ],
      command: rustLayer("domain::conversation::snapshots::tests::materialization"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.snapshots.validation",
      kind: "rust-domain",
      summary: "Archive integrity, semantic evidence, stale records, and baseline verification",
      inputs: [
        "crates/licoup-native/src/domain/conversation/snapshots/validation.rs",
        "crates/licoup-native/src/domain/conversation/snapshots/tests/validation.rs",
      ],
      command: rustLayer("domain::conversation::snapshots::tests::validation"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.snapshots.reporting",
      kind: "rust-domain",
      summary: "Archive reports, Markdown indexes, summaries, and workflow diagnostics",
      inputs: [
        "crates/licoup-native/src/domain/conversation/snapshots/reporting.rs",
        "crates/licoup-native/src/domain/conversation/snapshots/tests/reporting.rs",
      ],
      command: rustLayer("domain::conversation::snapshots::tests::reporting"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.snapshot-codec",
      kind: "rust-domain",
      summary: "Bounded raw conversation export and native-session filtering",
      inputs: [
        "crates/licoup-native/src/domain/conversation/snapshot_codec.rs",
      ],
      command: rustLayer("domain::conversation::snapshot_codec::tests"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.snapshot-collection",
      kind: "rust-domain",
      summary: "Snapshot collection schema, index records, matches, and summaries",
      inputs: [
        "crates/licoup-native/src/domain/conversation/snapshot_collection.rs",
      ],
      command: rustLayer("domain::conversation::snapshot_collection::tests"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.snapshot-content",
      kind: "rust-domain",
      summary: "Conversation-content classification for archive eligibility",
      inputs: [
        "crates/licoup-native/src/domain/conversation/snapshot_content.rs",
      ],
      command: rustLayer("domain::conversation::snapshot_content::tests"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.snapshot-identity",
      kind: "rust-domain",
      summary: "Stable native snapshot identity and session-aware JSON filtering",
      inputs: [
        "crates/licoup-native/src/domain/conversation/snapshot_identity.rs",
      ],
      command: rustLayer("domain::conversation::snapshot_identity::tests"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.archive-jobs.composition",
      kind: "rust-domain",
      summary: "Thin local conversation archive-job facade and public commands",
      inputs: [
        "crates/licoup-native/src/domain/conversation_archive_jobs.rs",
        "crates/licoup-native/src/domain/conversation_archive_jobs/commands.rs",
        "crates/licoup-native/src/domain/conversation_archive_jobs/constants.rs",
        "crates/licoup-native/src/domain/conversation_archive_jobs/tests/mod.rs",
      ],
      command: rustLayer("domain::conversation_archive_jobs::tests::create"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.archive-jobs.request",
      kind: "rust-domain",
      summary: "Archive request normalization and explicit local-filesystem path boundary",
      inputs: [
        "crates/licoup-native/src/domain/conversation_archive_jobs/request.rs",
        "crates/licoup-native/src/domain/conversation_archive_jobs/tests/request.rs",
      ],
      command: rustLayer("domain::conversation_archive_jobs::tests::request"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.archive-jobs.plan",
      kind: "rust-domain",
      summary: "Content-bound local backup preview and create authorization",
      inputs: [
        "crates/licoup-native/src/domain/conversation_archive_jobs/plan.rs",
        "crates/licoup-native/src/domain/conversation_archive_jobs/tests/create.rs",
      ],
      command: rustLayer(
        "domain::conversation_archive_jobs::tests::create::create_requires_the_exact_preview_binding",
      ),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.archive-jobs.activity",
      kind: "rust-domain",
      summary: "Selected-state-root local archive activity records",
      inputs: [
        "crates/licoup-native/src/domain/conversation_archive_jobs/activity.rs",
        "crates/licoup-native/src/domain/conversation_archive_jobs/tests/activity.rs",
      ],
      command: rustLayer("domain::conversation_archive_jobs::tests::activity"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.archive-jobs.creation",
      kind: "rust-domain",
      summary: "One target scan and durable queued-job creation",
      inputs: [
        "crates/licoup-native/src/domain/conversation_archive_jobs/creation.rs",
        "crates/licoup-native/src/domain/conversation_archive_jobs/tests/create.rs",
      ],
      command: rustLayer("domain::conversation_archive_jobs::tests::create"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.archive-jobs.store-queries",
      kind: "rust-domain",
      summary: "SQLite schema, job/event persistence, response projection, and reopen queries",
      inputs: [
        "crates/licoup-native/src/domain/conversation_archive_jobs/store/mod.rs",
        "crates/licoup-native/src/domain/conversation_archive_jobs/store/schema.rs",
        "crates/licoup-native/src/domain/conversation_archive_jobs/store/jobs.rs",
        "crates/licoup-native/src/domain/conversation_archive_jobs/store/events.rs",
        "crates/licoup-native/src/domain/conversation_archive_jobs/queries.rs",
        "crates/licoup-native/src/domain/conversation_archive_jobs/tests/reopen.rs",
      ],
      command: rustLayer("domain::conversation_archive_jobs::tests::reopen"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.archive-jobs.drain",
      kind: "rust-domain",
      summary: "Bounded oldest-first local queue draining",
      inputs: [
        "crates/licoup-native/src/domain/conversation_archive_jobs/drain.rs",
        "crates/licoup-native/src/domain/conversation_archive_jobs/tests/drain.rs",
      ],
      command: rustLayer("domain::conversation_archive_jobs::tests::drain"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.archive-jobs.execution",
      kind: "rust-domain",
      summary: "Local archive-and-verify execution state machine",
      inputs: [
        "crates/licoup-native/src/domain/conversation_archive_jobs/execution.rs",
        "crates/licoup-native/src/domain/conversation_archive_jobs/tests/execution.rs",
      ],
      command: rustLayer("domain::conversation_archive_jobs::tests::execution"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.archive-jobs.retry",
      kind: "rust-domain",
      summary: "Bounded retry scheduling and terminal dead-letter-style failure",
      inputs: [
        "crates/licoup-native/src/domain/conversation_archive_jobs/clock.rs",
        "crates/licoup-native/src/domain/conversation_archive_jobs/retry.rs",
        "crates/licoup-native/src/domain/conversation_archive_jobs/tests/retry.rs",
      ],
      command: rustLayer("domain::conversation_archive_jobs::tests::retry"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.archive-jobs.cancel",
      kind: "rust-domain",
      summary: "Explicit terminal cancellation excluded from later draining",
      inputs: [
        "crates/licoup-native/src/domain/conversation_archive_jobs/retry.rs",
        "crates/licoup-native/src/domain/conversation_archive_jobs/tests/cancel.rs",
      ],
      command: rustLayer("domain::conversation_archive_jobs::tests::cancel"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.archive-jobs.validation",
      kind: "rust-domain",
      summary: "Archive validation aggregation and stable response projection",
      inputs: [
        "crates/licoup-native/src/domain/conversation_archive_jobs/projection.rs",
        "crates/licoup-native/src/domain/conversation_archive_jobs/validation.rs",
        "crates/licoup-native/src/domain/conversation_archive_jobs/tests/validation.rs",
      ],
      command: rustLayer("domain::conversation_archive_jobs::tests::validation"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.archive-jobs.support",
      kind: "rust-domain",
      summary: "Shared isolated local fixture closure for archive-job leaves",
      inputs: [
        "crates/licoup-native/src/domain/conversation_archive_jobs/tests/support.rs",
      ],
      command: rustLayer("domain::conversation_archive_jobs::tests"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.archive-queue",
      kind: "rust-domain",
      summary: "Bounded in-process conversation archive task queue",
      inputs: [
        "crates/licoup-native/src/domain/conversation/archive_queue.rs",
      ],
      command: rustLayer("domain::conversation::archive_queue::tests"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.semantic.composition",
      kind: "rust-domain",
      summary: "Thin semantic conversation facade and public document/timeline composition",
      inputs: [
        "crates/licoup-native/src/domain/conversation_semantic.rs",
        "crates/licoup-native/src/domain/conversation_semantic/tests/mod.rs",
        "crates/licoup-native/src/domain/conversation_semantic/tests/composition.rs",
      ],
      command: rustLayer("domain::conversation_semantic::tests::composition::"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.semantic.model",
      kind: "rust-domain",
      summary: "Semantic schema constants, audit input, and explicit message-layer annotation",
      inputs: [
        "crates/licoup-native/src/domain/conversation_semantic/model.rs",
        "crates/licoup-native/src/domain/conversation_semantic/tests/model.rs",
      ],
      command: rustLayer("domain::conversation_semantic::tests::model::"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.semantic.builder",
      kind: "rust-domain",
      summary: "Canonical semantic layer assembly, evidence construction, and timeline composition",
      inputs: [
        "crates/licoup-native/src/domain/conversation_semantic/builder.rs",
        "crates/licoup-native/src/domain/conversation_semantic/tests/builder.rs",
      ],
      command: rustLayer("domain::conversation_semantic::tests::builder::"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.semantic.thread-projection",
      kind: "rust-domain",
      summary: "Thread role normalization, native wire projection, and ordered timeline mapping",
      inputs: [
        "crates/licoup-native/src/domain/conversation_semantic/thread_projection.rs",
        "crates/licoup-native/src/domain/conversation_semantic/tests/thread_projection.rs",
      ],
      command: rustLayer("domain::conversation_semantic::tests::thread_projection::"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.semantic.execution-projection",
      kind: "rust-domain",
      summary: "Execution card classification, bounded wire metadata, and collapsed timeline mapping",
      inputs: [
        "crates/licoup-native/src/domain/conversation_semantic/execution_projection.rs",
        "crates/licoup-native/src/domain/conversation_semantic/tests/execution_projection.rs",
      ],
      command: rustLayer("domain::conversation_semantic::tests::execution_projection::"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.semantic.artifact-projection",
      kind: "rust-domain",
      summary: "Minimal semantic artifact reference projection",
      inputs: [
        "crates/licoup-native/src/domain/conversation_semantic/artifact_projection.rs",
        "crates/licoup-native/src/domain/conversation_semantic/tests/artifact_projection.rs",
      ],
      command: rustLayer("domain::conversation_semantic::tests::artifact_projection::"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.semantic.validation",
      kind: "rust-domain",
      summary: "Fail-closed semantic schema and default-view leakage validation",
      inputs: [
        "crates/licoup-native/src/domain/conversation_semantic/validation.rs",
        "crates/licoup-native/src/domain/conversation_semantic/tests/validation.rs",
      ],
      command: rustLayer("domain::conversation_semantic::tests::validation::"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.semantic.privacy",
      kind: "rust-domain",
      summary: "Default-view token, path, context, and tool-payload privacy sanitation",
      inputs: [
        "crates/licoup-native/src/domain/conversation_semantic/privacy.rs",
        "crates/licoup-native/src/domain/conversation_semantic/tests/privacy.rs",
      ],
      command: rustLayer("domain::conversation_semantic::tests::privacy::"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.semantic.markdown",
      kind: "rust-domain",
      summary: "Semantic thread, execution, artifact, audit, and raw Markdown rendering",
      inputs: [
        "crates/licoup-native/src/domain/conversation_semantic/markdown.rs",
        "crates/licoup-native/src/domain/conversation_semantic/tests/markdown.rs",
      ],
      command: rustLayer("domain::conversation_semantic::tests::markdown::"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.semantic.io",
      kind: "rust-domain",
      summary: "Validated fixture loading and semantic JSON/Markdown materialization",
      inputs: [
        "crates/licoup-native/src/domain/conversation_semantic/io.rs",
        "crates/licoup-native/src/domain/conversation_semantic/tests/io.rs",
      ],
      command: rustLayer("domain::conversation_semantic::tests::io::"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.event-semantic",
      kind: "rust-domain",
      summary: "Conversation event semantic classification",
      inputs: [
        "crates/licoup-native/src/domain/conversation/event_semantics.rs",
      ],
      command: rustLayer("domain::conversation::event_semantics::tests"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.streaming",
      kind: "rust-domain",
      summary: "Bounded local conversation streaming and process event delivery",
      inputs: [
        "crates/licoup-native/src/domain/conversation/streaming.rs",
      ],
      command: rustLayer("domain::conversation::streaming"),
    }),
  defineModule({
      id: "rust.domain.agent-conversations.usage",
      kind: "rust-domain",
      summary: "Conversation-local token usage extraction and bounded normalization",
      inputs: [
        "crates/licoup-statistics/src/normalize.rs",
      ],
      command: statisticsLayer("normalize::tests::"),
    }),
  defineModule({
      id: "rust.domain.peer-ingress",
      kind: "rust-domain",
      summary: "Verified peer unit mapping, provenance, duplicate suppression, and canonical conversation ingress",
      inputs: [
        "crates/licoup-native/src/domain/client_conversation/peer_ingress/**",
        "crates/licoup-native/src/domain/mobile_relay/endpoint_transport/**",
        "crates/licoup-native/tests/peer_ingress/**",
      ],
      command: rustIntegrationTest("peer_ingress"),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.configuration",
      kind: "rust-domain",
      summary: "Local-only Mobile Relay configuration and bounded identifiers",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay.rs",
        "crates/licoup-native/src/domain/mobile_relay/config.rs",
        "crates/licoup-native/src/domain/mobile_relay/support.rs",
        "crates/licoup-native/src/domain/mobile_relay/tests.rs",
        "crates/licoup-native/src/domain/mobile_relay/tests/test_support.rs",
      ],
      command: rustLayer("mobile_relay::config"),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.pairing",
      kind: "rust-domain",
      summary: "Directly approved endpoint pairing and invitation policy",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay.rs",
        "crates/licoup-native/src/domain/mobile_relay/support.rs",
        "crates/licoup-native/src/domain/mobile_relay/tests.rs",
        "crates/licoup-native/src/domain/mobile_relay/tests/test_support.rs",
        "crates/licoup-native/src/domain/mobile_relay/pairing.rs",
        "crates/licoup-native/src/domain/mobile_relay/tests/pairing.rs",
      ],
      command: rustLayer("pairing"),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.pairwise-session",
      kind: "rust-domain",
      summary: "Pairwise session facade and aggregate split regression",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/pairwise_session.rs",
        "crates/licoup-native/src/domain/mobile_relay/pairwise_session/tests.rs",
      ],
      command: rustLayer("domain::mobile_relay::pairwise_session::tests::"),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.pairwise-session.scenarios",
      kind: "rust-domain",
      summary: "End-to-end pairwise session lifecycle scenarios",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay.rs",
        "crates/licoup-native/src/domain/mobile_relay/support.rs",
        "crates/licoup-native/src/domain/mobile_relay/tests.rs",
        "crates/licoup-native/src/domain/mobile_relay/tests/test_support.rs",
        "crates/licoup-native/src/domain/mobile_relay/tests/pairwise_session.rs",
      ],
      command: rustLayer("domain::mobile_relay::tests::pairwise_session::"),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.pairwise-session.status-projection",
      kind: "rust-domain",
      summary: "Authorized durable-session and capability status projection",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/pairwise_session/status_projection.rs",
        "crates/licoup-native/src/domain/mobile_relay/pairwise_session/tests/status_projection.rs",
      ],
      command: rustLayer("domain::mobile_relay::pairwise_session::tests::status_projection::"),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.pairwise-session.response-replay",
      kind: "rust-domain",
      summary: "Result response redaction and ratchet replay proof",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/pairwise_session/response.rs",
        "crates/licoup-native/src/domain/mobile_relay/pairwise_session/tests/response.rs",
      ],
      command: rustLayer("domain::mobile_relay::pairwise_session::tests::response::"),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.pairwise-session.payload",
      kind: "rust-domain",
      summary: "Bound command payload and authorization context construction",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/pairwise_session/payload.rs",
        "crates/licoup-native/src/domain/mobile_relay/pairwise_session/tests/payload.rs",
      ],
      command: rustLayer("domain::mobile_relay::pairwise_session::tests::payload::"),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.pairwise-session.crypto-operation",
      kind: "rust-domain",
      summary: "Directory-gated ciphertext seal and open operations",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/pairwise_session/crypto_operation.rs",
        "crates/licoup-native/src/domain/mobile_relay/pairwise_session/tests/crypto_operation.rs",
      ],
      command: rustLayer("domain::mobile_relay::pairwise_session::tests::crypto_operation::"),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.pairwise-session.transaction",
      kind: "rust-domain",
      summary: "Single authorized pairwise session transaction and atomic commit",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/pairwise_session/transaction.rs",
        "crates/licoup-native/src/domain/mobile_relay/pairwise_session/tests/transaction.rs",
      ],
      command: rustLayer("domain::mobile_relay::pairwise_session::tests::transaction::"),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.pairwise-session.handshake",
      kind: "rust-domain",
      summary: "PQXDH initiate accept and capability-proof handshake bootstrap",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/pairwise_session/handshake.rs",
        "crates/licoup-native/src/domain/mobile_relay/pairwise_session/tests/handshake.rs",
      ],
      command: rustLayer("domain::mobile_relay::pairwise_session::tests::handshake::"),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.pairwise-session.store",
      kind: "rust-domain",
      summary: "Durable pairwise store path secret backend restart purge and reset",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/pairwise_session/store.rs",
        "crates/licoup-native/src/domain/mobile_relay/pairwise_session/tests/store.rs",
      ],
      command: rustLayer("domain::mobile_relay::pairwise_session::tests::store::"),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.relay-operations",
      kind: "rust-domain",
      summary: "Relay operations facade and aggregate split regression",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/relay_operations.rs",
        "crates/licoup-native/src/domain/mobile_relay/relay_operations/tests.rs",
      ],
      command: rustLayer("domain::mobile_relay::relay_operations::tests::"),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.relay-operations.scenario.session-device-restore",
      kind: "rust-domain",
      summary: "Read-only session binding, authority reset CAS, and selected-device secret restoration",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/tests/relay_operations.rs",
        "crates/licoup-native/src/domain/mobile_relay/tests/relay_operations/session_device_restore.rs",
      ],
      command: rustLayer(
        "domain::mobile_relay::tests::relay_operations::session_device_restore::",
      ),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.relay-operations.scenario.envelope-roundtrip",
      kind: "rust-domain",
      summary: "Transport-only envelope validation, encrypted round trips, and file metadata boundaries",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/tests/relay_operations/envelope_roundtrip.rs",
      ],
      command: rustLayer(
        "domain::mobile_relay::tests::relay_operations::envelope_roundtrip::",
      ),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.relay-operations.scenario.identity-replay-safety",
      kind: "rust-domain",
      summary: "Pinned identity, tamper rejection, redacted errors, and replay protection",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/tests/relay_operations/identity_replay_safety.rs",
      ],
      command: rustLayer(
        "domain::mobile_relay::tests::relay_operations::identity_replay_safety::",
      ),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.relay-operations.scenario.local-result-authorization",
      kind: "rust-domain",
      summary: "Local confirmation, secure result handling, and single-operation authorization batches",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/tests/relay_operations/local_result_authorization.rs",
      ],
      command: rustLayer(
        "domain::mobile_relay::tests::relay_operations::local_result_authorization::",
      ),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.relay-operations.command-handlers",
      kind: "rust-domain",
      summary: "Ciphertext-only lease poll send receive delete and result command handlers",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/relay_operations/command_handlers.rs",
        "crates/licoup-native/src/domain/mobile_relay/relay_operations/command_handlers/check_in.rs",
        "crates/licoup-native/src/domain/mobile_relay/relay_operations/command_handlers/create.rs",
        "crates/licoup-native/src/domain/mobile_relay/relay_operations/command_handlers/poll.rs",
        "crates/licoup-native/src/domain/mobile_relay/relay_operations/command_handlers/result.rs",
      ],
      command: rustLayer("domain::mobile_relay::relay_operations::tests::"),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.relay-operations.station",
      kind: "rust-domain",
      summary: "Explicit BadTower station selection and untrusted transport-hint projection",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/relay_operations/station.rs",
        "crates/licoup-native/src/domain/mobile_relay/relay_operations/tests/station.rs",
      ],
      command: rustLayer("domain::mobile_relay::relay_operations::tests::station::"),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.relay-operations.mailbox",
      kind: "rust-domain",
      summary: "Pairwise mailbox schedule token and rotation epoch",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/relay_operations/mailbox.rs",
        "crates/licoup-native/src/domain/mobile_relay/relay_operations/tests/mailbox.rs",
      ],
      command: rustLayer("domain::mobile_relay::relay_operations::tests::mailbox::"),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.relay-operations.envelope",
      kind: "rust-domain",
      summary: "Canonical bounded encrypted relay envelope validation and codec",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/relay_operations/envelope.rs",
        "crates/licoup-native/src/domain/mobile_relay/relay_operations/tests/envelope.rs",
      ],
      command: rustLayer("domain::mobile_relay::relay_operations::tests::envelope::"),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.relay-operations.delivery",
      kind: "rust-domain",
      summary: "Exact outer-envelope delivery and local command conversion",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/relay_operations/delivery.rs",
        "crates/licoup-native/src/domain/mobile_relay/relay_operations/tests/delivery.rs",
      ],
      command: rustLayer("domain::mobile_relay::relay_operations::tests::delivery::"),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.relay-operations.status",
      kind: "rust-domain",
      summary: "Authorization-aware redacted E2EE readiness projection",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/relay_operations/status.rs",
        "crates/licoup-native/src/domain/mobile_relay/relay_operations/tests/status.rs",
      ],
      command: rustLayer("domain::mobile_relay::relay_operations::tests::status::"),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.relay-operations.allow-list",
      kind: "rust-domain",
      summary: "Canonical packaged-agent and detected runtime-send allow-list",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/relay_operations/allow_list.rs",
        "crates/licoup-native/src/domain/mobile_relay/relay_operations/tests/allow_list.rs",
      ],
      command: rustLayer("domain::mobile_relay::relay_operations::tests::allow_list::"),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.command-sync",
      kind: "rust-domain",
      summary: "Bounded encrypted command synchronization state",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay.rs",
        "crates/licoup-native/src/domain/mobile_relay/command_sync.rs",
      ],
      command: rustLayer("mobile_relay::command_sync"),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.endpoint-trust",
      kind: "rust-domain",
      summary: "Endpoint trust facade and public redacted projection",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/endpoint_trust.rs",
        "crates/licoup-native/src/domain/mobile_relay/endpoint_trust/tests.rs",
      ],
      command: rustLayer("domain::mobile_relay::endpoint_trust::tests::"),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.endpoint-trust.directory-transparency",
      kind: "rust-domain",
      summary: "Directory transparency facade and aggregate authorization regression",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/endpoint_trust/directory_transparency.rs",
        "crates/licoup-native/src/domain/mobile_relay/endpoint_trust/directory_transparency/authorization.rs",
        "crates/licoup-native/src/domain/mobile_relay/endpoint_trust/directory_transparency/tests.rs",
        "crates/licoup-native/src/domain/mobile_relay/endpoint_trust/directory_transparency/tests/support.rs",
      ],
      command: rustLayer(
        "domain::mobile_relay::endpoint_trust::directory_transparency::tests::",
      ),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.endpoint-trust.directory-transparency.claim",
      kind: "rust-domain",
      summary: "Canonical local directory claim and pairwise prekey bundle codec",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/endpoint_trust/directory_transparency/claim.rs",
        "crates/licoup-native/src/domain/mobile_relay/endpoint_trust/directory_transparency/tests/claim.rs",
      ],
      command: rustLayer(
        "domain::mobile_relay::endpoint_trust::directory_transparency::tests::claim::",
      ),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.endpoint-trust.directory-transparency.config-purpose",
      kind: "rust-domain",
      summary: "Pinned verifier configuration, scope commitment, and publication purpose",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/endpoint_trust/directory_transparency/config.rs",
        "crates/licoup-native/src/domain/mobile_relay/endpoint_trust/directory_transparency/tests/config.rs",
      ],
      command: rustLayer(
        "domain::mobile_relay::endpoint_trust::directory_transparency::tests::config::",
      ),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.endpoint-trust.directory-transparency.clock",
      kind: "rust-domain",
      summary: "Scoped key-transparency freshness clock",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/endpoint_trust/directory_transparency/clock.rs",
        "crates/licoup-native/src/domain/mobile_relay/endpoint_trust/directory_transparency/tests/clock.rs",
      ],
      command: rustLayer(
        "domain::mobile_relay::endpoint_trust::directory_transparency::tests::clock::",
      ),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.endpoint-trust.directory-transparency.freshness",
      kind: "rust-domain",
      summary: "Current active pairwise directory receipt reduction",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/endpoint_trust/directory_transparency/freshness.rs",
        "crates/licoup-native/src/domain/mobile_relay/endpoint_trust/directory_transparency/tests/freshness.rs",
      ],
      command: rustLayer(
        "domain::mobile_relay::endpoint_trust::directory_transparency::tests::freshness::",
      ),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.endpoint-trust.directory-transparency.verifier",
      kind: "rust-domain",
      summary: "Central KT response preparation and exact pairwise or MLS request binding",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/endpoint_trust/directory_transparency/verifier.rs",
        "crates/licoup-native/src/domain/mobile_relay/endpoint_trust/directory_transparency/ensure.rs",
        "crates/licoup-native/src/domain/mobile_relay/endpoint_trust/directory_transparency/tests/verifier.rs",
      ],
      command: rustLayer(
        "domain::mobile_relay::endpoint_trust::directory_transparency::tests::verifier::",
      ),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.endpoint-trust.directory-transparency.authority-open",
      kind: "rust-domain",
      summary: "Fail-closed pinned directory authority open",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/endpoint_trust/directory_transparency/authority.rs",
        "crates/licoup-native/src/domain/mobile_relay/endpoint_trust/directory_transparency/tests/authority.rs",
      ],
      command: rustLayer(
        "domain::mobile_relay::endpoint_trust::directory_transparency::tests::authority::",
      ),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.endpoint-trust.directory-transparency.peer-authorization",
      kind: "rust-domain",
      summary: "Peer descriptor key-transparency authorization",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/endpoint_trust/directory_transparency/authorization/peer.rs",
        "crates/licoup-native/src/domain/mobile_relay/endpoint_trust/directory_transparency/tests/peer_authorization.rs",
      ],
      command: rustLayer(
        "domain::mobile_relay::endpoint_trust::directory_transparency::tests::peer_authorization::",
      ),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.endpoint-trust.directory-transparency.local-authorization",
      kind: "rust-domain",
      summary: "Local endpoint key-transparency authorization",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/endpoint_trust/directory_transparency/authorization/local.rs",
        "crates/licoup-native/src/domain/mobile_relay/endpoint_trust/directory_transparency/tests/local_authorization.rs",
      ],
      command: rustLayer(
        "domain::mobile_relay::endpoint_trust::directory_transparency::tests::local_authorization::",
      ),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.endpoint-trust.directory-transparency.exact-authorization",
      kind: "rust-domain",
      summary: "Exact local directory claim authorization",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/endpoint_trust/directory_transparency/authorization/exact.rs",
        "crates/licoup-native/src/domain/mobile_relay/endpoint_trust/directory_transparency/tests/exact_authorization.rs",
      ],
      command: rustLayer(
        "domain::mobile_relay::endpoint_trust::directory_transparency::tests::exact_authorization::",
      ),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.endpoint-trust.directory-transparency.test-authority",
      kind: "rust-domain",
      summary: "Test-only isolated local KT authority and fresh-response simulation",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/endpoint_trust/directory_transparency/test_support.rs",
        "crates/licoup-native/src/domain/mobile_relay/endpoint_trust/directory_transparency/tests/test_support.rs",
      ],
      command: rustLayer(
        "domain::mobile_relay::endpoint_trust::directory_transparency::tests::test_support::",
      ),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.endpoint-trust.local-material",
      kind: "rust-domain",
      summary: "Local endpoint material facade, composition, and aggregate regression",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/endpoint_trust/local_material.rs",
        "crates/licoup-native/src/domain/mobile_relay/endpoint_trust/local_material/state.rs",
        "crates/licoup-native/src/domain/mobile_relay/endpoint_trust/local_material/tests.rs",
      ],
      command: rustLayer(
        "domain::mobile_relay::endpoint_trust::local_material::tests::",
      ),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.endpoint-trust.local-material.identity",
      kind: "rust-domain",
      summary: "Local endpoint identity generation, data-home binding, and replacement-import refusal",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/endpoint_trust/local_material/composition.rs",
        "crates/licoup-native/src/domain/mobile_relay/endpoint_trust/local_material/identity_generation.rs",
        "crates/licoup-native/src/domain/mobile_relay/endpoint_trust/local_material/material_mutation.rs",
        "crates/licoup-native/src/domain/mobile_relay/endpoint_trust/local_material/tests/generation.rs",
      ],
      command: rustLayer(
        "domain::mobile_relay::endpoint_trust::local_material::tests::generation::",
      ),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.endpoint-trust.local-material.prekey-inventory",
      kind: "rust-domain",
      summary: "PQXDH curve and ML-KEM prekey generation and inventory mutation",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/endpoint_trust/local_material/prekey_generation.rs",
        "crates/licoup-native/src/domain/mobile_relay/endpoint_trust/local_material/prekey_inventory.rs",
        "crates/licoup-native/src/domain/mobile_relay/endpoint_trust/local_material/tests/inventory.rs",
      ],
      command: rustLayer(
        "domain::mobile_relay::endpoint_trust::local_material::tests::inventory::",
      ),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.endpoint-trust.local-material.rotation",
      kind: "rust-domain",
      summary: "One-time prekey and repair-only local identity rotation",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/endpoint_trust/local_material/rotation.rs",
        "crates/licoup-native/src/domain/mobile_relay/endpoint_trust/local_material/tests/rotation.rs",
      ],
      command: rustLayer(
        "domain::mobile_relay::endpoint_trust::local_material::tests::rotation::",
      ),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.endpoint-trust.local-material.protocol-reset",
      kind: "rust-domain",
      summary: "Fail-closed reset for protocol-incompatible local pairwise state",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/endpoint_trust/local_material/protocol_reset.rs",
        "crates/licoup-native/src/domain/mobile_relay/endpoint_trust/local_material/tests/protocol_reset.rs",
      ],
      command: rustLayer(
        "domain::mobile_relay::endpoint_trust::local_material::tests::protocol_reset::",
      ),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.endpoint-trust.local-material.state-codec",
      kind: "rust-domain",
      summary: "Fail-closed local endpoint state codec and fingerprint projection",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/endpoint_trust/local_material/state_codec.rs",
        "crates/licoup-native/src/domain/mobile_relay/endpoint_trust/local_material/tests/state_codec.rs",
      ],
      command: rustLayer(
        "domain::mobile_relay::endpoint_trust::local_material::tests::state_codec::",
      ),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.endpoint-trust.local-material.descriptor-accessors",
      kind: "rust-domain",
      summary: "Secret-free local descriptor projection and typed key accessors",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/endpoint_trust/local_material/accessors.rs",
        "crates/licoup-native/src/domain/mobile_relay/endpoint_trust/local_material/descriptor.rs",
        "crates/licoup-native/src/domain/mobile_relay/endpoint_trust/local_material/tests/descriptor.rs",
      ],
      command: rustLayer(
        "domain::mobile_relay::endpoint_trust::local_material::tests::descriptor::",
      ),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.endpoint-trust.pairing-presentation",
      kind: "rust-domain",
      summary: "Secret-free endpoint pairing invitation projection",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/endpoint_trust/pairing_presentation.rs",
      ],
      command: rustLayer(
        "domain::mobile_relay::endpoint_trust::pairing_presentation::tests::",
      ),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.endpoint-trust.pairwise-codec",
      kind: "rust-domain",
      summary: "Canonical pairwise endpoint identity codec",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/endpoint_trust/pairwise_codec.rs",
      ],
      command: rustLayer(
        "domain::mobile_relay::endpoint_trust::pairwise_codec::tests::",
      ),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.endpoint-trust.peer-trust",
      kind: "rust-domain",
      summary: "Peer trust rotation and continuity policy",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/endpoint_trust/peer_trust.rs",
      ],
      command: rustLayer(
        "domain::mobile_relay::endpoint_trust::peer_trust::tests::",
      ),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.endpoint-trust.persistence",
      kind: "rust-domain",
      summary: "Stable endpoint trust persistence and scoped removal",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/endpoint_trust/persistence.rs",
      ],
      command: rustLayer(
        "domain::mobile_relay::endpoint_trust::persistence::tests::",
      ),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.endpoint-trust.primitives",
      kind: "rust-domain",
      summary: "Canonical endpoint trust digests and base64url primitives",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/endpoint_trust/primitives.rs",
      ],
      command: rustLayer(
        "domain::mobile_relay::endpoint_trust::primitives::tests::",
      ),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.endpoint-trust.scenarios",
      kind: "rust-domain",
      summary: "Cross-boundary endpoint trust scenarios",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/tests/endpoint_trust.rs",
      ],
      command: rustLayer("domain::mobile_relay::tests::endpoint_trust::"),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.key-transparency",
      kind: "rust-domain",
      summary: "Key-transparency action contract, dispatcher, facade, and aggregate regression",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay.rs",
        "crates/licoup-native/src/domain/mobile_relay/key_transparency.rs",
        "crates/licoup-native/src/domain/mobile_relay/key_transparency/contract.rs",
        "crates/licoup-native/src/domain/mobile_relay/key_transparency/dispatcher.rs",
        "crates/licoup-native/src/domain/mobile_relay/key_transparency/tests.rs",
        "crates/licoup-native/src/domain/mobile_relay/key_transparency/tests/dispatcher.rs",
      ],
      command: rustLayer("domain::mobile_relay::key_transparency::tests::"),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.key-transparency.authority",
      kind: "rust-domain",
      summary: "Pinned authority proposal, persisted challenge, transactional confirmation, and destructive reset",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/key_transparency/authority.rs",
        "crates/licoup-native/src/domain/mobile_relay/key_transparency/authority/challenge.rs",
        "crates/licoup-native/src/domain/mobile_relay/key_transparency/authority/proposal.rs",
        "crates/licoup-native/src/domain/mobile_relay/key_transparency/authority/reset.rs",
        "crates/licoup-native/src/domain/mobile_relay/key_transparency/authority/transaction.rs",
        "crates/licoup-native/src/domain/mobile_relay/key_transparency/persistence.rs",
        "crates/licoup-native/src/domain/mobile_relay/key_transparency/projection.rs",
        "crates/licoup-native/src/domain/mobile_relay/key_transparency/tests/authority.rs",
      ],
      command: rustLayer("domain::mobile_relay::key_transparency::tests::authority::"),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.key-transparency.publication",
      kind: "rust-domain",
      summary: "Exact local directory publication claim and derived authorization purpose",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/key_transparency/publication.rs",
        "crates/licoup-native/src/domain/mobile_relay/key_transparency/tests/publication.rs",
      ],
      command: rustLayer("domain::mobile_relay::key_transparency::tests::publication::"),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.key-transparency.revocation",
      kind: "rust-domain",
      summary: "Explicitly confirmed directory revocation claim",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/key_transparency/revocation.rs",
        "crates/licoup-native/src/domain/mobile_relay/key_transparency/tests/revocation.rs",
      ],
      command: rustLayer("domain::mobile_relay::key_transparency::tests::revocation::"),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.key-transparency.provision",
      kind: "rust-domain",
      summary: "Exact pending-claim service response authorization and committed projection",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/key_transparency/provision.rs",
        "crates/licoup-native/src/domain/mobile_relay/key_transparency/tests/provision.rs",
      ],
      command: rustLayer("domain::mobile_relay::key_transparency::tests::provision::"),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.key-transparency.monitor-gossip",
      kind: "rust-domain",
      summary: "Self-monitor authorization plus encrypted pairwise gossip control",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/key_transparency/self_monitor.rs",
        "crates/licoup-native/src/domain/mobile_relay/key_transparency/gossip.rs",
        "crates/licoup-native/src/domain/mobile_relay/key_transparency/tests/monitor_gossip.rs",
      ],
      command: rustLayer("domain::mobile_relay::key_transparency::tests::monitor_gossip::"),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.key-transparency.status-config",
      kind: "rust-domain",
      summary: "Single secret-context adapter, generation ownership, and fail-closed public status",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/key_transparency/config.rs",
        "crates/licoup-native/src/domain/mobile_relay/key_transparency/status.rs",
        "crates/licoup-native/src/domain/mobile_relay/key_transparency/tests/status.rs",
      ],
      command: rustLayer("domain::mobile_relay::key_transparency::tests::status::"),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.secret-custody",
      kind: "rust-domain",
      summary: "Secret custody facade, redacted credential inventory, and shared helper projection",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/secret_custody.rs",
        "crates/licoup-native/src/domain/mobile_relay/secret_custody/inventory.rs",
        "crates/licoup-native/src/domain/mobile_relay/secret_custody/tests.rs",
      ],
      command: rustLayer("domain::mobile_relay::secret_custody::tests::"),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.secret-custody.cleanup",
      kind: "rust-domain",
      summary: "Explicit disposable-proof secret-store cleanup",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/secret_custody/cleanup.rs",
      ],
      command: rustLayer(
        "domain::mobile_relay::secret_custody::cleanup::tests::",
      ),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.secret-custody.config-store",
      kind: "rust-domain",
      summary: "Protected secret-custody configuration persistence",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/secret_custody/config_store.rs",
      ],
      command: rustLayer(
        "domain::mobile_relay::secret_custody::config_store::tests::",
      ),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.secret-custody.persistence",
      kind: "rust-domain",
      summary: "Native secret persistence and restart semantics",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/secret_custody/persistence.rs",
      ],
      command: rustLayer(
        "domain::mobile_relay::secret_custody::persistence::tests::",
      ),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.secret-custody.presentation",
      kind: "rust-domain",
      summary: "Redacted secret-custody status presentation",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/secret_custody/presentation.rs",
      ],
      command: rustLayer(
        "domain::mobile_relay::secret_custody::presentation::tests::",
      ),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.secret-custody.reset-guard",
      kind: "rust-domain",
      summary: "Retired-state reset and protected-operation guard",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/secret_custody/reset_guard.rs",
      ],
      command: rustLayer(
        "domain::mobile_relay::secret_custody::reset_guard::tests::",
      ),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.secret-custody.cleanup-authority",
      kind: "rust-domain",
      summary: "Scoped cleanup authorization and owned-custody deletion",
      inputs: ["crates/licoup-native/src/domain/mobile_relay/secret_custody/cleanup_authority.rs"],
      command: rustLayer("domain::mobile_relay::secret_custody::cleanup_authority::tests::"),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.secret-custody.recovery",
      kind: "rust-domain",
      summary: "Archive custody metadata preparation and authorized same-device recovery",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/secret_custody/recovery.rs",
      ],
      command: rustLayer("domain::mobile_relay::secret_custody::recovery::tests::"),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.secret-custody.runtime",
      kind: "rust-domain",
      summary: "Native-authorized custody runtime and biometric session composition",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/secret_custody/runtime.rs",
      ],
      command: rustLayer(
        "domain::mobile_relay::secret_custody::runtime::tests::",
      ),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.secret-custody.secret-material",
      kind: "rust-domain",
      summary: "Bounded secret material generation and custody handles",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/secret_custody/secret_material.rs",
        "crates/licoup-native/src/domain/mobile_relay/secret_custody/runtime_secret_material.rs",
      ],
      command: rustLayer(
        "domain::mobile_relay::secret_custody::secret_material::tests::",
      ),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.secret-custody.self-test",
      kind: "rust-domain",
      summary: "Minimal secret-custody capability readiness self-test",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/secret_custody/self_test.rs",
      ],
      command: rustLayer(
        "domain::mobile_relay::secret_custody::self_test::tests::",
      ),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.secret-custody.scenario.config-integrity",
      kind: "rust-domain",
      summary: "Fail-closed config integrity, optimistic concurrency, and public redaction",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/tests/secret_custody.rs",
        "crates/licoup-native/src/domain/mobile_relay/tests/secret_custody/config_integrity.rs",
      ],
      command: rustLayer(
        "domain::mobile_relay::tests::secret_custody::config_integrity::",
      ),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.secret-custody.scenario.native-store-boundary",
      kind: "rust-domain",
      summary: "Native secret-bundle persistence, hydration, and redacted portable state",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/tests/secret_custody/native_store_boundary.rs",
      ],
      command: rustLayer(
        "domain::mobile_relay::tests::secret_custody::native_store_boundary::",
      ),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.secret-custody.scenario.ffi-dispatcher",
      kind: "rust-domain",
      summary: "Mobile FFI public-read and authorized native secret-store boundary",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/tests/secret_custody/ffi_dispatcher.rs",
      ],
      command: rustLayer(
        "domain::mobile_relay::tests::secret_custody::ffi_dispatcher::",
      ),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.secret-custody.scenario.authorization-batches",
      kind: "rust-domain",
      summary: "User-level secret mutation and cleanup authorization batch budgets",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/tests/secret_custody/authorization_batches.rs",
      ],
      command: rustLayer(
        "domain::mobile_relay::tests::secret_custody::authorization_batches::",
      ),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.secret-custody.scenario.cleanup-authority",
      kind: "rust-domain",
      summary: "Exact-confirmation disposable cleanup, bounded deletion, and failure propagation",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/tests/secret_custody/cleanup_authority.rs",
      ],
      command: rustLayer(
        "domain::mobile_relay::tests::secret_custody::cleanup_authority::",
      ),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.secret-custody.scenario.zeroizing-boundary",
      kind: "rust-domain",
      summary: "Owned runtime secret handoff and zeroizing replacement/drop boundary",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/tests/secret_custody/zeroizing_boundary.rs",
      ],
      command: rustLayer(
        "domain::mobile_relay::tests::secret_custody::zeroizing_boundary::",
      ),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.secret-custody.scenario.public-config-restore",
      kind: "rust-domain",
      summary: "Public config save, selected-device restoration, and runtime override rejection",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/tests/secret_custody/public_config_restore.rs",
      ],
      command: rustLayer(
        "domain::mobile_relay::tests::secret_custody::public_config_restore::",
      ),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.secret-custody.scenario.e2ee-status-authorization",
      kind: "rust-domain",
      summary: "Truthful E2EE status projection and bounded secret-store authorization",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/tests/secret_custody/e2ee_status_authorization.rs",
      ],
      command: rustLayer(
        "domain::mobile_relay::tests::secret_custody::e2ee_status_authorization::",
      ),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.secret-custody.scenario.secure-command-store",
      kind: "rust-domain",
      summary: "Secure command raw-secret rejection and native secret-store execution",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/tests/secret_custody/secure_command_store.rs",
      ],
      command: rustLayer(
        "domain::mobile_relay::tests::secret_custody::secure_command_store::",
      ),
    }),
  defineModule({
      id: "rust.domain.mobile-relay.badtower-acceptance",
      kind: "rust-domain",
      summary: "BadTower-backed mobile relay acceptance scenarios",
      inputs: [
        "crates/licoup-native/src/domain/mobile_relay/tests/badtower_acceptance.rs",
        "crates/licoup-native/src/domain/mobile_relay/tests/badtower_acceptance/**",
      ],
      command: rustLayer("domain::mobile_relay::tests::badtower_acceptance::"),
    }),
  defineModule({
      id: "rust.domain.targets",
      kind: "rust-domain",
      summary: "Target public facade, shared support, and test composition",
      inputs: [
        "crates/licoup-native/src/domain/targets.rs",
        "crates/licoup-agent-targets/src/domain/targets/support.rs",
        "crates/licoup-agent-targets/src/domain/targets/tests.rs",
        "crates/licoup-agent-targets/src/domain/targets/tests/test_support.rs",
      ],
      command: rustLayer("domain::targets::tests::"),
    }),
  defineModule({
      id: "rust.domain.targets.scan-paths",
      kind: "rust-domain",
      summary: "Agent Scan Path Manifest: allowlisted discovery, lexical deny, unused-agent other-app skip",
      targetEvidenceHosts: ["darwin", "linux", "win32"],
      inputs: [
        "crates/licoup-agent-targets/src/domain/targets/scan_paths.rs",
        "crates/licoup-foundation/src/platform/paths.rs",
        "crates/licoup-native/resources/agent-scan-paths.toml",
      ],
      command: rustCrateLayer("licoup-agent-targets","domain::targets::scan_paths::tests::"),
    }),
  defineModule({
      id: "rust.domain.targets.binaries",
      kind: "rust-domain",
      summary: "Bounded platform executable discovery and source classification",
      targetEvidenceHosts: ["darwin", "linux", "win32"],
      inputs: [
        "crates/licoup-agent-targets/src/domain/targets/binaries.rs",
        "crates/licoup-agent-targets/src/domain/targets/scan_paths.rs",
        "crates/licoup-native/resources/agent-scan-paths.toml",
      ],
      command: rustCrateLayer("licoup-agent-targets","domain::targets::binaries::tests::"),
    }),
  defineModule({
      id: "rust.domain.targets.catalog",
      kind: "rust-domain",
      summary: "Canonical target definitions and adapter readiness policy",
      inputs: [
        "crates/licoup-agent-targets/src/domain/targets/catalog.rs",
      ],
      command: rustCrateLayer("licoup-agent-targets","domain::targets::catalog::tests::"),
    }),
  defineModule({
      id: "rust.domain.targets.parameters",
      kind: "rust-domain",
      summary: "Bounded target command parameter parsing",
      inputs: [
        "crates/licoup-agent-targets/src/domain/targets/parameters.rs",
      ],
      command: rustCrateLayer("licoup-agent-targets","domain::targets::parameters::tests::"),
    }),
  defineModule({
      id: "rust.domain.targets.platform-paths",
      kind: "rust-domain",
      summary: "Cross-platform target configuration and evidence paths",
      targetEvidenceHosts: ["darwin", "linux", "win32"],
      inputs: [
        "crates/licoup-agent-targets/src/domain/targets/platform_paths.rs",
        "crates/licoup-agent-targets/src/domain/targets/scan_paths.rs",
        "crates/licoup-foundation/src/platform/paths.rs",
        "crates/licoup-native/resources/agent-scan-paths.toml",
      ],
      command: rustCrateLayer("licoup-agent-targets","domain::targets::platform_paths::tests::"),
    }),
  defineModule({
      id: "rust.domain.targets.processes",
      kind: "rust-domain",
      summary: "Single-snapshot running-process target detection",
      inputs: [
        "crates/licoup-agent-targets/src/domain/targets/processes.rs",
      ],
      command: rustCrateLayer("licoup-agent-targets","domain::targets::processes::tests::"),
    }),
  defineModule({
      id: "rust.domain.targets.platform-integration",
      kind: "rust-domain",
      summary: "Cross-platform path, process, and executable integration projections",
      targetEvidenceHosts: ["darwin", "linux", "win32"],
      inputs: [
        "crates/licoup-agent-targets/src/domain/targets/binaries.rs",
        "crates/licoup-agent-targets/src/domain/targets/platform_paths.rs",
        "crates/licoup-agent-targets/src/domain/targets/processes.rs",
        "crates/licoup-agent-targets/src/domain/targets/tests/platform.rs",
      ],
      command: rustCrateLayer("licoup-agent-targets","domain::targets::tests::platform::"),
    }),
  defineModule({
      id: "rust.domain.targets.probe-pool",
      kind: "rust-domain",
      summary: "Bounded ordered concurrent target probe scheduling",
      inputs: [
        "crates/licoup-agent-targets/src/domain/targets/probe_pool.rs",
      ],
      command: rustCrateLayer("licoup-agent-targets","domain::targets::probe_pool::tests::"),
    }),
  defineModule({
      id: "rust.domain.targets.discovery",
      kind: "rust-domain",
      summary: "Concurrent target discovery and inspect orchestration",
      inputs: [
        "crates/licoup-agent-targets/src/domain/targets/discovery.rs",
        "crates/licoup-agent-targets/src/domain/targets/virtual_machine_discovery.rs",
        "crates/licoup-agent-targets/src/domain/targets/tests/discovery.rs",
      ],
      command: rustCrateLayer("licoup-agent-targets","domain::targets::tests::discovery::"),
    }),
  defineModule({
      id: "rust.domain.targets.discovery-cache",
      kind: "rust-domain",
      summary: "Local quick-start target route cache without conversation or model content",
      inputs: [
        "crates/licoup-agent-targets/src/domain/targets/target_cache.rs",
      ],
      command: rustCrateLayer("licoup-agent-targets","domain::targets::target_cache::tests::"),
    }),
  defineModule({
      id: "rust.domain.targets.manual",
      kind: "rust-domain",
      summary: "Manual target persistence, normalization, and local state projection",
      inputs: [
        "crates/licoup-agent-targets/src/domain/targets/manual.rs",
        "crates/licoup-agent-targets/src/domain/targets/tests/manual.rs",
      ],
      command: rustCrateLayer("licoup-agent-targets","domain::targets::tests::manual::"),
    }),
  defineModule({
      id: "rust.domain.targets.scan-merge",
      kind: "rust-domain",
      summary: "Target evidence, capability, model-catalog, and supported-action reduction",
      inputs: [
        "crates/licoup-agent-targets/src/domain/targets/scan_merge.rs",
        "crates/licoup-agent-targets/src/domain/targets/tests/scan_merge.rs",
      ],
      command: rustCrateLayer("licoup-agent-targets","domain::targets::tests::scan_merge::"),
    }),
  defineModule({
      id: "rust.domain.targets.runtime-binding",
      kind: "rust-domain",
      summary: "Canonical ready-runtime executable binding",
      inputs: [
        "crates/licoup-agent-targets/src/domain/targets/runtime_binding.rs",
        "crates/licoup-agent-targets/src/domain/targets/tests/runtime_binding.rs",
      ],
      command: rustCrateLayer("licoup-agent-targets","domain::targets::tests::runtime_binding::"),
    }),
  defineModule({
      id: "rust.domain.targets.model-catalog",
      kind: "rust-domain",
      summary: "Local model catalog orchestration and test composition",
      inputs: [
        "crates/licoup-agent-targets/src/domain/targets/model_catalog/builtin.rs",
        "crates/licoup-agent-targets/src/domain/targets/model_catalog/builtin_catalog.json",
        "crates/licoup-agent-targets/src/domain/targets/model_catalog/deepseek.rs",
        "crates/licoup-agent-targets/src/domain/targets/model_catalog/kimi.rs",
        "crates/licoup-agent-targets/src/domain/targets/model_catalog/mod.rs",
        "crates/licoup-agent-targets/src/domain/targets/model_catalog/pi.rs",
        "crates/licoup-agent-targets/src/domain/targets/model_catalog/presentation.rs",
        "crates/licoup-agent-targets/src/domain/targets/model_catalog/tests.rs",
      ],
      command: rustCrateLayer("licoup-agent-targets","domain::targets::model_catalog::tests::"),
    }),
  defineModule({
      id: "rust.domain.targets.model-catalog.antigravity",
      kind: "rust-domain",
      summary: "Bounded Antigravity local model discovery",
      inputs: [
        "crates/licoup-agent-targets/src/domain/targets/model_catalog/antigravity.rs",
      ],
      command: rustCrateLayer("licoup-agent-targets","domain::targets::model_catalog::tests::antigravity::"),
    }),
  defineModule({
      id: "rust.domain.targets.model-catalog.cursor",
      kind: "rust-domain",
      summary: "Bounded Cursor Agent CLI model discovery",
      inputs: [
        "crates/licoup-agent-targets/src/domain/targets/model_catalog/cursor.rs",
      ],
      command: rustCrateLayer("licoup-agent-targets","domain::targets::model_catalog::tests::cursor::"),
    }),
  defineModule({
      id: "rust.domain.targets.model-catalog.config",
      kind: "rust-domain",
      summary: "Local model settings and cache document discovery",
      inputs: [
        "crates/licoup-agent-targets/src/domain/targets/model_catalog/config.rs",
      ],
      command: rustCrateLayer("licoup-agent-targets","domain::targets::model_catalog::tests::config_"),
    }),
  defineModule({
      id: "rust.domain.targets.model-catalog.history",
      kind: "rust-domain",
      summary: "Bounded local conversation history model projection",
      inputs: [
        "crates/licoup-agent-targets/src/domain/targets/model_catalog/history.rs",
      ],
      command: rustCrateLayer("licoup-agent-targets","domain::targets::model_catalog::tests::history::"),
    }),
  defineModule({
      id: "rust.domain.targets.model-catalog.kilo",
      kind: "rust-domain",
      summary: "Kilo CLI catalog with local-state fallback discovery",
      inputs: [
        "crates/licoup-agent-targets/src/domain/targets/model_catalog/kilo.rs",
      ],
      command: rustCrateLayer("licoup-agent-targets","domain::targets::model_catalog::tests::kilo::"),
    }),
  defineModule({
      id: "rust.domain.targets.model-catalog.claude-code",
      kind: "rust-domain",
      summary: "Claude Code admitted model aliases, configured providers, and allowlist projection",
      inputs: [
        "crates/licoup-agent-targets/src/domain/targets/model_catalog/claude.rs",
      ],
      command: rustCrateLayer("licoup-agent-targets","domain::targets::model_catalog::tests::claude_code::"),
    }),
  defineModule({
      id: "rust.domain.targets.model-catalog.opencode",
      kind: "rust-domain",
      summary: "OpenCode provider-scoped model catalog discovery",
      inputs: [
        "crates/licoup-agent-targets/src/domain/targets/model_catalog/opencode.rs",
      ],
      command: rustCrateLayer("licoup-agent-targets","domain::targets::model_catalog::tests::opencode::"),
    }),
  defineModule({
      id: "rust.domain.targets.model-catalog.normalization",
      kind: "rust-domain",
      summary: "Model identifiers, display names, and selectable collection normalization",
      inputs: [
        "crates/licoup-agent-targets/src/domain/targets/model_catalog/normalization.rs",
      ],
      command: rustCrateLayer("licoup-agent-targets","domain::targets::model_catalog::tests::normalization::"),
    }),
  defineModule({
      id: "rust.domain.targets.model-catalog.provider",
      kind: "rust-domain",
      summary: "Provider identity and display-label projection",
      inputs: [
        "crates/licoup-agent-targets/src/domain/targets/model_catalog/provider.rs",
      ],
      command: rustCrateLayer("licoup-agent-targets","domain::targets::model_catalog::tests::provider::"),
    }),
  defineModule({
      id: "rust.domain.targets.model-catalog.reasoning",
      kind: "rust-domain",
      summary: "Reasoning and thinking option extraction",
      inputs: [
        "crates/licoup-agent-targets/src/domain/targets/model_catalog/reasoning.rs",
      ],
      command: rustCrateLayer("licoup-agent-targets","domain::targets::model_catalog::tests::reasoning::"),
    }),
  defineModule({
      id: "rust.domain.targets.model-catalog.merge",
      kind: "rust-domain",
      summary: "Deterministic model source merge and JSON projection",
      inputs: [
        "crates/licoup-agent-targets/src/domain/targets/model_catalog/merge.rs",
      ],
      command: rustCrateLayer("licoup-agent-targets","domain::targets::model_catalog::tests::merge::"),
    }),
  defineModule({
      id: "rust.domain.skill-hub",
      kind: "rust-domain",
      summary: "Skill Hub public facade, installation orchestration, and shared contract tests",
      inputs: [
        "crates/licoup-native/src/domain/skill_hub.rs",
        "crates/licoup-native/src/domain/skill_hub/state.rs",
        "crates/licoup-native/src/domain/skill_hub/tests.rs",
        "crates/licoup-native/src/domain/skill_hub/tests/support.rs",
      ],
      command: rustLayer("domain::skill_hub::tests"),
    }),
  defineModule({
      id: "rust.domain.skill-hub.pairing-catalog",
      kind: "rust-domain",
      summary: "Local agent pairing, skill listing, visibility, pinning, and lookup catalog",
      inputs: [
        "crates/licoup-native/src/domain/skill_hub/catalog.rs",
        "crates/licoup-native/src/domain/skill_hub/pairing.rs",
        "crates/licoup-native/src/domain/skill_hub/tests/pairing_catalog.rs",
      ],
      command: rustLayer("domain::skill_hub::tests::pairing_catalog::"),
    }),
  defineModule({
      id: "rust.domain.skill-hub.package",
      kind: "rust-domain",
      summary: "Bounded skill package inspection, identifiers, file walks, and digests",
      inputs: [
        "crates/licoup-native/src/domain/skill_hub/package.rs",
      ],
      command: rustLayer("domain::skill_hub::package::tests"),
    }),
  defineModule({
      id: "rust.domain.skill-hub.discovery",
      kind: "rust-domain",
      summary: "Explicit local skill root discovery",
      inputs: [
        "crates/licoup-native/src/domain/skill_hub/discovery.rs",
      ],
      command: rustLayer("domain::skill_hub::discovery::tests"),
    }),
  defineModule({
      id: "rust.domain.skill-hub.delete",
      kind: "rust-domain",
      summary: "One-confirmation skill deletion across an explicit agent set",
      inputs: [
        "crates/licoup-native/src/domain/skill_hub/delete.rs",
      ],
      command: rustLayer("domain::skill_hub::delete::tests::"),
    }),
  defineModule({
      id: "rust.domain.skill-hub.usage",
      kind: "rust-domain",
      summary: "Real runtime skill invocation collection, concurrent daily aggregation, and bounded reports",
      inputs: [
        "crates/licoup-native/src/domain/skill_hub/usage.rs",
        "crates/licoup-native/src/domain/skill_hub/usage/**",
        "crates/licoup-native/src/ffi/commands/agent_conversation.rs",
      ],
      command: rustLayer("domain::skill_hub::usage::"),
    }),
  defineModule({
      id: "rust.domain.skill-hub.usage-backfill",
      kind: "rust-domain",
      summary: "Incremental history backfill for skill invocation counts with watermark and digest idempotency",
      inputs: [
        "crates/licoup-native/tests/skill_usage_backfill_cases.rs",
      ],
      command: rustIntegrationTest("skill_usage_backfill_cases"),
    }),
  defineModule({
      id: "rust.domain.optional-collaboration",
      kind: "rust-domain",
      summary: "Explicit-only optional collaboration source, package, and lifecycle policy",
      inputs: [
        "crates/licoup-native/src/domain/collaboration_plugin/mod.rs",
        "crates/licoup-native/src/domain/collaboration_plugin/authority.rs",
        "crates/licoup-native/src/domain/collaboration_plugin/manifest.rs",
        "crates/licoup-native/src/domain/collaboration_plugin/runner_signature.rs",
        "crates/licoup-native/src/domain/collaboration_plugin/source.rs",
        "crates/licoup-foundation/src/core/safe_archive.rs",
        "crates/licoup-foundation/src/core/safe_archive/**",
        "crates/licoup-native/src/domain/collaboration_plugin/test_support.rs",
        "crates/licoup-native/src/domain/collaboration_plugin/transaction.rs",
        "crates/licoup-native/src/domain/collaboration_plugin/lifecycle/**",
        "crates/licoup-native/src/domain/collaboration_plugin/package/**",
        "crates/licoup-native/src/domain/collaboration_plugin/workflow/mod.rs",
        "crates/licoup-native/src/domain/collaboration_plugin/workflow/commit.rs",
        "crates/licoup-native/src/domain/collaboration_plugin/workflow/model.rs",
        "crates/licoup-native/src/domain/collaboration_plugin/workflow/store.rs",
        "crates/licoup-native/src/ffi/commands/collaboration.rs",
      ],
      command: rustLayer("domain::collaboration_plugin"),
    }),
  defineModule({
      id: "rust.domain.optional-collaboration.local-server-assembly",
      kind: "rust-domain",
      summary: "Digest-bound local assembly, private operation locking, and loopback inspection runtime lifecycle",
      inputs: [
        "crates/licoup-native/src/domain/collaboration_plugin/assembly/**",
      ],
      command: rustLayer("local_server_assembly"),
    }),
  defineModule({
      id: "rust.domain.collaboration-plugin.mcp-registration",
      kind: "rust-domain",
      summary: "Explicit per-agent MCP registration binding and direct-user transfer policy",
      inputs: [
        "crates/licoup-native/src/domain/collaboration_plugin/registration.rs",
      ],
      command: rustLayer(
        "domain::collaboration_plugin::workflow::tests::mcp_partial_commit_rolls_back_payload_and_private_agent_registration",
      ),
    }),
  defineModule({
      id: "rust.domain.collaboration-plugin.mcp-bridge",
      kind: "rust-domain",
      summary: "Bounded MCP stdio bridge that can consume but never mint an exact one-shot authorization",
      inputs: [
        "crates/licoup-native/src/domain/collaboration_plugin/bridge.rs",
      ],
      command: rustLayer("domain::collaboration_plugin::bridge::tests::"),
    }),
  defineModule({
      id: "rust.domain.optional-collaboration.workflow-operations.composition",
      kind: "rust-domain",
      summary: "Thin collaboration workflow operation facade and direct-user routing",
      inputs: [
        "crates/licoup-native/src/domain/collaboration_plugin/workflow/operations.rs",
        "crates/licoup-native/src/domain/collaboration_plugin/workflow/operations/tests/mod.rs",
        "crates/licoup-native/src/domain/collaboration_plugin/workflow/operations/tests/composition.rs",
      ],
      command: rustLayer("domain::collaboration_plugin::workflow::operations::tests::composition::"),
    }),
  defineModule({
      id: "rust.domain.optional-collaboration.workflow-operations.plan-local",
      kind: "rust-domain",
      summary: "Explicit local deployment planning and exact file preview",
      inputs: [
        "crates/licoup-native/src/domain/collaboration_plugin/workflow/operations/plan_local.rs",
        "crates/licoup-native/src/domain/collaboration_plugin/workflow/operations/tests/plan_local.rs",
      ],
      command: rustLayer("domain::collaboration_plugin::workflow::operations::tests::plan_local::"),
    }),
  defineModule({
      id: "rust.domain.optional-collaboration.workflow-operations.plan-mcp",
      kind: "rust-domain",
      summary: "Explicit MCP installation planning across confirmed agent destinations",
      inputs: [
        "crates/licoup-native/src/domain/collaboration_plugin/workflow/operations/plan_mcp.rs",
        "crates/licoup-native/src/domain/collaboration_plugin/workflow/operations/tests/plan_mcp.rs",
      ],
      command: rustLayer("domain::collaboration_plugin::workflow::operations::tests::plan_mcp::"),
    }),
  defineModule({
      id: "rust.domain.optional-collaboration.workflow-operations.apply-local",
      kind: "rust-domain",
      summary: "Digest-bound one-time local deployment apply and claim settlement",
      inputs: [
        "crates/licoup-native/src/domain/collaboration_plugin/workflow/operations/apply_local.rs",
        "crates/licoup-native/src/domain/collaboration_plugin/workflow/operations/tests/apply_local.rs",
      ],
      command: rustLayer("domain::collaboration_plugin::workflow::operations::tests::apply_local::"),
    }),
  defineModule({
      id: "rust.domain.optional-collaboration.workflow-operations.apply-mcp",
      kind: "rust-domain",
      summary: "Digest-bound MCP payload and review-artifact apply across agents",
      inputs: [
        "crates/licoup-native/src/domain/collaboration_plugin/workflow/operations/apply_mcp.rs",
        "crates/licoup-native/src/domain/collaboration_plugin/workflow/operations/tests/apply_mcp.rs",
        "crates/licoup-native/src/domain/collaboration_plugin/workflow/mcp_transaction.rs",
      ],
      command: rustLayer("domain::collaboration_plugin::workflow::operations::tests::apply_mcp::"),
    }),
  defineModule({
      id: "rust.domain.optional-collaboration.workflow-operations.cancel",
      kind: "rust-domain",
      summary: "Explicit cancellation with plan and package digest binding",
      inputs: [
        "crates/licoup-native/src/domain/collaboration_plugin/workflow/operations/cancel.rs",
        "crates/licoup-native/src/domain/collaboration_plugin/workflow/operations/tests/cancel.rs",
      ],
      command: rustLayer("domain::collaboration_plugin::workflow::operations::tests::cancel::"),
    }),
  defineModule({
      id: "rust.domain.optional-collaboration.workflow-operations.validation",
      kind: "rust-domain",
      summary: "Direct-user, selection, UUID, expiry, and digest validation",
      inputs: [
        "crates/licoup-native/src/domain/collaboration_plugin/workflow/operations/validation.rs",
        "crates/licoup-native/src/domain/collaboration_plugin/workflow/operations/tests/validation.rs",
      ],
      command: rustLayer("domain::collaboration_plugin::workflow::operations::tests::validation::"),
    }),
  defineModule({
      id: "rust.domain.optional-collaboration.workflow-operations.package-revalidation",
      kind: "rust-domain",
      summary: "Installed package, selection payload, path, digest, and byte revalidation",
      inputs: [
        "crates/licoup-native/src/domain/collaboration_plugin/workflow/operations/package_revalidation.rs",
        "crates/licoup-native/src/domain/collaboration_plugin/workflow/operations/tests/package_revalidation.rs",
      ],
      command: rustLayer("domain::collaboration_plugin::workflow::operations::tests::package_revalidation::"),
    }),
  defineModule({
      id: "rust.domain.optional-collaboration.workflow-operations.destination-policy",
      kind: "rust-domain",
      summary: "Bounded absolute destinations, no-follow ancestry, and overlap rejection",
      inputs: [
        "crates/licoup-native/src/domain/collaboration_plugin/workflow/operations/destination_policy.rs",
        "crates/licoup-native/src/domain/collaboration_plugin/workflow/operations/tests/destination_policy.rs",
      ],
      command: rustLayer("domain::collaboration_plugin::workflow::operations::tests::destination_policy::"),
    }),
  defineModule({
      id: "rust.domain.optional-collaboration.workflow-operations.staging",
      kind: "rust-domain",
      summary: "Private MCP registrations, atomic staging, rollback, and claim cleanup",
      inputs: [
        "crates/licoup-native/src/domain/collaboration_plugin/workflow/operations/staging.rs",
        "crates/licoup-native/src/domain/collaboration_plugin/workflow/operations/tests/staging.rs",
      ],
      command: rustLayer(
        "domain::collaboration_plugin::workflow::operations::tests::staging::",
      ),
    }),
  defineModule({
      id: "rust.domain.optional-collaboration.workflow-operations.projection",
      kind: "rust-domain",
      summary: "Plan, apply, exact file-change, and privacy-safe workflow projection",
      inputs: [
        "crates/licoup-native/src/domain/collaboration_plugin/workflow/operations/projection.rs",
        "crates/licoup-native/src/domain/collaboration_plugin/workflow/operations/tests/projection.rs",
      ],
      command: rustLayer("domain::collaboration_plugin::workflow::operations::tests::projection::"),
    }),
  defineModule({
      id: "rust.domain.optional-collaboration.workflow-operations.integration",
      kind: "rust-domain",
      summary: "One-time local and MCP workflow plan, apply, rollback, race, and cancellation scenarios",
      inputs: [
        "crates/licoup-native/src/domain/collaboration_plugin/workflow/tests.rs",
      ],
      command: rustLayer("domain::collaboration_plugin::workflow::tests::"),
    }),
  defineModule({
      id: "rust.domain.client-state-migration",
      kind: "rust-domain",
      summary: "Forward-only client-state migration admission, reconciliation, and durable-domain preservation",
      inputs: [
        "crates/licoup-native/resources/client-state-migration-frontier.json",
        "crates/licoup-native/resources/state-machines/update-handoff.json",
        "crates/licoup-native/src/domain/client_state_migration.rs",
        "crates/licoup-native/src/domain/client_state_migration/handoff.rs",
        "crates/licoup-native/src/domain/client_state_migration/stores.rs",
        "crates/licoup-native/src/domain/client_state_migration/strategy_store.rs",
        "crates/licoup-native/src/domain/client_state_migration/tests.rs",
        "crates/licoup-native/src/domain/client_state_migration/tests/**",
        "tests/fixtures/client_state_migration/**",
        "crates/licoup-conversation/src/store/schema.rs",
        "crates/licoup-conversation/src/store/schema/**",
        "crates/licoup-conversation/src/store/native_sessions.rs",
        "crates/licoup-foundation/src/core/sqlite_contract.rs",
        "crates/licoup-workflow-store/**",
        "crates/licoup-agent-hermes/src/tui_gateway_driver.rs",
        "crates/licoup-native/build.rs",
      ],
      command: rustLayer("domain::client_state_migration::tests::"),
    }),
  defineModule({
      id: "rust.domain.client-update",
      kind: "rust-domain",
      summary: "Cross-cutting client update selection, metadata, receipts, and aggregate regression",
      runnableHosts: ["darwin", "linux"],
      targetEvidenceHosts: ["darwin", "linux"],
      inputs: [
        "crates/licoup-native/src/domain/client_update.rs",
        "crates/licoup-native/src/domain/client_update/constants.rs",
        "crates/licoup-native/src/domain/client_update/metadata.rs",
        "crates/licoup-native/src/domain/client_update/model.rs",
        "crates/licoup-native/src/domain/client_update/receipt.rs",
        "crates/licoup-native/src/domain/client_update/selection.rs",
        "crates/licoup-native/src/domain/client_update/tests.rs",
        "crates/licoup-native/src/domain/client_update/tests/support.rs",
        "crates/licoup-native/src/domain/client_update/verify.rs",
      ],
      command: rustLayer("domain::client_update::tests::", ["--skip", "native_runner::"]),
    }),
  defineModule({
      id: "rust.domain.client-update.signature-roles",
      kind: "rust-domain",
      summary: "Formal key document parsing and independent offline/online role signatures",
      inputs: [
        "crates/licoup-native/src/domain/client_update/keys.rs",
        "crates/licoup-native/src/domain/client_update/signature.rs",
        "crates/licoup-native/src/domain/client_update/tests/signature_roles.rs",
      ],
      command: rustLayer("domain::client_update::tests::signature_roles::"),
    }),
  defineModule({
      id: "rust.domain.client-update.release-selection",
      kind: "rust-domain",
      summary: "Strict channel, target, artifact metadata, and highest-SemVer selection",
      inputs: [
        "crates/licoup-native/src/domain/client_update/params.rs",
        "crates/licoup-native/src/domain/client_update/release.rs",
        "crates/licoup-native/src/domain/client_update/release/artifact.rs",
        "crates/licoup-native/src/domain/client_update/tests/release_selection.rs",
      ],
      command: rustLayer("domain::client_update::tests::release_selection::"),
    }),
  defineModule({
      id: "rust.domain.client-update.artifact-binding",
      kind: "rust-domain",
      summary: "Canonical signed artifact digest, name, and receipt binding",
      inputs: [
        "crates/licoup-native/src/domain/client_update/canonical.rs",
        "crates/licoup-native/src/domain/client_update/tests/artifact_binding.rs",
      ],
      command: rustLayer("domain::client_update::tests::artifact_binding::"),
    }),
  defineModule({
      id: "rust.domain.client-update.staging-paths",
      kind: "rust-domain",
      summary: "Caller-override rejection, canonical staging, resume, and symlink boundaries",
      inputs: [
        "crates/licoup-native/src/domain/client_update/download.rs",
        "crates/licoup-native/src/domain/client_update/staging.rs",
        "crates/licoup-native/src/domain/client_update/staging/copy.rs",
        "crates/licoup-native/src/domain/client_update/staging/path.rs",
        "crates/licoup-native/src/domain/client_update/tests/staging_paths.rs",
      ],
      command: rustLayer("domain::client_update::tests::staging_paths::"),
    }),
  defineModule({
      id: "rust.domain.client-update.revocation",
      kind: "rust-domain",
      summary: "Offline-root signed channel, key, version, and digest revocation policy",
      inputs: [
        "crates/licoup-native/src/domain/client_update/revocation.rs",
        "crates/licoup-native/src/domain/client_update/tests/revocation.rs",
      ],
      command: rustLayer("domain::client_update::tests::revocation::"),
    }),
  defineModule({
      id: "rust.domain.client-update.workflow",
      kind: "rust-domain",
      summary: "Redacted check, download, verify, apply-plan, status, and dispatch workflow",
      inputs: [
        "crates/licoup-native/src/domain/client_update/apply.rs",
        "crates/licoup-native/src/domain/client_update/check.rs",
        "crates/licoup-native/src/domain/client_update/dispatch.rs",
        "crates/licoup-native/src/domain/client_update/status.rs",
        "crates/licoup-native/src/domain/client_update/tests/workflow.rs",
      ],
      command: rustLayer("domain::client_update::tests::workflow::"),
    }),
  defineModule({
      id: "rust.domain.client-update.native-runner",
      kind: "rust-domain",
      summary: "Safe signed archive extraction and redacted macOS app lifecycle",
      runnableHosts: ["darwin"],
      targetEvidenceHosts: ["darwin"],
      inputs: [
        "crates/licoup-native/src/domain/client_update/native_runner/**",
        "crates/licoup-native/src/domain/client_update/tests/native_runner.rs",
      ],
      command: rustLayer("domain::client_update::tests::native_runner::"),
    }),
  defineModule({
      id: "rust.domain.secure-mesh-mls",
      kind: "rust-domain",
      summary: "Product-facing Secure Mesh MLS facade and aggregate domain regression",
      inputs: [
        "crates/licoup-native/src/domain/secure_mesh_mls.rs",
        "crates/licoup-native/src/domain/secure_mesh_mls/tests/mod.rs",
      ],
      command: rustLayer("domain::secure_mesh_mls::tests::"),
    }),
  defineModule({
      id: "rust.domain.secure-mesh-mls.actions",
      kind: "rust-domain",
      summary: "Stable native action dispatch and readiness status projection",
      inputs: [
        "crates/licoup-native/src/domain/secure_mesh_mls/actions.rs",
        "crates/licoup-native/src/domain/secure_mesh_mls/tests/actions.rs",
      ],
      command: rustLayer("domain::secure_mesh_mls::tests::actions::"),
    }),
  defineModule({
      id: "rust.domain.secure-mesh-mls.participant-key-package",
      kind: "rust-domain",
      summary: "Local participant projection and identity-bound KeyPackage creation",
      inputs: [
        "crates/licoup-native/src/domain/secure_mesh_mls/participant_key_package.rs",
        "crates/licoup-native/src/domain/secure_mesh_mls/tests/participant_key_package.rs",
      ],
      command: rustLayer(
        "domain::secure_mesh_mls::tests::participant_key_package::",
      ),
    }),
  defineModule({
      id: "rust.domain.secure-mesh-mls.group-flow",
      kind: "rust-domain",
      summary: "Group create, member mutation, join, and commit action orchestration",
      inputs: [
        "crates/licoup-native/src/domain/secure_mesh_mls/group_create.rs",
        "crates/licoup-native/src/domain/secure_mesh_mls/group_join.rs",
        "crates/licoup-native/src/domain/secure_mesh_mls/commit_process.rs",
        "crates/licoup-native/src/domain/secure_mesh_mls/member_mutation.rs",
        "crates/licoup-native/src/domain/secure_mesh_mls/tests/group_flow.rs",
      ],
      command: rustLayer("domain::secure_mesh_mls::tests::group_flow::"),
    }),
  defineModule({
      id: "rust.domain.secure-mesh-mls.payload",
      kind: "rust-domain",
      summary: "Trusted-roster payload sealing and opening request projection",
      inputs: [
        "crates/licoup-native/src/domain/secure_mesh_mls/payload.rs",
        "crates/licoup-native/src/domain/secure_mesh_mls/tests/payload.rs",
      ],
      command: rustLayer("domain::secure_mesh_mls::tests::payload::"),
    }),
  defineModule({
      id: "rust.domain.secure-mesh-mls.participant-runtime",
      kind: "rust-domain",
      summary: "Selected-custody participant runtime, persistence, reset, and missing-state policy",
      inputs: [
        "crates/licoup-native/src/domain/secure_mesh_mls/participant_runtime.rs",
        "crates/licoup-native/src/domain/secure_mesh_mls/tests/participant_runtime.rs",
      ],
      command: rustLayer(
        "domain::secure_mesh_mls::tests::participant_runtime::",
      ),
    }),
  defineModule({
      id: "rust.domain.secure-mesh-mls.directory-authorization",
      kind: "rust-domain",
      summary: "Pinned key-transparency directory authorization and roster readiness",
      inputs: [
        "crates/licoup-native/src/domain/secure_mesh_mls/directory_authorization.rs",
        "crates/licoup-native/src/domain/secure_mesh_mls/tests/directory_authorization.rs",
        "crates/licoup-native/src/domain/secure_mesh_mls/tests/support.rs",
      ],
      command: rustLayer(
        "domain::secure_mesh_mls::tests::directory_authorization::",
      ),
    }),
  defineModule({
      id: "rust.domain.secure-mesh-mls.journal-recovery",
      kind: "rust-domain",
      summary: "Crash-safe operation journal, failpoints, replay, and recovery",
      inputs: [
        "crates/licoup-native/src/domain/secure_mesh_mls/journal_recovery.rs",
        "crates/licoup-native/src/domain/secure_mesh_mls/tests/journal_recovery.rs",
      ],
      command: rustLayer(
        "domain::secure_mesh_mls::tests::journal_recovery::",
      ),
    }),
  defineModule({
      id: "rust.domain.secure-mesh-mls.group-state",
      kind: "rust-domain",
      summary: "Durable group authority reconciliation and rollback detection",
      inputs: [
        "crates/licoup-native/src/domain/secure_mesh_mls/group_state.rs",
        "crates/licoup-native/src/domain/secure_mesh_mls/tests/group_state.rs",
      ],
      command: rustLayer("domain::secure_mesh_mls::tests::group_state::"),
    }),
  defineModule({
      id: "rust.domain.secure-mesh-mls.input-codec",
      kind: "rust-domain",
      summary: "Bounded request schema, trust roster, identity, context, and canonical codecs",
      inputs: [
        "crates/licoup-native/src/domain/secure_mesh_mls/input_codec.rs",
        "crates/licoup-native/src/domain/secure_mesh_mls/tests/input_codec.rs",
      ],
      command: rustLayer("domain::secure_mesh_mls::tests::input_codec::"),
    }),
  defineModule({
      id: "rust.domain.conversation-snapshot-relocation",
      kind: "rust-domain",
      summary: "Structured snapshot reference rebasing across a data-root relocation",
      inputs: [
        "crates/licoup-native/src/domain/conversation/snapshots/relocation.rs",
        "crates/licoup-native/src/domain/conversation/snapshots/tests/relocation.rs",
      ],
      command: rustLayer("domain::conversation::snapshots::"),
    })
]);
