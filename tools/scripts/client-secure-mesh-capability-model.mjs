#!/usr/bin/env node
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import process from "node:process";
import {
  capabilityCatalogPath,
  loadCapabilityCatalog,
  reduceCapabilityFacts,
  validateCapabilityCatalogText,
  validateCapabilityReport
} from "./lib/secure-mesh-capability-report.mjs";

const generatedDartCatalogUrl = new URL(
  "../../apps/desktop/lib/src/contracts/generated/secure_mesh_capability_catalog.g.dart",
  import.meta.url
);

function requireValue(condition, message) {
  if (!condition) throw new Error(message);
}

function supportedFacts(catalog, predicate = () => true) {
  return catalog.order
    .map((id) => catalog.byId.get(id))
    .filter((definition) => !definition.derived && predicate(definition))
    .map((definition) => ({ capability: definition.id, state: "supported" }));
}

function expectRejected(action, label) {
  let rejected = false;
  try {
    action();
  } catch {
    rejected = true;
  }
  requireValue(rejected, label);
}

function generatedDartCatalogParts(source) {
  const digest = source.match(
    /const String secureMeshCapabilityCatalogDigest\s*=\s*\n?\s*'([a-f0-9]{64})';/u
  )?.[1];
  const catalogSource = source.match(
    /const String secureMeshCapabilityCatalogSource\s*=\s*r'''([\s\S]*?)''';/u
  )?.[1];
  requireValue(Boolean(digest), "generated Dart capability catalog digest is missing");
  requireValue(catalogSource !== undefined,
    "generated Dart capability catalog source is missing");
  return { digest, catalogSource };
}

function validateGeneratedDartCatalog(
  generatedSource = readFileSync(generatedDartCatalogUrl, "utf8"),
  canonicalSource = readFileSync(capabilityCatalogPath, "utf8")
) {
  const generated = generatedDartCatalogParts(generatedSource);
  const canonicalDigest = createHash("sha256").update(canonicalSource).digest("hex");
  requireValue(generated.digest === canonicalDigest,
    "generated Dart capability catalog digest differs from canonical source");
  requireValue(generated.catalogSource === canonicalSource,
    "generated Dart capability catalog text differs from canonical source");
  return { digest: canonicalDigest, source: canonicalSource };
}

