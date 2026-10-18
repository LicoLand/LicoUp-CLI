import { readFileSync } from "node:fs";
import path from "node:path";
import { regressionRoot } from "./factory.mjs";
import macos from "./platforms/macos.mjs";
import android from "./platforms/android.mjs";
import windows from "./platforms/windows.mjs";
import linux from "./platforms/linux.mjs";
import ios from "./platforms/ios.mjs";
import openclaw from "./agents/openclaw.mjs";
import claudeCode from "./agents/claude-code.mjs";
import codex from "./agents/codex.mjs";
import antigravity from "./agents/antigravity.mjs";
import opencode from "./agents/opencode.mjs";
import copilot from "./agents/copilot.mjs";
import kiloCode from "./agents/kilo-code.mjs";
import cursor from "./agents/cursor.mjs";
import hermes from "./agents/hermes.mjs";
import kimiCode from "./agents/kimi-code.mjs";
import pi from "./agents/pi.mjs";
import deepseekHarness from "./agents/deepseek-harness.mjs";
import licoAgent from "./agents/lico-agent.mjs";

export const PLATFORM_REGRESSION_ENTRIES = Object.freeze([
  macos, android, windows, linux, ios,
]);

export const AGENT_REGRESSION_ENTRIES = Object.freeze([
  openclaw,
  claudeCode,
  codex,
  antigravity,
  opencode,
  copilot,
  kiloCode,
  cursor,
  hermes,
  kimiCode,
  pi,
  deepseekHarness,
  licoAgent,
]);

export const CLIENT_COMPATIBILITY_ENTRIES = Object.freeze([
  ...PLATFORM_REGRESSION_ENTRIES,
  ...AGENT_REGRESSION_ENTRIES,
]);

export function validateClientRegressionEntries() {
  const expectedPlatforms = ["macos", "android", "windows", "linux", "ios"];
  const actualPlatforms = PLATFORM_REGRESSION_ENTRIES.map((entry) => entry.id);
  if (JSON.stringify(actualPlatforms) !== JSON.stringify(expectedPlatforms)) {
    throw new Error("platform regression entry inventory drift");
  }
  const inventory = JSON.parse(readFileSync(path.resolve(
    regressionRoot,
    "crates/licoup-native/resources/agent-conversation-drivers.json",
  ), "utf8"));
  const expectedAgents = inventory.drivers.map((driver) => driver.agentId);
  const actualAgents = AGENT_REGRESSION_ENTRIES.map((entry) => entry.id);
  if (JSON.stringify(actualAgents) !== JSON.stringify(expectedAgents)) {
    throw new Error("agent regression entry inventory drift");
  }
  const ids = CLIENT_COMPATIBILITY_ENTRIES.map((entry) => `${entry.kind}:${entry.id}`);
  if (new Set(ids).size !== ids.length) throw new Error("duplicate compatibility regression entry");
  for (const entry of CLIENT_COMPATIBILITY_ENTRIES) {
    if (entry.stage !== "compatibility" || typeof entry.probe !== "function") {
      throw new Error("compatibility regression entry is invalid");
    }
    if (!Array.isArray(entry.inputs) || !Object.isFrozen(entry.inputs) ||
        !Array.isArray(entry.unverifiedInputs) || !Object.isFrozen(entry.unverifiedInputs)) {
      throw new Error(`compatibility input classification is invalid: ${entry.kind}:${entry.id}`);
    }
    const classified = [...entry.inputs, ...entry.unverifiedInputs];
    if (new Set(classified).size !== classified.length) {
      throw new Error(`duplicate compatibility input classification: ${entry.kind}:${entry.id}`);
    }
  }
  return true;
}
