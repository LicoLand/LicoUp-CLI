#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  existsSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  readlinkSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { basename, dirname, join, relative, resolve } from "node:path";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";
import {
  releaseClosureChallengeDigest,
  requiredReleaseClosureChallenge,
} from "./lib/release-closure-challenge.mjs";
import {
  nativeContinuityDigest,
  productContinuityBindingDigest,
} from "./lib/agent-conversation-release-binding.mjs";
import {
  verificationEffortForAgent,
  verificationModelForAgent,
  verificationModelsMap,
} from "./lib/agent-conversation-verification-models.mjs";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const desktopRoot = resolve(root, "apps/desktop");
const widgetTestRef = "integration_test/agent_conversation_product_e2e_test.dart";
const fixtureRef = "integration_test/support/agent_conversation_product_fixture.dart";
const liveSourceRef = "lib/src/composition/product_acceptance/agent_conversation_release_live.dart";
const sentinel = "LICO_AGENT_CONVERSATION_RELEASE_UI_LIVE ";
const defaultAgent = "codex";
const selfTestChallengeDigest = `sha256:${"a".repeat(64)}`;
const selfTestModel = verificationModelForAgent(defaultAgent);
const validationModels = verificationModelsMap();
const runnableApp = resolve(root, "build/apps/desktop/runnable/macos/release/LicoUp.app");
const defaultOutput = resolve(root, "build/reports/agent-conversation-product-e2e.json");
const liveReceiptFields = new Set([
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

class ProductAcceptanceError extends Error {
  constructor(reasonCode, details = null) {
    super(reasonCode);
    this.details = details;
  }
}

function fail(reasonCode, details = null) {
  throw new ProductAcceptanceError(reasonCode, details);
}

function parseArgs(argv) {
  const options = {
    selfTest: false,
    output: defaultOutput,
    agents: [],
    platform: process.platform === "darwin" ? "macos" : process.platform === "win32" ? "windows" : "linux",
  };
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    if (argument === "--self-test") options.selfTest = true;
    else if (["--output", "--platform", "--agent"].includes(argument)) {
      const value = argv[++index];
      if (!value) fail("argument_missing");
      if (argument === "--output") options.output = resolve(root, value);
      else if (argument === "--platform") options.platform = value;
      else {
        const agentId = value.trim().toLowerCase().replaceAll("_", "-");
        if (!/^[a-z0-9-]{1,64}$/u.test(agentId)) fail("agent_id_invalid");
        options.agents.push(agentId);
      }
    } else fail("argument_unsupported");
  }
  if (!["macos", "linux", "windows"].includes(options.platform)) {
    fail("platform_unsupported");
  }
  options.agents = [...new Set(options.agents.length > 0 ? options.agents : [defaultAgent])];
  return options;
}

function assertExactFields(value, allowed, reasonCode) {
  if (!value || typeof value !== "object" || Array.isArray(value)) fail(reasonCode);
  if (Object.keys(value).some((field) => !allowed.has(field))) fail(reasonCode);
}

function decodeLiveReceipt(encodedOutput, expectedAgent = defaultAgent, expectedChallengeDigest) {
  const matches = String(encodedOutput || "")
    .split(/\r?\n/u)
    .filter((candidate) => candidate.startsWith(sentinel));
  if (matches.length !== 1) fail("release_ui_live_receipt_missing_or_ambiguous");
  let receipt;
  try {
    receipt = JSON.parse(Buffer.from(matches[0].slice(sentinel.length), "base64url").toString("utf8"));
  } catch {
    fail("release_ui_live_receipt_invalid");
  }
  assertExactFields(receipt, liveReceiptFields, "release_ui_live_receipt_unbounded");
  if (receipt.status === "failed") {
    const reasonCode = typeof receipt.reasonCode === "string"
      && /^[a-z0-9_-]{1,96}$/u.test(receipt.reasonCode)
      ? receipt.reasonCode
      : "release_ui_live_receipt_failed";
    fail(reasonCode);
  }
  const expectedModel = validationModels[expectedAgent];
  if (!expectedModel) fail("verification_model_unconfigured");
  const valid = receipt.schemaVersion === "lico-agent-conversation-release-ui-live-v1"
    && receipt.status === "passed"
    && receipt.reasonCode === undefined
    && receipt.receiptKind === "release-ui-live"
    && receipt.releaseMode === true
    && receipt.packagedApplicationProcess === true
    && receipt.packagedSidecarUsed === true
    && receipt.fixtureBackend === false
    && receipt.agentId === expectedAgent
    && typeof receipt.model === "string"
    && receipt.model.length > 0
    && receipt.model.length <= 128
    && receipt.model === expectedModel
    && typeof receipt.nativeSessionId === "string"
    && receipt.nativeSessionId.length > 0
    && receipt.nativeSessionId.length <= 512
    && receipt.composerSubmitted === true
    && receipt.progressiveTimelineVisible === true
    && receipt.sameNativeSessionId === true
    && receipt.historyReadback === true
    && receipt.turnCount === 2;
  const challengeBound = receipt.invocationChallengeDigest === expectedChallengeDigest;
  if (!valid || !challengeBound) fail("release_ui_live_receipt_incomplete");
  return receipt;
}

function encodedLiveReceipt(overrides = {}) {
  const receipt = {
    schemaVersion: "lico-agent-conversation-release-ui-live-v1",
    status: "passed",
    receiptKind: "release-ui-live",
    releaseMode: true,
    packagedApplicationProcess: true,
    packagedSidecarUsed: true,
    fixtureBackend: false,
    agentId: defaultAgent,
    model: selfTestModel,
    nativeSessionId: "self-test-native-session",
    composerSubmitted: true,
    progressiveTimelineVisible: true,
    sameNativeSessionId: true,
    historyReadback: true,
    turnCount: 2,
    invocationChallengeDigest: selfTestChallengeDigest,
    ...overrides,
  };
  return `${sentinel}${Buffer.from(JSON.stringify(receipt)).toString("base64url")}`;
}

function occurrenceCount(source, token) {
  return source.split(token).length - 1;
}

function selfTest() {
  const runnerSource = readFileSync(fileURLToPath(import.meta.url), "utf8");
  const liveSource = readFileSync(resolve(desktopRoot, liveSourceRef), "utf8");
  const mainSource = readFileSync(resolve(desktopRoot, "lib/main.dart"), "utf8");
  const packageSource = [
    "scripts/package-client.mjs",
    "scripts/package-client/build/flutter.mjs",
  ].map((ref) => readFileSync(resolve(desktopRoot, ref), "utf8")).join("\n");
  const widgetSource = readFileSync(resolve(desktopRoot, widgetTestRef), "utf8");
  const fixtureSource = readFileSync(resolve(desktopRoot, fixtureRef), "utf8");
  const parsed = decodeLiveReceipt(
    encodedLiveReceipt(),
    defaultAgent,
    selfTestChallengeDigest,
  );
  const rejects = [];
  for (const invalid of [
    { releaseMode: false },
    { packagedApplicationProcess: false },
    { packagedSidecarUsed: false },
    { fixtureBackend: true },
    { receiptKind: "fixture" },
    { progressiveTimelineVisible: false },
    { sameNativeSessionId: false },
    { historyReadback: false },
    { nativeSessionId: "" },
    { turnCount: 1 },
    { agentId: "unexpected-agent" },
    { invocationChallengeDigest: `sha256:${"b".repeat(64)}` },
    { reasonCode: "unexpected_success_reason" },
    { releaseUiPassed: true },
  ]) {
    try {
      decodeLiveReceipt(
        encodedLiveReceipt(invalid),
        defaultAgent,
        selfTestChallengeDigest,
      );
      rejects.push(false);
    } catch {
      rejects.push(true);
    }
  }
  const directLiveSource = liveSource.split(
    "Future<void> _runGroupAssistant",
  )[0];
  const oneConversationTwoMessagesBound =
    occurrenceCount(directLiveSource, "controller.startNewConversationSession();") === 1
    && occurrenceCount(directLiveSource, "await _submitComposer(") === 2
    && !directLiveSource.includes(".steer(")
    && !runnerSource.includes([
      "LICO_AGENT_CONVERSATION_PRODUCT",
      "STEER_PROMPT",
    ].join("_"));
  const sourceBound = runnerSource.includes('"client:build"')
    && runnerSource.includes('"--agent-conversation-release-live"')
    && liveSource.includes("ClientController()")
    && liveSource.includes("initializeController: false")
    && liveSource.includes("initializeWithOptions(runBackgroundSteps: false)")
    && liveSource.includes("fixtureBackend': false")
    && liveSource.includes("agent-conversation-composer-field")
    && liveSource.includes("liveConversationMessagesByScope")
    && liveSource.includes("conversationLiveScopeKeysForAgent")
    && liveSource.includes("exact native-session readback")
    && liveSource.includes("LICO_AGENT_CONVERSATION_PRODUCT_GROUP_ASSISTANT")
    && liveSource.includes("_verifyAssistantControl(")
    && liveSource.includes("updateMembershipProfileIntent(")
    && liveSource.includes("_groupRawReplyExists(")
    && mainSource.includes("runAgentConversationReleaseLive")
    && packageSource.includes("agentConversationReleaseLive")
    && packageSource.includes("LICO_AGENT_CONVERSATION_RELEASE_LIVE=true")
    && widgetSource.includes("createAcceptanceController")
    && fixtureSource.includes("AcceptanceConversationService")
    && runnerSource.includes('LICO_CLIENT_PATH: join(appBundle, "Contents/Helpers/LicoUpCustody.app/Contents/MacOS/licoup-cli")')
    && runnerSource.includes('LICO_AGENT_CONVERSATION_ACCEPTANCE: "dispatch-lane-unified-1"')
    && runnerSource.includes("delete runtimeEnvironment.CARGO_TARGET_DIR")
    && runnerSource.includes("releaseUiPassed: false")
    && oneConversationTwoMessagesBound;
  const bindingInput = {
    artifactDigest: `sha256:${"1".repeat(64)}`,
    invocationChallengeDigest: selfTestChallengeDigest,
    agentId: defaultAgent,
    model: selfTestModel,
    nativeDigest: nativeContinuityDigest("self-test-native-session"),
  };
  const binding = productContinuityBindingDigest(bindingInput);
  const bindingBound = [
    { ...bindingInput, artifactDigest: `sha256:${"2".repeat(64)}` },
    { ...bindingInput, invocationChallengeDigest: `sha256:${"3".repeat(64)}` },
    { ...bindingInput, agentId: "cursor" },
    { ...bindingInput, model: "different-model" },
    { ...bindingInput, nativeDigest: `sha256:${"4".repeat(64)}` },
  ].every((candidate) => productContinuityBindingDigest(candidate) !== binding);
  return {
    schemaVersion: "lico-agent-conversation-product-e2e-self-test-v3",
    status: sourceBound && bindingBound && rejects.every(Boolean) && parsed.fixtureBackend === false
      ? "passed"
      : "failed",
    fixtureReceiptRejected: rejects[3] === true,
    releaseReceiptFailClosed: rejects.every(Boolean),
    packagedLivePathBound: sourceBound,
    oneConversationTwoMessagesBound,
    invocationBindingRecomputed: bindingBound,
  };
}

function readBundleExecutable(appBundle) {
  const plist = readFileSync(join(appBundle, "Contents/Info.plist"), "utf8");
  const match = plist.match(/<key>CFBundleExecutable<\/key>\s*<string>([^<]+)<\/string>/u);
  if (!match || !/^[A-Za-z0-9._ -]+$/u.test(match[1])) fail("release_app_executable_invalid");
  return join(appBundle, "Contents/MacOS", match[1]);
}

function bundleDigest(appBundle) {
  const hash = createHash("sha256");
  let fileCount = 0;
  function visit(directory) {
    const entries = readdirSync(directory, { withFileTypes: true })
      .sort((left, right) => left.name.localeCompare(right.name, "en"));
    for (const entry of entries) {
      const path = join(directory, entry.name);
      const name = relative(appBundle, path).replaceAll("\\", "/");
      const stat = lstatSync(path);
      if (stat.isDirectory()) {
        hash.update(`d\0${name}\0`);
        visit(path);
      } else if (stat.isSymbolicLink()) {
        hash.update(`l\0${name}\0${readlinkSync(path)}\0`);
        fileCount += 1;
      } else if (stat.isFile()) {
        hash.update(`f\0${name}\0`);
        hash.update(readFileSync(path));
        fileCount += 1;
      }
    }
  }
  visit(appBundle);
  return { digest: `sha256:${hash.digest("hex")}`, fileCount };
}

function buildPackagedReleaseApplication() {
  const execution = spawnSync(
    "npm",
    [
      "run",
      "client:build",
      "--",
      "--platform",
      "macos",
      "--agent-conversation-release-live",
    ],
    {
      cwd: root,
      encoding: "utf8",
      env: process.env,
      maxBuffer: 8 * 1024 * 1024,
      timeout: 30 * 60 * 1000,
    },
  );
  if (execution.status !== 0) {
    if (execution.error?.code === "ETIMEDOUT") fail("release_app_build_timeout");
    const safeBuildErrors = new Set([
      "conversation_parity_readiness_failed",
      "flutter_app_build_failed",
      "flutter_toolchain_unavailable",
      "macos_app_icon_verification_failed",
      "native_sidecar_build_failed",
      "package_subprocess_failed",
      "release_packaging_config_changed_during_build",
      "release_source_attestation_changed_during_build",
      "release_source_changed_during_build",
      "swift_sidecar_build_failed",
    ]);
    const output = `${execution.stdout || ""}\n${execution.stderr || ""}`;
    const receipts = output.split(/\r?\n/u).flatMap((line) => {
      try {
        const receipt = JSON.parse(line);
        return receipt?.ok === false && safeBuildErrors.has(receipt.error) ? [receipt] : [];
      } catch {
        return [];
      }
    });
    const buildReceipt = receipts.at(-1);
    const safeChangedRefs = buildReceipt?.error === "release_source_changed_during_build"
      ? (buildReceipt.changedSourceRefs || []).filter((sourceRef) =>
          typeof sourceRef === "string" &&
          sourceRef.length <= 240 &&
          !sourceRef.startsWith("/") &&
          !sourceRef.split("/").includes("..") &&
          /^[A-Za-z0-9._/-]+$/u.test(sourceRef)).slice(0, 32)
      : [];
    const details = safeChangedRefs.length > 0
      ? {
          changedSourceCount: Number.isSafeInteger(buildReceipt.changedSourceCount)
            ? buildReceipt.changedSourceCount
            : safeChangedRefs.length,
          changedSourceRefs: safeChangedRefs,
          changedSourceRefsTruncated: buildReceipt.truncated === true,
        }
      : null;
    fail(buildReceipt
      ? `release_app_build_${buildReceipt.error}`
      : "release_app_build_failed", details);
  }
  if (!existsSync(runnableApp)) fail("packaged_release_app_missing");
  if (!existsSync(join(runnableApp, "Contents/Helpers/LicoUpCustody.app/Contents/MacOS/licoup-cli"))) {
    fail("packaged_release_sidecar_missing");
  }
  return runnableApp;
}

function runReleaseApplication(appBundle, agentId, invocationChallengeDigest) {
  const executable = readBundleExecutable(appBundle);
  const receiptDirectory = mkdtempSync(join(tmpdir(), "lico-p10-live-receipt-"));
  const receiptPath = join(receiptDirectory, "receipt.txt");
  const sessionPath = join(receiptDirectory, "session.json");
  const isolatedRuntimeRoot = join(receiptDirectory, "runtime-state");
  const model = validationModels[agentId];
  if (!model) fail("verification_model_unconfigured");
  const firstPrompt = "Hi";
  const secondPrompt = "Hi";
  try {
    const runtimeEnvironment = {
      ...process.env,
      // Pin the live product process to the sidecar inside the exact app bundle
      // whose digest is joined into the receipt. Never inherit a debug target.
      LICO_CLIENT_PATH: join(appBundle, "Contents/Helpers/LicoUpCustody.app/Contents/MacOS/licoup-cli"),
      LICO_AGENT_CONVERSATION_ACCEPTANCE: "dispatch-lane-unified-1",
      ...(agentId === "kimi-code" ? { KIMI_CODE_HOME: isolatedRuntimeRoot } : {}),
      ...(agentId === "pi" ? { PI_CODING_AGENT_SESSION_DIR: isolatedRuntimeRoot } : {}),
    };
    delete runtimeEnvironment.CARGO_TARGET_DIR;
    const execution = spawnSync(executable, [], {
      cwd: dirname(appBundle),
      encoding: "utf8",
      env: {
        ...runtimeEnvironment,
        LICO_AGENT_CONVERSATION_PRODUCT_AGENT: agentId,
        LICO_AGENT_CONVERSATION_PRODUCT_MODEL: model,
        LICO_AGENT_CONVERSATION_PRODUCT_EFFORT: verificationEffortForAgent(agentId, model),
        LICO_AGENT_CONVERSATION_PRODUCT_FIRST_PROMPT: firstPrompt,
        LICO_AGENT_CONVERSATION_PRODUCT_SECOND_PROMPT: secondPrompt,
        LICO_AGENT_CONVERSATION_PRODUCT_RECEIPT: receiptPath,
        LICO_AGENT_CONVERSATION_PRODUCT_CHALLENGE_DIGEST: invocationChallengeDigest,
      },
      maxBuffer: 1024 * 1024,
      timeout: 25 * 60 * 1000,
    });
    if (execution.error?.code === "ETIMEDOUT") fail("release_app_acceptance_timeout");
    if (!existsSync(receiptPath)) fail("release_app_acceptance_receipt_missing");
    const receipt = decodeLiveReceipt(
      readFileSync(receiptPath, "utf8"),
      agentId,
      invocationChallengeDigest,
    );
    if (execution.status !== 0) fail("release_app_acceptance_exit_failed");
    writeFileSync(sessionPath, `${JSON.stringify({
      schemaVersion: "lico-agent-conversation-product-session-v1",
      agentId,
      nativeSessionId: receipt.nativeSessionId,
    })}\n`, { mode: 0o600 });
    const cleanup = spawnSync("node", [
      "tests/product-e2e/cli/agent-conversations/support/parity-facade.mjs",
      "--agent", agentId,
      "--cleanup-product-session", sessionPath,
    ], {
      cwd: root,
      encoding: "utf8",
      env: runtimeEnvironment,
      maxBuffer: 256 * 1024,
      timeout: 90 * 1000,
    });
    let cleanupReceipt = null;
    try { cleanupReceipt = JSON.parse((cleanup.stdout || "").trim()); } catch { /* fail closed below */ }
    if (cleanup.status !== 0 || cleanupReceipt?.cleanupPassed !== true) {
      fail(cleanup.error?.code === "ETIMEDOUT"
        ? "release_app_cleanup_timeout"
        : "release_app_cleanup_failed");
    }
    return { receipt, cleanupPassed: true };
  } finally {
    rmSync(receiptDirectory, { recursive: true, force: true });
  }
}

function runProductAcceptance(options) {
  if (options.platform !== "macos" || process.platform !== "darwin") {
    fail("release_app_platform_unsupported");
  }
  const appBundle = buildPackagedReleaseApplication();
  const artifact = bundleDigest(appBundle);
  const invocationChallengeDigest = releaseClosureChallengeDigest(
    requiredReleaseClosureChallenge(process.env),
  );
  const receipts = options.agents.map((agentId) =>
    runReleaseApplication(appBundle, agentId, invocationChallengeDigest));
  const report = {
    schemaVersion: "lico-agent-conversation-product-e2e-report-v3",
    status: "passed",
    receiptKind: "release-ui-live-product",
    platform: "macos",
    buildMode: "release",
    productHarnessKind: "packaged-release-app-live-runtime",
    fixtureBackend: false,
    productLivePassed: true,
    releaseUiPassed: false,
    cleanupPassed: receipts.every((entry) => entry.cleanupPassed),
    coreJoinRequired: true,
    externalRuntimeInvoked: true,
    invocationChallengeDigest,
    testedAgents: receipts.map(({ receipt, cleanupPassed }) => {
      const nativeDigest = nativeContinuityDigest(receipt.nativeSessionId);
      return {
        agentId: receipt.agentId,
        model: receipt.model,
        turnCount: receipt.turnCount,
        productLivePassed: true,
        releaseUiPassed: false,
        cleanupPassed,
        nativeContinuityDigest: nativeDigest,
        productContinuityBindingDigest: productContinuityBindingDigest({
          artifactDigest: artifact.digest,
          invocationChallengeDigest,
          agentId: receipt.agentId,
          model: receipt.model,
          nativeDigest,
        }),
      };
    }),
    testedAgentCount: receipts.length,
    composerSubmitted: receipts.every(({ receipt }) => receipt.composerSubmitted),
    progressiveTimelineVisible: receipts.every(({ receipt }) => receipt.progressiveTimelineVisible),
    sameNativeSessionId: receipts.every(({ receipt }) => receipt.sameNativeSessionId),
    historyReadback: receipts.every(({ receipt }) => receipt.historyReadback),
    artifactDigest: artifact.digest,
    artifactFileCount: artifact.fileCount,
    artifactName: basename(appBundle),
  };
  mkdirSync(dirname(options.output), { recursive: true });
  writeFileSync(options.output, `${JSON.stringify(report, null, 2)}\n`, { mode: 0o600 });
  return report;
}

let output;
try {
  const options = parseArgs(process.argv.slice(2));
  output = options.selfTest ? selfTest() : runProductAcceptance(options);
} catch (error) {
  const reasonCode = /^[a-z0-9_-]+$/u.test(error?.message || "")
    ? error.message
    : "unexpected_failure";
  output = {
    schemaVersion: "lico-agent-conversation-product-e2e-report-v3",
    status: "failed",
    reasonCode,
    ...(error instanceof ProductAcceptanceError && error.details
      ? error.details
      : {}),
  };
}

process.stdout.write(`${JSON.stringify(output)}\n`);
if (output.status !== "passed") process.exitCode = 1;
