#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import { mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import {
  RELEASE_CLOSURE_CHALLENGE_ENV,
  createReleaseClosureChallenge,
} from "./lib/release-closure-challenge.mjs";
import { verificationModelsMap } from "./lib/agent-conversation-verification-models.mjs";
import { strictRoundCount } from "../../tests/product-e2e/cli/agent-conversations/support/parity/constants.mjs";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const inventoryPath = resolve(root, "crates/licoup-native/resources/agent-conversation-drivers.json");
const readinessPath = resolve(root, "crates/licoup-native/resources/agent-conversation-readiness.json");
const defaultReport = resolve(root, "build/reports/agent-conversation-verification.json");
const validationModels = verificationModelsMap();

function parseArgs(argv) {
  const options = { agents: [], live: false, releaseUi: false, selfTest: false, output: defaultReport };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--live") options.live = true;
    else if (arg === "--release-ui") { options.live = true; options.releaseUi = true; }
    else if (arg === "--self-test") options.selfTest = true;
    else if (arg === "--agent" || arg === "--output") {
      const value = argv[++index];
      if (!value) throw new Error("argument_missing");
      if (arg === "--agent") options.agents.push(value.trim().toLowerCase().replaceAll("_", "-"));
      else options.output = resolve(root, value);
    } else throw new Error("argument_unsupported");
  }
  return options;
}

function run(command, args, timeoutMs = 15 * 60 * 1000) {
  const result = spawnSync(command, args, {
    cwd: root,
    encoding: "utf8",
    env: process.env,
    maxBuffer: 16 * 1024 * 1024,
    timeout: timeoutMs,
  });
  return {
    status: result.status === 0 ? "passed" : "failed",
    reasonCode: result.status === 0
      ? null
      : result.error?.code === "ETIMEDOUT"
        ? "command_timeout"
        : result.error
          ? "process_start_failed"
          : "command_failed",
  };
}

function runJson(command, args, environment = process.env, timeoutMs = 45 * 60 * 1000) {
  const execution = spawnSync(command, args, {
    cwd: root,
    encoding: "utf8",
    env: environment,
    maxBuffer: 16 * 1024 * 1024,
    timeout: timeoutMs,
  });
  let payload = null;
  try { payload = JSON.parse((execution.stdout || "").trim()); } catch { /* redacted classification below */ }
  return {
    status: execution.status === 0 ? "passed" : "failed",
    reasonCode: execution.status === 0
      ? null
      : payload?.reasonCode
        || payload?.errorCode
        || (execution.error?.code === "ETIMEDOUT"
          ? "command_timeout"
          : execution.error
            ? "process_start_failed"
            : "command_failed"),
    payload,
  };
}

function markdownCell(value) {
  return String(value ?? "").replaceAll("|", "\\|").replaceAll("\n", " ");
}

function tableMetrics(liveResult, staticStatus) {
  const payload = liveResult?.rawResult;
  const required = Number(payload?.roundsRequired || 0);
  const completed = Number(payload?.roundsCompleted || 0);
  const testedSessions = Number(payload?.testedSessions ?? completed * 2);
  const requests = Number(payload?.requestCount ?? completed * 4);
  const successfulRequests = Number(payload?.successfulRequestCount ?? completed * 4);
  const rate = requests > 0
    ? `${Math.round((successfulRequests / requests) * 100)}%`
    : required > 0
    ? `${Math.round((Math.min(completed, required) / required) * 100)}%`
    : staticStatus === "passed" ? "100%" : "0%";
  return { rate, testedSessions, requests };
}

