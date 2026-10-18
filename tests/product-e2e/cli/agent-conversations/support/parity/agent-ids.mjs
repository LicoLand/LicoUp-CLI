import {
  verificationEffortForAgent,
  verificationModelForAgent,
} from "../../../../../../tools/scripts/lib/agent-conversation-verification-models.mjs";

export function normalizeAgentId(value) {
  const normalized = String(value).trim().toLowerCase().replaceAll("_", "-");
  const aliases = {
    kilo: "kilo-code",
    kilocode: "kilo-code",
    "github-copilot": "copilot",
    "hermes-agent": "hermes",
    "cursor-agent": "cursor",
    kimicode: "kimi-code",
  };
  return aliases[normalized] || normalized;
}

/** Live verification uses the maintained low-cost model, never a shell override. */
export function parityModelForAgent(agentId) {
  const model = verificationModelForAgent(agentId);
  if (!model) throw new Error("verification_model_unconfigured");
  return model;
}

/**
 * Verification / parity reasoning effort paired with the configured model.
 * Inherited environment variables cannot silently raise the verification cost.
 */
export function parityEffortForAgent(agentId, model) {
  return verificationEffortForAgent(agentId, model);
}
