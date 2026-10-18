import { command, defineModule } from "../helpers.mjs";

function crateTests(id, summary, crate, inputs) {
  const manifest = `crates/${crate}/Cargo.toml`;
  return defineModule({
    id,
    kind: "rust-crate",
    summary,
    inputs: [manifest, ...inputs],
    command: command(
      "cargo",
      ["test", "--manifest-path", manifest],
      10 * 60_000,
    ),
  });
}

export const RUST_COMPONENT_MODULES = Object.freeze([
  defineModule({
    id: "rust.component.analytics",
    kind: "rust-crate",
    summary: "Optional metrics correction/correlation and borrowed core-fact preservation on removal",
    inputs: [
      "components/analytics/**",
      "sdk/usage-source/**",
      "tests/integration/usage_sources/**",
      "crates/licoup-extension-contracts/src/usage.rs",
    ],
    command: command(
      "cargo",
      [
        "test",
        "--locked",
        "--offline",
        "--manifest-path",
        "components/analytics/Cargo.toml",
      ],
      20 * 60_000,
    ),
  }),
  crateTests(
    "rust.crate.gateway",
    "Optional gateway package declaration and kernel separation",
    "licoup-private-gateway",
    ["crates/licoup-private-gateway/src/**", "crates/licoup-private-gateway/tests/**"],
  ),
  crateTests(
    "rust.crate.mobile-core",
    "Portable mobile entry and group bridge contracts",
    "licoup-mobile-core",
    ["crates/licoup-mobile-core/src/**", "crates/licoup-mobile-core/tests/**"],
  ),
  crateTests(
    "rust.crate.model-catalog",
    "Model candidate policy and composition port",
    "licoup-model-catalog",
    ["crates/licoup-model-catalog/src/**", "crates/licoup-model-catalog/tests/**"],
  ),
  crateTests(
    "rust.crate.project",
    "Project identity, dependency, plan and scheduler contracts",
    "licoup-project",
    ["crates/licoup-project/src/**", "crates/licoup-project/tests/**"],
  ),
  crateTests(
    "rust.crate.agent-runtime",
    "Agent runtime work-context authority and protocol-neutral session state",
    "licoup-agent-runtime",
    ["crates/licoup-agent-runtime/tests/work_context.rs"],
  ),
  crateTests(
    "rust.crate.application-contracts",
    "Protocol-neutral application commands, dependency direction, and extension contracts",
    "licoup-application",
    [
      "crates/licoup-application/tests/command_contract.rs",
      "crates/licoup-application/tests/dependency_boundary.rs",
      "crates/licoup-application/tests/extension_contract.rs",
    ],
  ),
  crateTests(
    "rust.crate.conversation-contracts",
    "Conversation continuity contract and configured state-machine invariants",
    "licoup-conversation",
    [
      "crates/licoup-conversation/tests/continuity_contract.rs",
      "crates/licoup-conversation/tests/state_machine_proptest.rs",
      "tests/fixtures/continuous-assistant/contracts/**",
    ],
  ),
  crateTests(
    "rust.crate.extension-contracts",
    "Published extension schemas, sample vectors, and portable extension behavior",
    "licoup-extension-contracts",
    [
      "crates/licoup-extension-contracts/samples/**",
      "crates/licoup-extension-contracts/tests/minimal_agent_sample.rs",
      "crates/licoup-extension-contracts/tests/schema_agreement.rs",
      "crates/licoup-extension-contracts/tests/state_converter_sample.rs",
      "schemas/extensions/**",
    ],
  ),
  crateTests(
    "rust.crate.state-machine-codegen",
    "Deterministic state-machine generation and invalid transition rejection",
    "licoup-state-machine-codegen",
    [
      "crates/licoup-state-machine-codegen/tests/compiler.rs",
    ],
  ),
  crateTests(
    "rust.crate.workflow-runtime",
    "Workflow admission, driver, routing, and successor runtime over the pure workflow crate",
    "licoup-workflow-runtime",
    [
      "crates/licoup-workflow-runtime/src/**",
      "crates/licoup-workflow/src/**",
      "crates/licoup-workflow-runtime/tests/**",
    ],
  ),
  crateTests(
    "rust.crate.workflow-store",
    "Durable workflow transactions, deliveries, and recovery over SQLite",
    "licoup-workflow-store",
    [
      "crates/licoup-workflow-store/src/**",
      "crates/licoup-workflow/src/**",
    ],
  ),
]);
