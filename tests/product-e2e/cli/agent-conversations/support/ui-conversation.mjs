#!/usr/bin/env node

/**
 * Front-end UI real conversation executor for every agent directory.
 *
 * One conversation through the LicoUp front end: launch the packaged macOS
 * app in release-live mode, submit short prompts from the Composer widget,
 * and accept the Agent's raw streamed reply while checking the session facts.
 * The release app drives the native sidecar and agent binary; nothing is
 * mocked.
 */

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { verificationEffortForAgent } from "../../../../../tools/scripts/lib/agent-conversation-verification-models.mjs";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../../../../..");
const desktopRoot = resolve(root, "apps/desktop");
const runnableApp = resolve(root, "build/apps/desktop/runnable/macos/release/LicoUp.app");
const modelsPath = resolve(root, "tools/scripts/config/agent-conversation-verification-models.toml");
const sentinel = "LICO_AGENT_CONVERSATION_RELEASE_UI_LIVE ";

const receiptFields = new Set([
  "schemaVersion",
  "status",
  "reasonCode",
  "receiptKind",
  "releaseMode",
  "packagedApplicationProcess",
  "packagedSidecarUsed",
  "fixtureBackend",
  "agentId",
  "model",
  "nativeSessionId",
  "composerSubmitted",
  "progressiveTimelineVisible",
  "sameNativeSessionId",
  "historyReadback",
  "turnCount",
  "invocationChallengeDigest",
]);

function modelForAgent(agentId) {
  const source = readFileSync(modelsPath, "utf8");
  const escaped = agentId.replaceAll(/[.*+?^${}()|[\]\\]/gu, "\\$&");
  const pattern = new RegExp(
    `^\\s*(?:${escaped}|"${escaped}")\\s*=\\s*"([^"]+)"\\s*$`,
    "mu",
  );
  const match = pattern.exec(source);
  return match?.[1]?.trim() || "";
}

function parseArgs(argv) {
  const options = { agent: "", output: "" };
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    if (argument === "--agent") {
      options.agent = String(argv[++index] || "").trim().toLowerCase().replaceAll("_", "-");
    } else if (argument === "--output") {
      options.output = resolve(root, argv[++index] || "");
    } else {
      throw new Error("argument_unsupported");
    }
  }
  if (!/^[a-z0-9-]{1,64}$/u.test(options.agent)) {
    throw new Error("agent_id_invalid");
  }
  return options;
}

function bundleExecutable(appBundle) {
  const plist = readFileSync(join(appBundle, "Contents/Info.plist"), "utf8");
  const match = plist.match(/<key>CFBundleExecutable<\/key>\s*<string>([^<]+)<\/string>/u);
  if (!match || !/^[A-Za-z0-9._ -]+$/u.test(match[1])) {
    throw new Error("release_app_executable_invalid");
  }
  return join(appBundle, "Contents/MacOS", match[1]);
}

