import { definePlatformEntry } from "../factory.mjs";
export default definePlatformEntry({
  id: "linux",
  hosts: ["linux"],
  tools: ["flutter", "xvfb-run"],
  artifacts: ["build/apps/desktop/runnable/linux/release"],
  liveCommand: Object.freeze({ program: "node", args: Object.freeze([
    "apps/desktop/scripts/gui-smoke-linux-bundle.mjs",
  ]), cwd: ".", timeoutMs: 30 * 60_000 }),
});
