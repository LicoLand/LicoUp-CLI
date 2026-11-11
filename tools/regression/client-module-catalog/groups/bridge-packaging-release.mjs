import {
  command,
  node,
  rustBinaryTests,
  rustLayer,
  defineModule,
} from "../helpers.mjs";

function releaseScriptModule({ id, summary, script, args = [], inputs = [] }) {
  return defineModule({
    id,
    kind: "release",
    summary,
    inputs: [script, ...inputs],
    command: node(script, args, 5 * 60_000),
  });
}

export const BRIDGE_PACKAGING_RELEASE_MODULES = Object.freeze([
  defineModule({
      id: "bridge.native-mcp-command",
      kind: "platform-bridge",
      summary: "Private-stdin MCP preview, direct authorization, and one-shot execute commands",
      inputs: [
        "crates/licoup/src/ffi/commands/mcp.rs",
        "crates/licoup/src/ffi/commands/mod.rs",
      ],
      command: rustLayer("ffi::commands::mcp::tests::"),
    }),
  defineModule({
      id: "bridge.native-mcp-rpc-guard",
      kind: "platform-bridge",
      summary: "CLI help and persistent-RPC guard for private MCP stdin",
      inputs: [
        "crates/licoup/src/bin/licoup/presentation.rs",
        "crates/licoup/src/bin/licoup/stdio_rpc.rs",
        "crates/licoup/src/bin/licoup/stdio_rpc/**",
      ],
      command: rustBinaryTests("licoup", "mcp"),
    }),
  releaseScriptModule({
    id: "release.native-smoke-policy",
    summary: "Native smoke command policy and runtime-data boundary",
    script: "tests/smoke/native-client-smoke-policy-self-test.mjs",
  }),
  defineModule({
      id: "release.model-pricing",
      kind: "release",
      summary: "Single current model pricing catalog and freshness guard",
      inputs: [
        "crates/licoup/src/domain/provider_model_pricing.rs",
        "crates/licoup-model-catalog/src/pricing/pricing_catalog.json",
        "tools/scripts/model-pricing-facts.mjs",
        "tests/contract/client/model-pricing.test.mjs",
      ],
      command: node("tests/contract/client/model-pricing.test.mjs"),
    }),
]);