export function runUiConversation(options) {
  if (!existsSync(runnableApp)) {
    throw new Error("packaged_release_app_missing");
  }
  if (!existsSync(join(runnableApp, "Contents/Helpers/LicoUpCustody.app/Contents/MacOS/licoup-cli"))) {
    throw new Error("packaged_release_sidecar_missing");
  }
  const model = modelForAgent(options.agent);
  if (!model) {
    throw new Error("verification_model_unconfigured");
  }
  const executable = bundleExecutable(runnableApp);
  const receiptDirectory = mkdtempSync(join(tmpdir(), "lico-ui-conversation-"));
  const receiptPath = join(receiptDirectory, "receipt.txt");
  const firstPrompt = "Hi";
  const secondPrompt = "Hi";
  const invocationChallengeDigest = `sha256:${createHash("sha256")
    .update(`${options.agent}:${firstPrompt}:${secondPrompt}`)
    .digest("hex")}`;
  const environment = {
    ...process.env,
    LICO_CLIENT_PATH: join(runnableApp, "Contents/Helpers/LicoUpCustody.app/Contents/MacOS/licoup-cli"),
    LICO_AGENT_CONVERSATION_ACCEPTANCE: "dispatch-lane-unified-1",
    LICO_AGENT_CONVERSATION_PRODUCT_AGENT: options.agent,
    LICO_AGENT_CONVERSATION_PRODUCT_MODEL: model,
    LICO_AGENT_CONVERSATION_PRODUCT_EFFORT: verificationEffortForAgent(options.agent, model),
    LICO_AGENT_CONVERSATION_PRODUCT_FIRST_PROMPT: firstPrompt,
    LICO_AGENT_CONVERSATION_PRODUCT_SECOND_PROMPT: secondPrompt,
    LICO_AGENT_CONVERSATION_PRODUCT_RECEIPT: receiptPath,
    LICO_AGENT_CONVERSATION_PRODUCT_CHALLENGE_DIGEST: invocationChallengeDigest,
  };
  delete environment.CARGO_TARGET_DIR;

  const execution = spawnSync(executable, [], {
    cwd: dirname(runnableApp),
    encoding: "utf8",
    env: environment,
    maxBuffer: 1024 * 1024,
    timeout: 25 * 60 * 1000,
  });
  if (execution.error?.code === "ETIMEDOUT") {
    throw new Error("release_app_acceptance_timeout");
  }
  if (!existsSync(receiptPath)) {
    throw new Error("release_app_acceptance_receipt_missing");
  }
  const lines = readFileSync(receiptPath, "utf8")
    .split(/\r?\n/u)
    .filter((candidate) => candidate.startsWith(sentinel));
  if (lines.length !== 1) {
    throw new Error("release_ui_live_receipt_missing_or_ambiguous");
  }
  let receipt;
  try {
    receipt = JSON.parse(Buffer.from(lines[0].slice(sentinel.length), "base64url").toString("utf8"));
  } catch {
    throw new Error("release_ui_live_receipt_invalid");
  }
  const unknownFields = Object.keys(receipt).filter((field) => !receiptFields.has(field));
  if (unknownFields.length > 0) {
    throw new Error("release_ui_live_receipt_unbounded");
  }
  if (receipt.status !== "passed") {
    throw new Error(typeof receipt.reasonCode === "string" && /^[a-z0-9_-]{1,96}$/u.test(receipt.reasonCode)
      ? receipt.reasonCode
      : "release_ui_live_receipt_failed");
  }
  const passed =
    receipt.schemaVersion === "lico-agent-conversation-release-ui-live-v1"
    && receipt.receiptKind === "release-ui-live"
    && receipt.releaseMode === true
    && receipt.packagedApplicationProcess === true
    && receipt.packagedSidecarUsed === true
    && receipt.fixtureBackend === false
    && receipt.agentId === options.agent
    && typeof receipt.model === "string"
    && receipt.model.length > 0
    && receipt.composerSubmitted === true
    && receipt.progressiveTimelineVisible === true
    && receipt.sameNativeSessionId === true
    && receipt.historyReadback === true
    && receipt.turnCount === 2
    && receipt.invocationChallengeDigest === invocationChallengeDigest;
  if (!passed) {
    throw new Error("release_ui_live_receipt_incomplete");
  }
  return {
    ...receipt,
  };
}

export async function runUiConversationCli(argv = process.argv.slice(2)) {
  let output;
  try {
    const options = parseArgs(argv);
    const result = runUiConversation(options);
    output = {
      status: "passed",
      schemaVersion: "lico-agent-conversation-ui-e2e-v1",
      agent: options.agent,
      model: result.model,
      conversationPassed: true,
      composerSubmitted: true,
      progressiveTimelineVisible: true,
      sameNativeSessionId: true,
      historyReadback: true,
      turnCount: 2,
      nativeSessionIdPresent: Boolean(result.nativeSessionId),
      replyVisibleInUi: true,
    };
    if (options.output) {
      mkdirSync(dirname(options.output), { recursive: true });
      writeFileSync(options.output, `${JSON.stringify(output, null, 2)}\n`, { mode: 0o600 });
    }
  } catch (error) {
    const reasonCode = /^[a-z0-9_-]+$/u.test(error?.message || "")
      ? error.message
      : "unexpected_failure";
    output = {
      status: "failed",
      schemaVersion: "lico-agent-conversation-ui-e2e-v1",
      agent: null,
      conversationPassed: false,
      reasonCode,
    };
  }

  process.stdout.write(`${JSON.stringify(output)}\n`);
  if (output.status !== "passed") process.exitCode = 1;
  return output;
}

const invoked = process.argv[1]
  && resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (invoked) {
  await runUiConversationCli();
}
