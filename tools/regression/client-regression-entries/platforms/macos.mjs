import { definePlatformEntry } from "../factory.mjs";
export default definePlatformEntry({
  id: "macos",
  hosts: ["darwin"],
  tools: ["xcrun", "flutter"],
  resources: ["agent-runtime:codex"],
  inputs: [
    "apps/desktop/integration_test/agent_conversation_product_e2e_test.dart",
  ],
  unverifiedInputs: [
    "apps/desktop/integration_test/conversation_profile_create_flow_test.dart",
    "apps/desktop/integration_test/dashboard_visual_drive_test.dart",
    "apps/desktop/integration_test/glass_filter_rendering_test.dart",
    "apps/desktop/integration_test/glass_visual_reference_test.dart",
    "apps/desktop/integration_test/group_conversation_button_flow_test.dart",
    "apps/desktop/integration_test/ui_state_machine_test.dart",
    "tests/product-e2e/cli/client-update/nightly-self-migration-installed.test.mjs",
  ],
  artifacts: ["build/apps/desktop/runnable/macos/release/LicoUp.app"],
  liveCommand: Object.freeze({ program: "node", args: Object.freeze([
    "tools/scripts/client-device-demo.mjs", "--platform", "macos",
  ]), cwd: ".", timeoutMs: 30 * 60_000 }),
});
