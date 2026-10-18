#!/usr/bin/env node

/**
 * Nightly upstream-latest observation probe for the 13 packaged agent adapters.
 *
 * This tool extends the staged client-module regression tooling with a strictly
 * READ-ONLY observation job. It never starts a conversation, never injects a
 * prompt, and never opens, resumes, streams, or cancels a session: a healthy
 * agent may be busy, and forced-reply probes are a serious logic hole. Observed
 * facts are:
 *
 *   install     presence of the configured agent executable (environment and
 *               PATH candidates, plus the authoritative regression entry probe
 *               that scans the LicoUp sidecar inventory). The agent executable
 *               itself is never executed.
 *   version     a measured verdict, not a declaration echo. The adapter
 *               manifest's officialCapabilityAssessment.versionSupport declares
 *               a version interval or a machine-readable unknown, and a recorded
 *               probe result supplied with --observed-versions carries the
 *               version an Agent reported. The two are compared and produce
 *               exactly one of supported, unsupported or unknown. With no
 *               recorded observation, or with a manifest that declares no bound,
 *               the answer is unknown: this tool never invents a version and
 *               never invents a bound. The declaration facts
 *               (assessedVersion, versionProbe) and the last successful
 *               evidence-bind from crate resources
 *               agent-conversation-evidence.json are still reported beside the
 *               verdict, and an unsupported version is breakage.
 *   capability  driver inventory capability matrix and adapter manifest
 *               presence/driver binding (conformance is enforced by
 *               client-agent-adapter-standard.mjs in the staged regression).
 *   handshake   the LicoUp sidecar `agent conversation capabilities` dispatch
 *               handshake (bounded, fixture-free, no session). Only a built
 *               local sidecar is queried; absent hardware degrades to
 *               "unverified", never to a fake pass.
 *
 * Report contract: privacy-safe — public agent ids plus anonymous indexes,
 * allowlisted reason codes, and booleans. No repository paths, no captured
 * output, no environment values are ever persisted.
 */

import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import {
  AGENT_REGRESSION_ENTRIES,
  validateClientRegressionEntries,
} from "../regression/client-regression-entries/index.mjs";
import {
  agentConfigs,
  dispatchLaneHarnessVersion,
  driversInventoryPath,
  evidenceManifestPath,
  workspaceRoot,
} from "../../tests/product-e2e/cli/agent-conversations/support/parity/constants.mjs";
import {
  resolveExecutable,
  resolveSidecar,
  runDispatchLaneCli,
} from "../../tests/product-e2e/cli/agent-conversations/support/parity/sidecar.mjs";

const manifestDirectory = path.resolve(
  workspaceRoot,
  "packages",
  "contracts",
  "client",
  "fixtures",
  "agent-conversation-adapter",
  "manifests",
);
const defaultReportPath = path.resolve(workspaceRoot, "build/reports/upstream-agent-probe.json");

const reasonCodes = new Set([
  "adapter_manifest_missing",
  "agent_binary_unavailable",
  "agent_executable_unavailable",
  "agent_version_no_declared_bound",
  "agent_version_not_comparable",
  "agent_version_not_observed",
  "agent_version_unsupported",
  "capabilities_handshake_failed",
  "agent_live_harness_unavailable",
  "dispatch_lane_family_drift",
  "lico_client_executable_unavailable",
  "runtime_protocol_drift",
  "upstream_evidence_missing",
]);

const VERSION_SUPPORT_STATUSES = Object.freeze(["supported", "unsupported", "unknown"]);
const observedVersionsSchemaVersion = "licoup.agent-observed-versions.v1";

function safeReason(value, fallback) {
  return typeof value === "string" && reasonCodes.has(value) ? value : fallback;
}