export function classifyAdapter(
  driver,
  readiness,
  liveResult,
  liveRequested,
  releaseUiRequested,
  productRelease = null,
) {
  const blocked = driver.driverMode === "blocked";
  const staticStatus = driver.historyReadable === true && driver.capabilityMatrix?.officialLane === true
    ? "passed"
    : blocked ? "blocked" : "failed";
  let liveStatus = "not-run";
  let liveReason = liveRequested ? "live_harness_not_run" : "live_verification_not_requested";
  if (blocked) {
    liveStatus = "blocked";
    liveReason = driver.blockerCodes?.[0] || "official_native_lane_missing";
  } else if (liveResult) {
    liveStatus = liveResult.status;
    liveReason = liveResult.reasonCode;
  }
  const metrics = tableMetrics(liveResult, staticStatus);
  const rawResult = liveResult?.rawResult || {
    readinessStatus: readiness?.status || "unknown",
    sendEnabled: readiness?.sendEnabled === true,
    summaryCodes: readiness?.summaryCodes || [],
  };
  const conversationPassed = liveResult?.rawResult?.conversationPassed === true;
  const readinessReady = readiness?.status === "ready" && readiness?.sendEnabled === true;
  const productAgentPassed = productRelease?.status === "passed"
    && productRelease?.testedAgents?.some((row) =>
      row?.agentId === driver.agentId
        && row?.productLivePassed === true
        && row?.cleanupPassed === true)
    && liveResult?.rawResult?.conversationGatePassed === true
    && liveResult?.rawResult?.productReceiptJoined === true;
  const staticReason = readiness?.summaryCodes?.[0]
    || driver.blockerCodes?.[0]
    || "acceptance_evidence_unavailable";
  const resultLabel = liveRequested
    ? liveStatus === "passed" || conversationPassed ? "成功" : `Failed: ${liveReason}`
    : readinessReady ? "成功" : `Not ready: ${staticReason}`;
  return {
    agentId: driver.agentId,
    driverId: driver.driverId,
    laneFamily: driver.capabilityMatrix?.laneFamily || "unknown",
    validationModel: validationModels[driver.agentId] || null,
    staticStatus,
    liveStatus,
    releaseStatus: readinessReady && (!releaseUiRequested || productAgentPassed)
      ? "passed"
      : "not-ready",
    sendEnabled: readiness?.sendEnabled === true,
    consecutivePasses: readiness?.consecutivePasses || 0,
    resultLabel,
    passRate: metrics.rate,
    testedSessions: metrics.testedSessions,
    requestCount: metrics.requests,
    rawResult,
    reasonCodes: [...new Set([
      liveReason,
      ...(readiness?.summaryCodes || []),
      releaseUiRequested && !productAgentPassed
        ? productRelease?.reasonCode || "release_ui_product_evidence_incomplete"
        : null,
      releaseUiRequested && readiness?.sendEnabled !== true ? "release_ui_evidence_incomplete" : null,
    ].filter(Boolean))],
  };
}

function markdownReport(report) {
  const lines = [
    "| 智能体 | 测试是否成功 | 通过率 | 测试会话数 | 请求数 | 运行原始返回值（脱敏） |",
    "| --- | --- | ---: | ---: | ---: | --- |",
  ];
  for (const row of report.adapters) {
    lines.push(`| ${markdownCell(row.agentId)} | ${markdownCell(row.resultLabel)} | ${row.passRate} | ${row.testedSessions} | ${row.requestCount} | ${markdownCell(JSON.stringify(row.rawResult))} |`);
  }
  return `${lines.join("\n")}\n`;
}

