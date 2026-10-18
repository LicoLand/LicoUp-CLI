import { definePlatformEntry, readOnlyCommandOutput } from "../factory.mjs";
export default definePlatformEntry({
  id: "android",
  inputs: ["apps/desktop/integration_test/mobile_simulator_closure_test.dart"],
  unverifiedInputs: [],
  hosts: [],
  tools: ["flutter", "adb", "emulator"],
  resources: ["cargo-target", "flutter-cache", "gradle-cache"],
  async capabilityProbe() {
    const devices = await readOnlyCommandOutput("adb", ["devices"]);
    return devices.ok && /^emulator-[0-9]+\s+device\s*$/mu.test(devices.output)
      ? { eligible: true, reason: null }
      : { eligible: false, reason: "android_emulator_unavailable" };
  },
  liveCommand: Object.freeze({ program: "node", args: Object.freeze([
    "tools/scripts/client-mobile-simulator-closure.mjs", "--platform", "android",
  ]), cwd: ".", timeoutMs: 30 * 60_000 }),
});
