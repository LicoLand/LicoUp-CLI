import { requireFact } from "../errors.mjs";
import { sessionSettings } from "./pi.mjs";

/** Apply explicit verification settings using the options returned by ACP. */
export async function applyAcpVerificationSettings(client, sessionId, initial, { model, effort, cwd }) {
  let current = initial;
  const setOption = async (configId, value) => {
    const updated = await client.request("session/set_config_option", {
      sessionId,
      configId,
      value,
    });
    requireFact(Array.isArray(updated?.configOptions), "native_config_readback_missing");
    // A setter response is the only valid evidence that the runtime accepted
    // the requested setting; metadata from session/new is only a starting point.
    current = { ...current, ...updated, models: updated.models };
  };
  if (model) {
    await setOption("model", model);
    requireFact(sessionSettings(current, cwd).model === model, "native_model_selection_mismatch");
  }
  if (effort) {
    const option = current?.configOptions?.find((candidate) =>
      ["reasoning_effort", "variant", "thinking"].includes(candidate?.id));
    requireFact(Boolean(option), "native_effort_option_missing");
    await setOption(option.id, effort);
    requireFact(
      sessionSettings(current, cwd).reasoningEffort === effort,
      "native_effort_selection_mismatch",
    );
  }
  return current;
}