function parseArgs(argv) {
  const options = { reportPath: defaultReportPath };
  for (let index = 0; index < argv.length; index += 1) {
    if (argv[index] === "--observed-versions") {
      const value = argv[++index];
      if (!value) throw new Error("upstream agent probe observed version path is required");
      const relative = path.posix.normalize(value.replaceAll("\\", "/").replace(/^\.\//u, ""));
      if (relative.startsWith("/") || relative.startsWith("../")) {
        throw new Error("upstream agent probe observed versions must stay inside the workspace");
      }
      options.observedVersionPath = path.resolve(workspaceRoot, relative);
    } else if (argv[index] === "--self-test") {
      options.selfTest = true;
    } else if (argv[index] === "--output") {
      const value = argv[++index];
      if (!value) throw new Error("upstream agent probe output path is required");
      const relative = path.posix.normalize(value.replaceAll("\\", "/").replace(/^\.\//u, ""));
      if (relative.startsWith("/") || relative.startsWith("../") || !relative.startsWith("build/reports/")) {
        throw new Error("upstream agent probe reports must stay under build/reports");
      }
      options.reportPath = path.resolve(workspaceRoot, relative);
    } else {
      throw new Error("unknown upstream agent probe option");
    }
  }
  return options;
}

function readJson(relativePath) {
  return JSON.parse(readFileSync(path.resolve(workspaceRoot, relativePath), "utf8"));
}

/// A semantic version, or nothing: a value this tool cannot compare is never
/// treated as one it can.
function parseComparableVersion(raw) {
  const value = String(raw ?? "").trim().replace(/^[vV]/u, "");
  const match = /^(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?$/u.exec(value);
  if (!match) return null;
  return {
    core: [Number(match[1]), Number(match[2]), Number(match[3])],
    prerelease: match[4] ?? "",
  };
}

function compareCore(left, right) {
  for (let index = 0; index < 3; index += 1) {
    if (left.core[index] !== right.core[index]) {
      return left.core[index] < right.core[index] ? -1 : 1;
    }
  }
  if (left.prerelease === right.prerelease) return 0;
  // A release outranks its own pre-releases; two different pre-release
  // identifiers are not ordered here, because semver orders them field by field
  // and this tool does not guess.
  if (left.prerelease === "") return 1;
  if (right.prerelease === "") return -1;
  return 0;
}

/// Whether both values carry a pre-release this comparison cannot order.
function ambiguous(left, right) {
  return left.prerelease !== "" && right.prerelease !== ""
    && compareCore({ ...left, prerelease: "" }, { ...right, prerelease: "" }) === 0;
}

function declaredBoundText(declared) {
  if (declared?.kind !== "bounded") return null;
  return `${declared.from}..<${declared.before}`;
}

/// The measured verdict for one Agent version: what the probe reported against
/// what the manifest declares.
///
/// Exactly one of supported, unsupported or unknown, with the reason that
/// produced it. No observation, no declared bound, and an uncomparable value all
/// answer unknown rather than assuming support.
function measureVersionSupport(declared, observed) {
  const declaredBound = declaredBoundText(declared);
  const declaredSource = typeof declared?.source === "string" ? declared.source : null;
  const observedVersion = typeof observed === "string" && observed.trim() !== ""
    ? observed.trim()
    : null;
  const verdict = (status, reasonCode) => Object.freeze({
    status,
    reasonCode,
    observedVersion,
    declaredBound,
    declaredSource,
  });
  if (!observedVersion) {
    return verdict("unknown", "agent_version_not_observed");
  }
  if (declared?.kind !== "bounded") {
    return verdict("unknown", "agent_version_no_declared_bound");
  }
  const from = parseComparableVersion(declared.from);
  const before = parseComparableVersion(declared.before);
  const seen = parseComparableVersion(observedVersion);
  if (!from || !before || !seen || ambiguous(from, seen) || ambiguous(before, seen)) {
    return verdict("unknown", "agent_version_not_comparable");
  }
  const atOrAfterStart = compareCore(seen, from) >= 0;
  const beforeEnd = compareCore(seen, before) < 0;
  return atOrAfterStart && beforeEnd
    ? verdict("supported", null)
    : verdict("unsupported", "agent_version_unsupported");
}

/// The recorded probe results one run compares against the declared intervals.
function readObservedVersions(options, knownAgentIds) {
  if (!options.observedVersionPath) return new Map();
  const document = JSON.parse(readFileSync(options.observedVersionPath, "utf8"));
  if (document?.schemaVersion !== observedVersionsSchemaVersion
    || typeof document?.observedVersions !== "object"
    || document.observedVersions === null
    || Array.isArray(document.observedVersions)) {
    throw new Error("upstream agent observed versions document is invalid");
  }
  const observed = new Map();
  for (const [agentId, version] of Object.entries(document.observedVersions)) {
    if (!knownAgentIds.has(agentId)) {
      throw new Error("upstream agent observed versions name an unknown agent");
    }
    if (typeof version !== "string" || version.trim() === "") {
      throw new Error("upstream agent observed version is invalid");
    }
    observed.set(agentId, version.trim());
  }
  return observed;
}

function manifestFor(agentId) {
  const candidate = path.resolve(manifestDirectory, `${agentId}.json`);
  try {
    return JSON.parse(readFileSync(candidate, "utf8"));
  } catch {
    return null;
  }
}

async function observeAgent({ entry, driver, evidenceRows, manifest, observedVersions }) {
  const config = agentConfigs[entry.id];
  const measured = measureVersionSupport(
    manifest?.officialCapabilityAssessment?.versionSupport,
    observedVersions.get(entry.id),
  );
  const row = {
    index: 0,
    agentId: entry.id,
    status: "unverified",
    reasonCode: null,
    install: {
      executablePresent: Boolean(config && resolveExecutable("", config)),
      source: config ? "configured" : "unavailable",
      registryProbeEligible: false,
      registryProbeReason: null,
    },
    version: {
      status: measured.status,
      reasonCode: safeReason(measured.reasonCode, null),
      observedVersion: measured.observedVersion,
      declaredBound: measured.declaredBound,
      declaredSource: measured.declaredSource,
      assessedVersionDeclared: Boolean(manifest?.officialCapabilityAssessment?.assessedVersion),
      versionProbeDeclared: Boolean(manifest?.officialCapabilityAssessment?.versionProbe),
      evidenceRowBound: evidenceRows.has(entry.id),
      harnessCurrent: evidenceRows.get(entry.id)?.harnessVersion === dispatchLaneHarnessVersion,
    },
    capability: {
      manifestPresent: Boolean(manifest),
      manifestDriverBound: Boolean(
        manifest && driver && manifest.identity?.driverId === driver.driverId,
      ),
      driverMode: driver?.driverMode || null,
      laneFamily: driver?.capabilityMatrix?.laneFamily || null,
      officialLane: driver?.capabilityMatrix?.officialLane === true,
    },
    handshake: {
      observed: false,
      laneFamilyMatched: null,
      runtimeProtocolMatched: null,
    },
  };

  const entryProbe = await entry.probe();
  row.install.registryProbeEligible = entryProbe.eligible === true;
  row.install.registryProbeReason = entryProbe.eligible ? null : safeReason(entryProbe.reason, null);

  // Subset of the inventory that the sidecar should report for this agent.
  const sidecar = resolveSidecar("", {});
  if (sidecar) {
    const capabilities = runDispatchLaneCli(sidecar, "capabilities", { agent: entry.id });
    const observed = capabilities?.ok === true && typeof capabilities?.laneFamily === "string";
    row.handshake.observed = observed;
    if (observed) {
      row.handshake.laneFamilyMatched = capabilities.laneFamily === driver?.capabilityMatrix?.laneFamily;
      row.handshake.runtimeProtocolMatched =
        typeof capabilities?.runtimeProtocol === "string" &&
        capabilities.runtimeProtocol === driver?.runtimeProtocol;
    }
  }

  const missingManifest = !manifest || !driver || !row.capability.manifestDriverBound;
  if (missingManifest) {
    row.status = "breakage";
    row.reasonCode = "adapter_manifest_missing";
  } else if (!row.install.executablePresent) {
    row.status = "unverified";
    row.reasonCode = "agent_binary_unavailable";
  } else if (!sidecar) {
    row.status = "unverified";
    row.reasonCode = "lico_client_executable_unavailable";
  } else if (!row.handshake.observed) {
    row.status = "breakage";
    row.reasonCode = "capabilities_handshake_failed";
  } else if (row.handshake.laneFamilyMatched !== true) {
    row.status = "breakage";
    row.reasonCode = "dispatch_lane_family_drift";
  } else if (row.handshake.runtimeProtocolMatched !== true) {
    row.status = "breakage";
    row.reasonCode = "runtime_protocol_drift";
  } else if (row.version.status === "unsupported") {
    // A version the manifest declares outside its own interval cannot be driven:
    // that is breakage, and it is measured rather than declared.
    row.status = "breakage";
    row.reasonCode = "agent_version_unsupported";
  } else if (!row.version.evidenceRowBound) {
    row.status = "unverified";
    row.reasonCode = "upstream_evidence_missing";
  } else {
    row.status = "verified";
    row.reasonCode = null;
  }
  return row;
}

export function createUpstreamAgentProbeReport(rows, summary) {
  return Object.freeze({
    schemaVersion: "licoup.upstream-agent-probe.v1",
    generatedAt: new Date().toISOString(),
    harnessClass: dispatchLaneHarnessVersion,
    agentCount: rows.length,
    status: summary.breakage > 0 || summary.versionUnsupported > 0
      ? "breakage"
      : summary.verified > 0 ? "verified" : "unverified",
    summary,
    agents: Object.freeze(rows.map((row) => Object.freeze({
      index: row.index,
      agentId: row.agentId,
      status: row.status,
      reasonCode: row.reasonCode,
      install: Object.freeze({ ...row.install }),
      version: Object.freeze({ ...row.version }),
      capability: Object.freeze({ ...row.capability }),
      handshake: Object.freeze({ ...row.handshake }),
    }))),
    breakage: Object.freeze(rows
      .filter((row) => row.status === "breakage")
      .map((row) => Object.freeze({ index: row.index, agentId: row.agentId, reasonCode: row.reasonCode }))),
  });
}

/// The measurement contract, checked without a machine, an Agent or a network.
///
/// Each case states what a run must answer, so a reviewer can falsify the
/// measurement by changing it and watching this report turn red.
const versionMeasurementCases = Object.freeze([
  {
    label: "observed version inside the declared interval",
    declared: { kind: "bounded", source: "vendor-documented", from: "1.2.0", before: "2.0.0" },
    observed: "1.9.9",
    status: "supported",
    reasonCode: null,
  },
  {
    label: "observed version below the declared interval",
    declared: { kind: "bounded", source: "vendor-documented", from: "1.2.0", before: "2.0.0" },
    observed: "1.1.9",
    status: "unsupported",
    reasonCode: "agent_version_unsupported",
  },
  {
    label: "observed version at the exclusive end of the interval",
    declared: { kind: "bounded", source: "vendor-documented", from: "1.2.0", before: "2.0.0" },
    observed: "2.0.0",
    status: "unsupported",
    reasonCode: "agent_version_unsupported",
  },
  {
    label: "observed pre-release below the release it precedes",
    declared: { kind: "bounded", source: "vendor-documented", from: "1.2.0", before: "2.0.0" },
    observed: "1.2.0-rc.1",
    status: "unsupported",
    reasonCode: "agent_version_unsupported",
  },
  {
    label: "no recorded observation",
    declared: { kind: "bounded", source: "vendor-documented", from: "1.2.0", before: "2.0.0" },
    observed: null,
    status: "unknown",
    reasonCode: "agent_version_not_observed",
  },
  {
    label: "a manifest that declares no bound",
    declared: { kind: "unknown", source: "unverified", reason: "no_verified_vendor_bound" },
    observed: "1.9.9",
    status: "unknown",
    reasonCode: "agent_version_no_declared_bound",
  },
  {
    label: "a reported version that is not a version",
    declared: { kind: "bounded", source: "vendor-documented", from: "1.2.0", before: "2.0.0" },
    observed: "latest",
    status: "unknown",
    reasonCode: "agent_version_not_comparable",
  },
]);

function selfTestVersionMeasurement(output) {
  const failures = [];
  for (const testCase of versionMeasurementCases) {
    const measured = measureVersionSupport(testCase.declared, testCase.observed);
    if (measured.status !== testCase.status || measured.reasonCode !== testCase.reasonCode) {
      failures.push({
        label: testCase.label,
        expected: `${testCase.status}:${testCase.reasonCode}`,
        actual: `${measured.status}:${measured.reasonCode}`,
      });
    }
    if (!VERSION_SUPPORT_STATUSES.includes(measured.status)) {
      failures.push({ label: testCase.label, expected: "a published status", actual: measured.status });
    }
  }
  output.write(`${JSON.stringify({
    schemaVersion: "licoup.upstream-agent-probe-self-test.v1",
    status: failures.length === 0 ? "ok" : "failed",
    cases: versionMeasurementCases.length,
    failures,
  })}\n`);
  return failures.length === 0 ? 0 : 1;
}

async function main(argv = process.argv.slice(2), output = process.stdout, error = process.stderr) {
  try {
    validateClientRegressionEntries();
    const options = parseArgs(argv);
    if (options.selfTest) {
      return selfTestVersionMeasurement(output);
    }
    const inventory = readJson("crates/licoup-native/resources/agent-conversation-drivers.json");
    const evidence = readJson("crates/licoup-native/resources/agent-conversation-evidence.json");
    const driverByAgent = new Map(inventory.drivers.map((driver) => [driver.agentId, driver]));
    const evidenceRows = new Map((evidence.adapters || []).map((row) => [row.agentId, row]));
    const observedVersions = readObservedVersions(
      options,
      new Set(AGENT_REGRESSION_ENTRIES.map((entry) => entry.id)),
    );
    const rows = [];
    for (const [index, entry] of AGENT_REGRESSION_ENTRIES.entries()) {
      rows.push(await observeAgent({
        entry,
        driver: driverByAgent.get(entry.id),
        evidenceRows,
        manifest: manifestFor(entry.id),
        observedVersions,
      }));
    }
    rows.forEach((row, index) => { row.index = index; });
    if (rows.length !== 13) throw new Error("upstream agent probe inventory is incomplete");
    const summary = {
      verified: rows.filter((row) => row.status === "verified").length,
      breakage: rows.filter((row) => row.status === "breakage").length,
      unverified: rows.filter((row) => row.status === "unverified").length,
      versionSupported: rows.filter((row) => row.version.status === "supported").length,
      versionUnsupported: rows.filter((row) => row.version.status === "unsupported").length,
      versionUnknown: rows.filter((row) => row.version.status === "unknown").length,
    };
    const report = createUpstreamAgentProbeReport(rows, summary);
    mkdirSync(path.dirname(options.reportPath), { recursive: true });
    writeFileSync(options.reportPath, `${JSON.stringify(report, null, 2)}\n`, { mode: 0o600 });
    output.write(`${JSON.stringify(report)}\n`);
    return summary.breakage > 0 || summary.versionUnsupported > 0 ? 1 : 0;
  } catch (failure) {
    error.write(`${JSON.stringify({
      schemaVersion: "licoup.upstream-agent-probe.v1",
      status: "failed",
      reasonCode: /^[a-z0-9_.:+-]{1,96}$/u.test(failure?.message || "")
        ? failure.message
        : "upstream_probe_failed",
    })}\n`);
    return 2;
  }
}

const invoked = process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (invoked) process.exitCode = await main();
