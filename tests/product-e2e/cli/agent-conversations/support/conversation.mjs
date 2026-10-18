#!/usr/bin/env node

/**
 * Public real-conversation executor for every agent directory.
 *
 * One real conversation: send exactly one message from the LicoUp interface
 * (licoup-cli sidecar `agent conversation send`) to the named agent and
 * assert that the agent returns a non-empty reply.
 */

import { mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { agentConfigs, acceptanceMode } from "./parity/constants.mjs";
import { AcceptanceError, requireFact } from "./parity/errors.mjs";
import { createPrivateWrapper } from "./parity/process.mjs";
import { resolveExecutable, resolveSidecar } from "./parity/sidecar.mjs";
import { runSidecar } from "./parity/native/acp-turn.mjs";
import { cleanupSession, preflightCleanup } from "./parity/session-cleanup.mjs";
import { parityEffortForAgent, parityModelForAgent } from "./parity/agent-ids.mjs";

function parseArgs(argv) {
  const options = {
    agent: "",
    timeoutMs: Number(process.env.LICO_ACP_PARITY_TIMEOUT_MS || 600_000),
    maxOutputBytes: Number(process.env.LICO_ACP_PARITY_MAX_OUTPUT_BYTES || 4 * 1024 * 1024),
    binary: "",
    sidecar: "",
  };
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    if (["--binary", "--sidecar", "--timeout-ms", "--max-output-bytes", "--agent"].includes(argument)) {
      const value = argv[++index];
      requireFact(typeof value === "string" && value.length > 0, "cli_argument_missing");
      if (argument === "--agent") options.agent = value.trim().toLowerCase().replaceAll("_", "-");
      if (argument === "--binary") options.binary = value;
      if (argument === "--sidecar") options.sidecar = value;
      if (argument === "--timeout-ms") options.timeoutMs = Number(value);
      if (argument === "--max-output-bytes") options.maxOutputBytes = Number(value);
    } else {
      throw new AcceptanceError("cli_argument_unsupported");
    }
  }
  requireFact(Number.isFinite(options.timeoutMs) && options.timeoutMs >= 1_000, "timeout_invalid");
  requireFact(options.agent.length > 0, "agent_required");
  return options;
}

export async function runAgentConversation(argv = process.argv.slice(2)) {
  let sessionId = "";
  let output = "";
  let cleanupPassed = false;
  let temporaryDirectory = "";
  try {
    const options = parseArgs(argv);
    const config = agentConfigs[options.agent];
    requireFact(Boolean(config), "agent_not_acp_packaged");
    const binary = resolveExecutable(options.binary, config);
    requireFact(Boolean(binary), "install_agent_binary");
    const sidecar = resolveSidecar(options.sidecar, { releaseUi: false });
    requireFact(Boolean(sidecar), "sidecar_missing");
    temporaryDirectory = mkdtempSync(join(tmpdir(), `lico-conversation-${options.agent}-`));
    const cwd = join(temporaryDirectory, "workspace");
    mkdirSync(cwd, { recursive: true });
    const wrapper = createPrivateWrapper(temporaryDirectory, binary);
    const portableDataRoot = join(temporaryDirectory, "licoup-portable");
    mkdirSync(portableDataRoot, { recursive: true, mode: 0o700 });
    wrapper.environment = {
      ...wrapper.environment,
      ...process.env,
      LICO_AGENT_CONVERSATION_ACCEPTANCE: acceptanceMode,
      LICOUP_HOME: portableDataRoot,
    };
    const context = {
      config,
      binary,
      sidecar,
      wrapper,
      cwd,
      environment: wrapper.environment,
      temporaryDirectory,
      disposableDataRoot: "",
      disposableSeedSource: "",
      disposableProfileSeeded: false,
      timeoutMs: options.timeoutMs,
      maxOutputBytes: options.maxOutputBytes,
      observedSessions: new Set(),
      cleanedSessions: new Set(),
      acceptanceMode,
      copilotSdkLaunchArgs: null,
    };

    const preflight = await preflightCleanup(context);
    requireFact(preflight.ready === true, preflight.code || "cleanup_preflight_failed");

    const request = {
      agent: options.agent,
      text: "Hi",
      workingDirectory: cwd,
      binaryPath: wrapper.wrapperPath,
      timeoutMs: options.timeoutMs,
      maxStdoutBytes: options.maxOutputBytes,
      maxStderrBytes: options.maxOutputBytes,
      streamEvents: true,
    };
    const model = parityModelForAgent(options.agent);
    if (model) request.model = model;
    const effort = parityEffortForAgent(options.agent, model);
    if (effort) request.reasoningEffort = effort;

    const sent = await runSidecar(context, request);
    sessionId = sent.result?.sessionId || sent.result?.nativeSessionId || "";
    output = String(sent.result?.output || "").trim();
    requireFact(typeof sessionId === "string" && sessionId.length > 0, "native_session_id_missing");
    requireFact(output.length > 0, "native_final_message_missing");
    requireFact(
      String(sent.result?.turnStatus || "") === "completed"
        || String(sent.result?.turnStatus || "") === "end_turn",
      "native_turn_not_completed",
    );

    cleanupPassed = await cleanupSession(context, sessionId, temporaryDirectory);
    requireFact(cleanupPassed === true, "cleanup_failed");

    return {
      status: "passed",
      agent: options.agent,
      conversationPassed: true,
      sessionIdPresent: true,
      replyBytes: Buffer.byteLength(output),
      streamingSeen: sent.streamingSeen === true,
      structuredSeen: sent.structuredSeen === true,
      boundedOutput: sent.boundedOutput === true,
      cleanupPassed: true,
    };
  } catch (error) {
    const reasonCode = /^[a-z0-9_-]+$/u.test(error?.message || "")
      ? error.message
      : "unexpected_failure";
    return {
      status: "failed",
      agent: null,
      conversationPassed: false,
      reasonCode,
      sessionIdPresent: Boolean(sessionId),
      cleanupPassed,
    };
  } finally {
    if (temporaryDirectory) {
      rmSync(temporaryDirectory, { recursive: true, force: true });
    }
  }
}

async function main() {
  let output;
  try {
    output = await runAgentConversation(process.argv.slice(2));
  } catch (error) {
    output = {
      status: "failed",
      agent: null,
      conversationPassed: false,
      reasonCode: /^[a-z0-9_-]+$/u.test(error?.message || "")
        ? error.message
        : "unexpected_failure",
    };
  }
  process.stdout.write(`${JSON.stringify(output)}\n`);
  if (output.status !== "passed") process.exitCode = 1;
}

const invoked = process.argv[1]
  && resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (invoked) {
  await main();
}
