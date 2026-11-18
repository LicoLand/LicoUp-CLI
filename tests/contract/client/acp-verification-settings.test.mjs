import assert from "node:assert/strict";
import test from "node:test";
import { applyAcpVerificationSettings } from "../../product-e2e/cli/agent-conversations/support/parity/native/acp-settings.mjs";
import { sessionSettings } from "../../product-e2e/cli/agent-conversations/support/parity/native/pi.mjs";

test("effort uses the options exposed after switching models and returns actual settings", async () => {
  const calls = [];
  const client = { async request(method, params) {
    calls.push({ method, ...params });
    return { configOptions: [
      { id: "model", currentValue: "synthetic-model" },
      { id: "variant", currentValue: params.configId === "variant" ? params.value : "low" },
    ] };
  } };
  const result = await applyAcpVerificationSettings(client, "synthetic-session", {
    configOptions: [{ id: "model", currentValue: "previous-model" }],
  }, { model: "synthetic-model", effort: "high", cwd: "fixture" });
  assert.deepEqual(calls.map(({ configId, value }) => [configId, value]), [
    ["model", "synthetic-model"], ["variant", "high"],
  ]);
  assert.equal(sessionSettings(result, "fixture").reasoningEffort, "high");
  assert.equal(sessionSettings(result, "fixture").model, "synthetic-model");
});

test("missing effort support stops before sending an invented option", async () => {
  let calls = 0;
  await assert.rejects(applyAcpVerificationSettings({ async request() { calls += 1; } },
    "synthetic-session", { configOptions: [] }, { effort: "high", cwd: "fixture" }),
  /native_effort_option_missing/);
  assert.equal(calls, 0);
});

test("Kimi thinking selection is verified from the returned ACP option", async () => {
  const client = { async request(method, params) {
    assert.equal(params.configId, "thinking");
    return { configOptions: [{ id: "thinking", category: "thought_level", currentValue: params.value }] };
  } };
  const result = await applyAcpVerificationSettings(client, "synthetic-session", {
    configOptions: [{ id: "thinking", category: "thought_level", currentValue: "max" }],
  }, { effort: "high", cwd: "fixture" });
  assert.equal(sessionSettings(result, "fixture").reasoningEffort, "high");
});

test("a server that retains another effort cannot produce passing selection evidence", async () => {
  await assert.rejects(applyAcpVerificationSettings({ async request() {
    return { configOptions: [{ id: "reasoning_effort", currentValue: "low" }] };
  } }, "synthetic-session", {
    configOptions: [{ id: "reasoning_effort", currentValue: "low" }],
  }, { effort: "high", cwd: "fixture" }), /native_effort_selection_mismatch/);
});

test("missing configuration readback cannot be replaced with requested values", async () => {
  await assert.rejects(applyAcpVerificationSettings({ async request() { return {}; } },
    "synthetic-session", {}, { model: "synthetic-model", cwd: "fixture" }),
  /native_config_readback_missing/);
});

test("empty setter options cannot reuse the initial model as fresh evidence", async () => {
  await assert.rejects(applyAcpVerificationSettings({ async request() {
    return { configOptions: [] };
  } }, "synthetic-session", {
    models: { currentModelId: "synthetic-model" },
  }, { model: "synthetic-model", cwd: "fixture" }), /native_model_selection_mismatch/);
});