function runSelfTest() {
  const catalog = loadCapabilityCatalog();
  const generatedCatalog = validateGeneratedDartCatalog();
  requireValue(generatedCatalog.digest === catalog.digest,
    "generated Dart capability catalog is not bound to the runtime catalog");
  requireValue(catalog.order.length > 0 && catalog.edgeCount > 0, "catalog graph is empty");

  const complete = reduceCapabilityFacts(supportedFacts(catalog), catalog);
  const completeResult = validateCapabilityReport(complete, catalog);
  requireValue(completeResult.mandatoryFoundationComplete === true,
    "complete capability fixture did not close the mandatory foundation");

  const protocolFacts = supportedFacts(catalog, (definition) =>
    definition.scope === "protocol_session" && definition.mandatory);
  const memoryOnly = reduceCapabilityFacts(protocolFacts, catalog);
  const memoryResult = validateCapabilityReport(memoryOnly, catalog);
  requireValue(memoryResult.custodyStrategy === "memory_only_ephemeral",
    "memory-only fixture selected the wrong custody strategy");
  requireValue(memoryOnly.custody.restartSemantics === "re_pair_rekey_after_restart",
    "memory-only fixture omitted restart re-pair/rekey semantics");

  const softwareOs = reduceCapabilityFacts([
    ...protocolFacts,
    { capability: "custody.os_secure_store", state: "supported" },
    { capability: "custody.software_backed", state: "supported" }
  ], catalog);
  const softwareResult = validateCapabilityReport(softwareOs, catalog);
  requireValue(softwareResult.custodyStrategy === "os_secure_store",
    "software OS store fixture was rejected");
  requireValue(!softwareOs.enabled.includes("custody.hardware_backed"),
    "software OS store fixture falsely enabled hardware backing");

  for (const mandatory of catalog.order.filter((id) => catalog.byId.get(id).mandatory &&
    !catalog.byId.get(id).derived)) {
    const missing = reduceCapabilityFacts(protocolFacts.filter((fact) => fact.capability !== mandatory), catalog);
    validateCapabilityReport(missing, catalog);
    requireValue(missing.mandatoryFoundationComplete === false &&
      missing.missingMandatory.includes(mandatory),
    "missing mandatory capability was not rejected");
  }

  const higherFacts = [
    ...protocolFacts,
    { capability: "custody.os_secure_store", state: "supported" },
    { capability: "custody.non_exportable", state: "supported" },
    { capability: "custody.device_bound", state: "supported" },
    { capability: "custody.hardware_backed", state: "supported" },
    { capability: "custody.tee", state: "supported" }
  ];
  const lower = reduceCapabilityFacts(protocolFacts, catalog);
  const higher = reduceCapabilityFacts(higherFacts, catalog);
  requireValue(lower.enabled.every((id) => higher.enabled.includes(id)),
    "capability closure is not monotonic");
  requireValue(higher.enabled.includes("custody.tee"),
    "supported dependency-closed capability did not auto-enable");

  const forgedDependency = structuredClone(higher);
  forgedDependency.enabled = forgedDependency.enabled.filter((id) => id !== "custody.device_bound");
  expectRejected(() => validateCapabilityReport(forgedDependency, catalog),
    "dependency-invalid capability report was accepted");

  const fixedGrade = structuredClone(memoryOnly);
  fixedGrade.level = 3;
  expectRejected(() => validateCapabilityReport(fixedGrade, catalog),
    "retired scalar posture field was accepted");

  const staleCatalog = structuredClone(memoryOnly);
  staleCatalog.catalogDigest = "0".repeat(64);
  expectRejected(() => validateCapabilityReport(staleCatalog, catalog),
    "stale catalog digest was accepted");

  const unsafeStrategy = structuredClone(memoryOnly);
  unsafeStrategy.custody.strategy = "portable_file";
  expectRejected(() => validateCapabilityReport(unsafeStrategy, catalog),
    "unsafe persistence strategy was accepted");

  const unknownCatalogField = JSON.parse(readFileSync(capabilityCatalogPath, "utf8"));
  unknownCatalogField.unknown = true;
  expectRejected(() => validateCapabilityCatalogText(JSON.stringify(unknownCatalogField)),
    "unknown catalog field was accepted");

  const generatedSource = readFileSync(generatedDartCatalogUrl, "utf8");
  const generatedParts = generatedDartCatalogParts(generatedSource);
  expectRejected(
    () => validateGeneratedDartCatalog(
      generatedSource.replace(generatedParts.digest, "0".repeat(64))
    ),
    "stale generated Dart capability digest was accepted"
  );
  expectRejected(
    () => validateGeneratedDartCatalog(
      generatedSource.replace('"schemaVersion": 1', '"schemaVersion": 2')
    ),
    "divergent generated Dart capability source was accepted"
  );

  return {
    ok: true,
    mode: "self-test",
    catalogDigest: catalog.digest,
    capabilityCount: catalog.order.length,
    edgeCount: catalog.edgeCount,
    fixtureCount: 11
  };
}

function runStaticGate() {
  const catalog = loadCapabilityCatalog();
  const generatedCatalog = validateGeneratedDartCatalog();
  requireValue(generatedCatalog.digest === catalog.digest,
    "generated Dart capability catalog is not bound to the runtime catalog");
  const source = readFileSync(capabilityCatalogPath, "utf8");
  const reparsed = validateCapabilityCatalogText(source);
  requireValue(reparsed.digest === catalog.digest, "catalog digest is not deterministic");
  const report = reduceCapabilityFacts(supportedFacts(catalog), catalog);
  validateCapabilityReport(report, catalog);
  const serialized = JSON.stringify(report);
  for (const forbidden of [
    "\"tier\"",
    "\"level\"",
    "\"strategy\":\"plaintext\"",
    "\"strategy\":\"ordinary_file\"",
    "\"strategy\":\"portable_file\""
  ]) {
    requireValue(!serialized.includes(forbidden), `capability model contains forbidden authority: ${forbidden}`);
  }
  return {
    ok: true,
    mode: "static-gate",
    catalogDigest: catalog.digest,
    capabilityCount: catalog.order.length,
    edgeCount: catalog.edgeCount
  };
}

async function validateReportFromStdin() {
  const chunks = [];
  for await (const chunk of process.stdin) chunks.push(chunk);
  const input = JSON.parse(Buffer.concat(chunks).toString("utf8"));
  const report = input?.capabilityReport || input;
  const result = validateCapabilityReport(report, loadCapabilityCatalog());
  return {
    ok: true,
    mode: "native-report",
    catalogDigest: result.catalogDigest,
    mandatoryFoundationComplete: result.mandatoryFoundationComplete,
    custodyStrategy: result.custodyStrategy,
    enabledCount: result.enabledCount
  };
}

const result = process.argv.includes("--self-test")
  ? runSelfTest()
  : process.argv.includes("--report-stdin")
    ? await validateReportFromStdin()
    : runStaticGate();
console.log(JSON.stringify(result));
