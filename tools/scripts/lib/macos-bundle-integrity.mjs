import { spawnSync } from "node:child_process";
import {
  existsSync,
  mkdtempSync,
  realpathSync,
  rmSync,
} from "node:fs";
import os from "node:os";
import path from "node:path";
import {
  artifactTreeSnapshot,
  CLIENT_RELEASE_ARTIFACT_TREE_LIMITS,
  sha256Buffer,
  stableReadFile,
} from "./client-release-artifact-digest.mjs";

export const MACOS_CODE_INSPECTION_LIMITS = Object.freeze({
  artifactTree: CLIENT_RELEASE_ARTIFACT_TREE_LIMITS,
  maxNestedCodePaths: 4096,
  maxDurationMs: 120_000,
});

function requireValue(condition, message) {
  if (!condition) throw new Error(message);
}

function canonicalJson(value) {
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(",")}]`;
  if (value && typeof value === "object") {
    return `{${Object.keys(value).sort()
      .map((key) => `${JSON.stringify(key)}:${canonicalJson(value[key])}`)
      .join(",")}}`;
  }
  return JSON.stringify(value);
}

function run(command, args, options = {}) {
  const { deadlineMs, ...spawnOptions } = options;
  const remainingMs = deadlineMs === undefined
    ? 30_000
    : Math.min(30_000, Math.floor(Number(deadlineMs) - Date.now()));
  requireValue(Number.isFinite(remainingMs) && remainingMs > 0,
    "macOS code signature inspection deadline exceeded");
  return spawnSync(command, args, {
    encoding: "utf8",
    stdio: "pipe",
    maxBuffer: 16 * 1024 * 1024,
    timeout: remainingMs,
    ...spawnOptions,
  });
}

function plistToCanonicalJson(plistBytes, { deadlineMs } = {}) {
  const converted = run("/usr/bin/plutil", ["-convert", "json", "-o", "-", "--", "-"], {
    input: plistBytes,
    deadlineMs,
  });
  requireValue(converted.status === 0, "macOS entitlements plist could not be normalized");
  const decoded = JSON.parse(String(converted.stdout || ""));
  return canonicalJson(decoded);
}

function signerCertificateFingerprint(codePath, { deadlineMs } = {}) {
  const temporaryRoot = mkdtempSync(path.join(os.tmpdir(), "licoup-codesign-certificate-"));
  const certificatePrefix = path.join(temporaryRoot, "certificate");
  try {
    const extracted = run(
      "/usr/bin/codesign",
      ["-d", `--extract-certificates=${certificatePrefix}`, codePath],
      { deadlineMs },
    );
    const leafCertificate = `${certificatePrefix}0`;
    requireValue(
      extracted.status === 0 && existsSync(leafCertificate),
      "macOS signer certificate could not be extracted",
    );
    return sha256Buffer(stableReadFile(leafCertificate, { maxBytes: 4 * 1024 * 1024 }));
  } finally {
    rmSync(temporaryRoot, { recursive: true, force: true });
  }
}

export function macosSignatureEvidenceFromText(
  detailText,
  requirementText,
  requirementStatus = 0,
) {
  const timestamp = /(?:^|\n)Timestamp=([^\r\n]+)/u.exec(detailText)?.[1]?.trim() || "";
  const teamIdentifier = /(?:^|\n)TeamIdentifier=([A-Z0-9]{10})(?:\r?$)/mu
    .exec(detailText)?.[1] || "";
  return Object.freeze({
    detailText,
    secureTimestamp: timestamp !== "" && timestamp.toLowerCase() !== "none",
    teamIdentifier,
    developerIdApplication: requirementStatus === 0 &&
      /\banchor apple generic\b/u.test(requirementText) &&
      /certificate 1\[field\.1\.2\.840\.113635\.100\.6\.2\.6\]/u.test(requirementText) &&
      /certificate leaf\[field\.1\.2\.840\.113635\.100\.6\.1\.13\]/u.test(requirementText),
  });
}

function parsedSignatureEvidence(codePath, details, { deadlineMs } = {}) {
  const requirements = run(
    "/usr/bin/codesign",
    ["-d", "-r-", codePath],
    { deadlineMs },
  );
  const detailText = `${String(details.stdout || "")}\n${String(details.stderr || "")}`;
  const requirementText = `${String(requirements.stdout || "")}\n${String(requirements.stderr || "")}`;
  return macosSignatureEvidenceFromText(
    detailText,
    requirementText,
    requirements.status,
  );
}

export function macosEntitlementsInspection({
  expected,
  actualCanonical,
  expectedCanonical,
  raw,
  parsed,
  status,
}) {
  const entitlementsMatch = expected === true && actualCanonical !== "" &&
    actualCanonical === expectedCanonical;
  const inspectionCompleted = Number.isInteger(status) && [0, 1].includes(status);
  const entitlementsEmpty = expected !== true &&
    inspectionCompleted &&
    (String(raw || "").trim() === "" || (parsed === true && actualCanonical === "{}"));
  return Object.freeze({
    entitlementsMatch,
    entitlementsEmpty,
    ready: expected === true
      ? status === 0 && parsed === true
      : entitlementsEmpty,
  });
}

export function inspectMacosContainerSignature(codePath, { deadlineMs } = {}) {
  const verification = run(
    "/usr/bin/codesign",
    ["--verify", "--strict", codePath],
    { deadlineMs },
  );
  const details = run(
    "/usr/bin/codesign",
    ["-dv", "--verbose=4", codePath],
    { deadlineMs },
  );
  const evidence = parsedSignatureEvidence(codePath, details, { deadlineMs });
  const { detailText } = evidence;
  const signatureKind = detailText.includes("Signature=adhoc")
    ? "local-ad-hoc-codesign"
    : /(?:^|\n)Authority=/u.test(detailText)
      ? "local-identity-codesign"
      : "unknown";
  let signerFingerprint = "";
  if (signatureKind === "local-identity-codesign" && details.status === 0) {
    try {
      signerFingerprint = signerCertificateFingerprint(codePath, { deadlineMs });
    } catch {
      signerFingerprint = "";
    }
  }
  return Object.freeze({
    verified: verification.status === 0 && details.status === 0 &&
      signatureKind === "local-identity-codesign" && signerFingerprint !== "",
    signatureKind,
    signerFingerprint,
    developerIdApplication: evidence.developerIdApplication,
    secureTimestamp: evidence.secureTimestamp,
    teamIdentifier: evidence.teamIdentifier,
  });
}

export function normalizedMacosEntitlementsFile(entitlementsPath, options = {}) {
  return plistToCanonicalJson(stableReadFile(entitlementsPath, {
    maxBytes: 1024 * 1024,
  }), options);
}

export function inspectMacosCodeSignature(
  appPath,
  expectedEntitlementsPath,
  { deadlineMs } = {},
) {
  const verification = run(
    "/usr/bin/codesign",
    ["--verify", "--deep", "--strict", appPath],
    { deadlineMs },
  );
  const details = run(
    "/usr/bin/codesign",
    ["-dv", "--verbose=4", appPath],
    { deadlineMs },
  );
  const evidence = parsedSignatureEvidence(appPath, details, { deadlineMs });
  const entitlementResult = run(
    "/usr/bin/codesign",
    ["-d", "--entitlements", ":-", appPath],
    { deadlineMs },
  );
  const { detailText } = evidence;
  const signatureKind = detailText.includes("Signature=adhoc")
    ? "local-ad-hoc-codesign"
    : /(?:^|\n)Authority=/u.test(detailText)
      ? "local-identity-codesign"
      : "unknown";
  const hardenedRuntime = /\bflags=0x[0-9a-f]+\([^\n)]*runtime[^\n)]*\)/iu.test(
    detailText,
  );
  let signerFingerprint = "";
  if (signatureKind === "local-identity-codesign" && details.status === 0) {
    try {
      signerFingerprint = signerCertificateFingerprint(appPath, { deadlineMs });
    } catch {
      signerFingerprint = "";
    }
  }
  let actualEntitlements = "";
  let expectedEntitlements = "";
  let entitlementsParsed = false;
  try {
    const rawEntitlements = String(entitlementResult.stdout || "");
    if (rawEntitlements.trim() !== "") {
      actualEntitlements = plistToCanonicalJson(
        Buffer.from(rawEntitlements, "utf8"),
        { deadlineMs },
      );
      entitlementsParsed = true;
    }
    expectedEntitlements = expectedEntitlementsPath
      ? normalizedMacosEntitlementsFile(expectedEntitlementsPath, { deadlineMs })
      : "";
  } catch {
    actualEntitlements = "";
    expectedEntitlements = "";
  }
  const entitlementInspection = macosEntitlementsInspection({
    expected: Boolean(expectedEntitlementsPath),
    actualCanonical: actualEntitlements,
    expectedCanonical: expectedEntitlements,
    raw: entitlementResult.stdout,
    parsed: entitlementsParsed,
    status: entitlementResult.status,
  });
  return Object.freeze({
    verified: verification.status === 0 && details.status === 0 &&
      entitlementInspection.ready &&
      (signatureKind !== "local-identity-codesign" || signerFingerprint !== ""),
    signatureKind,
    signerFingerprint,
    hardenedRuntime,
    developerIdApplication: evidence.developerIdApplication,
    secureTimestamp: evidence.secureTimestamp,
    teamIdentifier: evidence.teamIdentifier,
    entitlementsMatch: entitlementInspection.entitlementsMatch,
    entitlementsEmpty: entitlementInspection.entitlementsEmpty,
    entitlementsDigest: entitlementInspection.entitlementsMatch
      ? sha256Buffer(Buffer.from(expectedEntitlements, "utf8"))
      : "",
  });
}

export function listMacosNestedCodePaths(appPath, mainExecutableName, {
  snapshot,
  limits = MACOS_CODE_INSPECTION_LIMITS.artifactTree,
  deadlineMs = Date.now() + MACOS_CODE_INSPECTION_LIMITS.maxDurationMs,
} = {}) {
  const candidates = [];
  const signableBundleSuffixes = [
    ".app",
    ".appex",
    ".bundle",
    ".framework",
    ".plugin",
    ".xpc",
  ];
  requireValue(Date.now() <= deadlineMs,
    "macOS code signature inspection deadline exceeded");
  const tree = snapshot || artifactTreeSnapshot(appPath, { limits, deadlineMs });
  const root = realpathSync(appPath);
  requireValue(tree?.root === root && Array.isArray(tree?.entries),
    "macOS code inventory does not match its app bundle");
  for (const entry of tree.entries) {
    if (entry.kind === "symlink" || !entry.path.startsWith("Contents/")) continue;
    const name = path.posix.basename(entry.path);
    if (entry.kind === "directory") {
      if (signableBundleSuffixes.some((suffix) => name.endsWith(suffix))) {
        candidates.push(path.join(root, ...entry.path.split("/")));
      }
      continue;
    }
    if (entry.kind !== "file") continue;
    const isMainExecutable = path.posix.dirname(entry.path) === "Contents/MacOS" &&
      name === mainExecutableName;
    const isNestedExecutable = !isMainExecutable &&
      (Number.parseInt(entry.mode, 8) & 0o111) !== 0;
    const isDynamicLibrary = name.endsWith(".dylib");
    if (isNestedExecutable || isDynamicLibrary) {
      candidates.push(path.join(root, ...entry.path.split("/")));
    }
  }
  return [...new Set(candidates)].sort((left, right) =>
    right.split(path.sep).length - left.split(path.sep).length || left.localeCompare(right));
}

export function inspectBoundedMacosCodePolicy(
  appPath,
  mainExecutableName,
  expectedEntitlementsPath,
  {
    limits = MACOS_CODE_INSPECTION_LIMITS.artifactTree,
    maxNestedCodePaths = MACOS_CODE_INSPECTION_LIMITS.maxNestedCodePaths,
    deadlineMs = Date.now() + MACOS_CODE_INSPECTION_LIMITS.maxDurationMs,
    inspectSignature = inspectMacosCodeSignature,
  } = {},
) {
  requireValue(Number.isSafeInteger(maxNestedCodePaths) && maxNestedCodePaths > 0,
    "macOS nested code path limit is invalid");
  const before = artifactTreeSnapshot(appPath, { limits, deadlineMs });
  const nestedCodePaths = listMacosNestedCodePaths(appPath, mainExecutableName, {
    snapshot: before,
    limits,
    deadlineMs,
  });
  requireValue(nestedCodePaths.length <= maxNestedCodePaths,
    "macOS app bundle exceeds its nested code path bound");
  requireValue(Date.now() <= deadlineMs,
    "macOS code signature inspection deadline exceeded");
  const signature = inspectSignature(appPath, expectedEntitlementsPath, { deadlineMs });
  const nestedSignatures = nestedCodePaths.map((nestedPath) => {
    requireValue(Date.now() <= deadlineMs,
      "macOS code signature inspection deadline exceeded");
    return {
      path: nestedPath,
      signature: inspectSignature(nestedPath, undefined, { deadlineMs }),
    };
  });
  const signerIdentityUniform = signature.signatureKind === "local-identity-codesign" &&
    /^sha256:[a-f0-9]{64}$/u.test(signature.signerFingerprint) &&
    nestedSignatures.every(({ signature: nestedSignature }) =>
      nestedSignature.signatureKind === "local-identity-codesign" &&
      nestedSignature.signerFingerprint === signature.signerFingerprint);
  if (signature.signatureKind === "local-identity-codesign") {
    requireValue(
      signerIdentityUniform,
      "macOS nested code signing identity does not match the app identity",
    );
  }
  const after = artifactTreeSnapshot(appPath, { limits, deadlineMs });
  requireValue(before.digest === after.digest,
    "macOS app bundle changed during code signature inspection");
  return Object.freeze({
    artifactDigest: before.digest,
    signature,
    signerIdentityUniform,
    nestedCodePaths: Object.freeze([...nestedCodePaths]),
    nestedSignatures: Object.freeze(nestedSignatures.map((entry) => Object.freeze(entry))),
    treeMetrics: before.metrics,
    deadlineMs,
  });
}