function selfTest() {
  const blocked = classifyAdapter(
    { agentId: "blocked", driverId: "blocked-driver", driverMode: "blocked", historyReadable: true,
      blockerCodes: ["native_transport_missing"], capabilityMatrix: { officialLane: false, laneFamily: "unavailable" } },
    { sendEnabled: false, consecutivePasses: 0, summaryCodes: ["native_transport_missing"] }, null, true, false,
  );
  const ready = classifyAdapter(
    { agentId: "ready", driverId: "ready-driver", driverMode: "conversation", historyReadable: true,
      blockerCodes: [], capabilityMatrix: { officialLane: true, laneFamily: "rpc" } },
    { status: "ready", sendEnabled: true, consecutivePasses: strictRoundCount, summaryCodes: [] },
    { status: "passed", reasonCode: null, rawResult: { conversationGatePassed: true, productReceiptJoined: true } },
    true,
    true,
    { status: "passed", testedAgents: [{ agentId: "ready", productLivePassed: true, cleanupPassed: true }] },
  );
  const staticUnverified = classifyAdapter(
    { agentId: "unverified", driverId: "unverified-driver", driverMode: "conversation", historyReadable: true,
      blockerCodes: [], capabilityMatrix: { officialLane: true, laneFamily: "rpc" } },
    { status: "unverified", sendEnabled: false, consecutivePasses: 0, summaryCodes: ["evidence_missing"] },
    null,
    false,
    false,
  );
  const ok = blocked.liveStatus === "blocked" && blocked.sendEnabled === false
    && ready.staticStatus === "passed" && ready.liveStatus === "passed" && ready.releaseStatus === "passed"
    && staticUnverified.staticStatus === "passed"
    && staticUnverified.resultLabel === "Not ready: evidence_missing"
    && staticUnverified.resultLabel !== "成功";
  console.log(JSON.stringify({ schemaVersion: "lico-agent-conversation-verifier-self-test-v1", status: ok ? "passed" : "failed" }));
  process.exitCode = ok ? 0 : 1;
}

