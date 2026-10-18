import process from "node:process";
import { spawnSync } from "node:child_process";
import { commandOptions } from "./process.mjs";

export function runSwiftProof(helper, options = {}) {
  const env = { ...process.env };
  if (options.interactive === true) env.LICO_MACOS_USER_PRESENCE_INTERACTIVE = "1";
  const result = spawnSync(helper.path, [], {
    ...commandOptions(options.interactive === true ? 75_000 : 30_000),
    env,
  });
  helper.ran = result.status === 0;
  if (result.status !== 0) {
    throw new Error(`signed macOS adaptive custody helper failed with redacted status ${String(result.status ?? "unknown")}`);
  }
  return result;
}