async function main() {
  const options = parseArgs(process.argv.slice(2));
  if (options.selfTest) return selfTest();

  const inventory = JSON.parse(readFileSync(inventoryPath, "utf8"));
  let readiness = JSON.parse(readFileSync(readinessPath, "utf8"));
  const selected = new Set(options.agents);
  const drivers = inventory.drivers.filter((driver) => selected.size === 0 || selected.has(driver.agentId));
  if (drivers.length === 0) throw new Error("agent_not_found");

  const checks = {
    nativePlatform: run("cargo", ["test", "-p", "licoup-native", "--lib", "platform::", "--", "--test-threads=1"]),
    reducerContract: run("node", ["tests/contract/client/agent-conversation-parity-reducer.test.mjs"]),
    readinessContract: run("node", ["tests/product-e2e/cli/agent-conversations/support/reducer-facade.mjs", "--check"]),
    harnessSelfTest: runJson("node", ["tests/product-e2e/cli/agent-conversations/support/parity-facade.mjs", "--self-test"]),
    productHarnessSelfTest: runJson("node", ["tools/scripts/client-agent-conversation-product-e2e.mjs", "--self-test"]),
  };

  let productRelease = null;
  const releaseInvocationEnvironment = options.releaseUi
    ? {
      ...process.env,
      [RELEASE_CLOSURE_CHALLENGE_ENV]: createReleaseClosureChallenge(),
    }
    : process.env;
  const productReceiptPath = resolve(root, "build/reports/agent-conversation-product-e2e.json");
  if (options.releaseUi) {
    rmSync(productReceiptPath, { force: true });
    const productArgs = [
      "tools/scripts/client-agent-conversation-product-e2e.mjs",
      "--output", "build/reports/agent-conversation-product-e2e.json",
    ];
    for (const driver of drivers) {
      if (driver.driverMode !== "blocked") productArgs.push("--agent", driver.agentId);
    }
    const execution = runJson(
      "node",
      productArgs,
      releaseInvocationEnvironment,
      60 * 60 * 1000,
    );
    productRelease = execution.payload || {
      status: "failed",
      reasonCode: execution.reasonCode || "release_ui_product_harness_failed",
    };
  }

  const rows = [];
  for (const driver of drivers) {
    let liveResult = null;
    if (options.live && driver.driverMode !== "blocked") {
      if (options.releaseUi && productRelease?.status !== "passed") {
        const reasonCode = /^[a-z0-9_-]{1,96}$/u.test(productRelease?.reasonCode || "")
          ? productRelease.reasonCode
          : "release_ui_product_harness_failed";
        liveResult = {
          status: "failed",
          reasonCode,
          rawResult: {
            status: "failed",
            conversationGatePassed: false,
            errorCode: reasonCode,
          },
        };
      } else {
        const args = [
          "tests/product-e2e/cli/agent-conversations/support/parity-facade.mjs",
          "--agent", driver.agentId,
          "--strict",
          "--timeout-ms", "180000",
        ];
        if (options.releaseUi) {
          args.push("--release-ui", "--product-receipt", productReceiptPath);
        }
        const execution = runJson("node", args, releaseInvocationEnvironment);
        liveResult = {
          status: execution.status,
          reasonCode: execution.payload?.errorCode || execution.reasonCode,
          rawResult: execution.payload,
        };
      }
    }
    if (options.releaseUi && liveResult?.status === "passed") {
      const reducerWrite = runJson("node", [
        "tests/product-e2e/cli/agent-conversations/support/reducer-facade.mjs",
        "--write",
      ]);
      checks.releaseReducerWrite = reducerWrite;
      if (reducerWrite.status === "passed") {
        readiness = JSON.parse(readFileSync(readinessPath, "utf8"));
      }
    }
    rows.push(classifyAdapter(
      driver,
      readiness.adapters.find((row) => row.agentId === driver.agentId),
      liveResult,
      options.live,
      options.releaseUi,
      productRelease,
    ));
  }

  if (options.releaseUi) {
    // The P-10 application is an acceptance-only Release artifact. Restore a
    // normal Release build before handoff so it cannot be mistaken for the
    // ordinary GitHub Release/package artifact.
    checks.ordinaryReleaseRebuild = run("npm", [
      "run",
      "client:build",
      "--",
      "--platform",
      "macos",
    ]);
  }

  const staticPassed = Object.values(checks).every((check) => check.status === "passed");
  const requestedLivePassed = !options.live || rows.every((row) => ["passed", "blocked"].includes(row.liveStatus));
  const productReleasePassed = !options.releaseUi || productRelease?.status === "passed";
  const requestedReleasePassed = !options.releaseUi
    || productReleasePassed
      && rows.every((row) => row.releaseStatus === "passed" || row.liveStatus === "blocked");
  const report = {
    schemaVersion: "lico-agent-conversation-verification-v1",
    generatedAt: new Date().toISOString(),
    mode: options.releaseUi ? "release-ui" : options.live ? "live" : "static",
    status: staticPassed && requestedLivePassed && requestedReleasePassed ? "passed" : "failed",
    releaseUiProduct: productRelease,
    checks,
    summary: {
      total: rows.length,
      staticPassed: rows.filter((row) => row.staticStatus === "passed").length,
      livePassed: rows.filter((row) => row.liveStatus === "passed").length,
      blocked: rows.filter((row) => row.liveStatus === "blocked").length,
      releaseReady: rows.filter((row) => row.releaseStatus === "passed").length,
    },
    adapters: rows,
  };
  mkdirSync(dirname(options.output), { recursive: true });
  writeFileSync(options.output, `${JSON.stringify(report, null, 2)}\n`, { mode: 0o600 });
  const markdownPath = options.output.replace(/\.json$/u, ".md");
  const markdown = markdownReport(report);
  writeFileSync(markdownPath, markdown, { mode: 0o600 });
  process.stdout.write(markdown);
  process.stdout.write(`\nJSON: ${options.output.replace(`${root}/`, "")}\nMarkdown: ${markdownPath.replace(`${root}/`, "")}\n`);
  process.exitCode = report.status === "passed" ? 0 : 1;
}

main().catch((error) => {
  console.log(JSON.stringify({
    schemaVersion: "lico-agent-conversation-verification-v1",
    status: "failed",
    reasonCode: /^[a-z0-9_-]+$/u.test(error?.message || "") ? error.message : "unexpected_failure",
  }));
  process.exitCode = 1;
});
