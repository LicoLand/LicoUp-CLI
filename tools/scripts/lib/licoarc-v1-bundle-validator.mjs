import { createHash } from "node:crypto";
import { isDeepStrictEqual } from "node:util";

const ARTIFACT_VERSION = "licoarc.bundle.v1";
const ARTIFACT_PATH = "artifacts/v1/licoarc.bundle.json";
const MANIFEST_VERSION = "licoarc.source-manifest.v1";
const MANIFEST_PATH = "spec/v1/manifest.json";
const SCHEMA_PATH = "spec/v1/relay/envelope.schema.json";
const RESULT_SCHEMA_PATH =
  "spec/v1/relay/conformance-result.schema.json";
const POLICY_PATH = "spec/v1/relay/governance.policy.json";
const CORPUS_MANIFEST_PATH = "conformance/v1/relay/manifest.json";
const INVALID_CASES_PATH = "conformance/v1/relay/invalid.json";
const VALID_CASES_PATH = "conformance/v1/relay/valid.json";
const CANONICAL_ARTIFACT_DIGEST =
  "a6e961d96ed9ae143680f5834356ec92b9391059d9654c6b713511e1240df239";
const SOURCE_PATHS = Object.freeze([
  CORPUS_MANIFEST_PATH,
  INVALID_CASES_PATH,
  VALID_CASES_PATH,
  MANIFEST_PATH,
  RESULT_SCHEMA_PATH,
  SCHEMA_PATH,
  POLICY_PATH,
].sort());
const MANIFEST_SOURCE_PATHS = Object.freeze(
  SOURCE_PATHS.filter((sourcePath) => sourcePath !== MANIFEST_PATH),
);
const SUPPORTED_SCHEMA_KEYWORDS = Object.freeze(new Set([
  "$schema",
  "$id",
  "title",
  "description",
  "$comment",
  "type",
  "const",
  "enum",
  "oneOf",
  "required",
  "properties",
  "additionalProperties",
  "minLength",
  "maxLength",
  "pattern",
  "format",
]));

export function validateLicoArcV1BundleBytes(bytes) {
  requireValue(Buffer.isBuffer(bytes) && bytes.length > 0,
    "Lico Arc candidate bundle is empty");
  const source = bytes.toString("utf8");
  const bundle = JSON.parse(source);
  assertClosedObject(bundle, [
    "artifactVersion",
    "digestAlgorithm",
    "sources",
    "digest",
  ], "Lico Arc artifact");
  requireValue(bundle.artifactVersion === ARTIFACT_VERSION,
    "Lico Arc artifact version is unsupported");
  requireValue(bundle.digestAlgorithm === "sha256",
    "Lico Arc artifact digest algorithm is unsupported");
  requireValue(
    typeof bundle.digest === "string" &&
      /^[a-f0-9]{64}$/u.test(bundle.digest) &&
      bundle.digest === CANONICAL_ARTIFACT_DIGEST,
    "Lico Arc artifact digest is not the pinned v1 candidate",
  );
  assertClosedObject(bundle.sources, SOURCE_PATHS, "Lico Arc artifact sources");

  const canonicalSources = sortJsonValue(bundle.sources);
  const body = {
    artifactVersion: ARTIFACT_VERSION,
    digestAlgorithm: "sha256",
    sources: canonicalSources,
  };
  const canonicalBody = `${canonicalizeJson(body)}\n`;
  const recomputedDigest =
    createHash("sha256").update(canonicalBody).digest("hex");
  requireValue(recomputedDigest === bundle.digest,
    "Lico Arc artifact digest does not bind its sources");
  const canonicalArtifact =
    `${JSON.stringify({ ...body, digest: bundle.digest }, null, 2)}\n`;
  requireValue(source === canonicalArtifact,
    "Lico Arc artifact is not the canonical serialized candidate");

  validateSourceManifest(bundle.sources[MANIFEST_PATH]);
  const conformance = validateConformanceSources(bundle.sources);
  validateGovernancePolicy(bundle.sources[POLICY_PATH]);
  return Object.freeze({
    ready: true,
    sourceCount: SOURCE_PATHS.length,
    caseCount: conformance.caseCount,
    acceptedCaseCount: conformance.acceptedCaseCount,
    rejectedCaseCount: conformance.rejectedCaseCount,
  });
}

export function runLicoArcV1BundleValidatorSelfTest(canonicalBytes) {
  const canonical = validateLicoArcV1BundleBytes(canonicalBytes);
  const original = JSON.parse(canonicalBytes.toString("utf8"));
  const emptyRejected = rejectsBundle(Buffer.from("{}\n"));
  const unknownFieldRejected = rejectsBundle(serializeArtifact({
    ...original,
    unknown: false,
  }));

  const tampered = structuredClone(original);
  tampered.sources[POLICY_PATH].trustModel = "station-is-trusted";
  const tamperedSourceRejected = rejectsBundle(
    Buffer.from(`${JSON.stringify(tampered, null, 2)}\n`),
  );

  const stale = structuredClone(original);
  stale.sources[POLICY_PATH].implementationDefinedCapabilities =
    [...stale.sources[POLICY_PATH].implementationDefinedCapabilities, "stale"];
  const staleBody = {
    artifactVersion: stale.artifactVersion,
    digestAlgorithm: stale.digestAlgorithm,
    sources: sortJsonValue(stale.sources),
  };
  stale.digest = createHash("sha256")
    .update(`${canonicalizeJson(staleBody)}\n`)
    .digest("hex");
  const staleRecomputedBundleRejected = rejectsBundle(serializeArtifact(stale));

  return Object.freeze({
    ok: canonical.ready === true &&
      emptyRejected &&
      unknownFieldRejected &&
      tamperedSourceRejected &&
      staleRecomputedBundleRejected,
    canonicalBundleAccepted: canonical.ready === true,
    emptyBundleRejected: emptyRejected,
    unknownFieldRejected,
    tamperedSourceRejected,
    staleRecomputedBundleRejected,
  });
}

function validateSourceManifest(manifest) {
  assertClosedObject(manifest, [
    "manifestVersion",
    "artifactVersion",
    "artifactPath",
    "sources",
  ], "Lico Arc source manifest");
  requireValue(
    manifest.manifestVersion === MANIFEST_VERSION &&
      manifest.artifactVersion === ARTIFACT_VERSION &&
      manifest.artifactPath === ARTIFACT_PATH,
    "Lico Arc source manifest binding is invalid",
  );
  requireValue(
    Array.isArray(manifest.sources) &&
      arraysEqual(manifest.sources, MANIFEST_SOURCE_PATHS),
    "Lico Arc source manifest does not bind the closed v1 source set",
  );
}

function validateConformanceSources(sources) {
  const schema = sources[SCHEMA_PATH];
  const resultSchema = sources[RESULT_SCHEMA_PATH];
  const corpus = sources[CORPUS_MANIFEST_PATH];
  const validCases = sources[VALID_CASES_PATH];
  const invalidCases = sources[INVALID_CASES_PATH];
  assertSupportedSchema(schema);
  assertSupportedSchema(resultSchema);
  validatePinnedEnvelopeSchema(schema);
  validatePinnedResultSchema(resultSchema);

  assertClosedObject(corpus, [
    "corpusVersion",
    "contractVersion",
    "schemaPath",
    "resultSchemaPath",
    "formatAssertions",
    "caseFiles",
    "caseIds",
  ], "Lico Arc conformance manifest");
  requireValue(
    corpus.corpusVersion === "licoarc.relay-conformance-corpus.v1" &&
      corpus.contractVersion === "licoarc.relay.v1" &&
      corpus.schemaPath === SCHEMA_PATH &&
      corpus.resultSchemaPath === RESULT_SCHEMA_PATH &&
      arraysEqual(corpus.formatAssertions, ["date-time"]) &&
      arraysEqual(corpus.caseFiles, [INVALID_CASES_PATH, VALID_CASES_PATH]),
    "Lico Arc conformance manifest bindings are invalid",
  );
  requireValue(
    Array.isArray(validCases) && validCases.length > 0 &&
      Array.isArray(invalidCases) && invalidCases.length > 0,
    "Lico Arc conformance corpus is empty",
  );

  validateCaseList(validCases, "accept", schema, resultSchema);
  validateCaseList(invalidCases, "reject", schema, resultSchema);
  const allCases = [...validCases, ...invalidCases];
  const caseIds = allCases.map(({ id }) => id);
  requireValue(new Set(caseIds).size === caseIds.length,
    "Lico Arc conformance case identifiers are not unique");
  requireValue(
    Array.isArray(corpus.caseIds) &&
      arraysEqual(corpus.caseIds, [...corpus.caseIds].sort()) &&
      arraysEqual([...caseIds].sort(), corpus.caseIds),
    "Lico Arc conformance manifest case binding is incomplete",
  );
  return {
    caseCount: allCases.length,
    acceptedCaseCount: validCases.length,
    rejectedCaseCount: invalidCases.length,
  };
}

function validateCaseList(cases, expectedOutcome, schema, resultSchema) {
  const ids = cases.map(({ id }) => id);
  requireValue(arraysEqual(ids, [...ids].sort()),
    "Lico Arc conformance cases are not canonically ordered");
  for (const case_ of cases) {
    assertClosedObject(case_, ["id", "value", "expected"],
      "Lico Arc conformance case");
    requireValue(
      typeof case_.id === "string" &&
        /^relay-v1\.(?:accept|reject)\.[a-z0-9-]+$/u.test(case_.id),
      "Lico Arc conformance case identifier is invalid",
    );
    assertClosedObject(case_.expected, ["outcome", "rejectionClass"],
      "Lico Arc conformance expected result");
    requireValue(
      case_.expected.outcome === expectedOutcome &&
        validateWithSchema(case_.expected, resultSchema).length === 0,
      "Lico Arc conformance expected result is invalid",
    );
    const errors = validateWithSchema(case_.value, schema);
    const actual = errors.length === 0
      ? { outcome: "accept", rejectionClass: "none" }
      : { outcome: "reject", rejectionClass: classifyRejection(errors) };
    requireValue(isDeepStrictEqual(actual, case_.expected),
      "Lico Arc independent conformance evaluation disagrees with the corpus");
  }
}

function validatePinnedEnvelopeSchema(schema) {
  requireValue(
    schema?.$schema === "https://json-schema.org/draft/2020-12/schema" &&
      schema?.$id === "https://licoarc.com/spec/v1/relay/envelope.schema.json" &&
      schema?.type === "object" &&
      schema?.additionalProperties === false &&
      arraysEqual(schema?.required || [], [
        "contractVersion",
        "envelopeId",
        "mailboxId",
        "ciphertext",
        "expiresAt",
      ]) &&
      Object.keys(schema?.properties || {}).length === 5 &&
      schema?.properties?.contractVersion?.const === "licoarc.relay.v1" &&
      schema?.properties?.ciphertext?.minLength === 1 &&
      schema?.properties?.ciphertext?.maxLength === 1_048_576,
    "Lico Arc envelope schema is not the pinned closed v1 contract",
  );
}

function validatePinnedResultSchema(schema) {
  requireValue(
    schema?.$schema === "https://json-schema.org/draft/2020-12/schema" &&
      schema?.$id ===
        "https://licoarc.com/spec/v1/relay/conformance-result.schema.json" &&
      Array.isArray(schema?.oneOf) &&
      schema.oneOf.length === 2,
    "Lico Arc conformance result schema is not the pinned v1 contract",
  );
}

function validateGovernancePolicy(policy) {
  assertClosedObject(policy, [
    "adversarialStationSecurityPosture",
    "endpointOwnedSecurityDecisions",
    "firstReleaseAcceptanceScenario",
    "forbiddenCapabilities",
    "implementationDefinedCapabilities",
    "implementationRelationship",
    "nonAuthoritativeStationSignals",
    "policyVersion",
    "requiredCapabilities",
    "trustModel",
  ], "Lico Arc governance policy");
  assertClosedObject(policy.firstReleaseAcceptanceScenario, [
    "conformantEnvelopeDisposition",
    "nonConformantEnvelopeDisposition",
    "payloadProtectionOwner",
    "scenarioId",
    "stationPayloadVisibility",
    "stationReceiptAuthority",
  ], "Lico Arc first-release acceptance policy");
  assertClosedObject(policy.adversarialStationSecurityPosture, [
    "algorithmPolicy",
    "cryptographicAgility",
    "failureMode",
    "securityEvolution",
    "stationSecurityAuthority",
    "threatHorizon",
  ], "Lico Arc adversarial station policy");
  requireValue(
    policy.policyVersion === "licoarc.relay-governance.v1" &&
      policy.trustModel === "relay-is-untrusted" &&
      policy.firstReleaseAcceptanceScenario.scenarioId ===
        "two-endpoints-via-untrusted-station" &&
      policy.firstReleaseAcceptanceScenario.payloadProtectionOwner ===
        "endpoint" &&
      policy.firstReleaseAcceptanceScenario.stationPayloadVisibility ===
        "opaque-protected-payload-only" &&
      policy.firstReleaseAcceptanceScenario
        .nonConformantEnvelopeDisposition === "reject" &&
      policy.firstReleaseAcceptanceScenario.stationReceiptAuthority ===
        "transport-hint-only" &&
      policy.adversarialStationSecurityPosture.failureMode === "fail-closed" &&
      policy.adversarialStationSecurityPosture.stationSecurityAuthority ===
        "none" &&
      policy.forbiddenCapabilities.includes("plaintext-inspection") &&
      policy.forbiddenCapabilities.includes("client-key-custody") &&
      policy.requiredCapabilities.includes("closed-envelope-validation"),
    "Lico Arc governance policy does not preserve the v1 security boundary",
  );
}

function assertSupportedSchema(schema, schemaPath = "#") {
  if (typeof schema === "boolean") return;
  requireValue(isPlainObject(schema),
    `${schemaPath} must be a schema object or boolean`);
  for (const keyword of Object.keys(schema)) {
    requireValue(SUPPORTED_SCHEMA_KEYWORDS.has(keyword),
      `${schemaPath} uses an unsupported schema keyword`);
  }
  if (Object.hasOwn(schema, "type")) {
    requireValue([
      "null", "boolean", "object", "array", "number", "integer", "string",
    ].includes(schema.type), `${schemaPath} has an unsupported type`);
  }
  if (Object.hasOwn(schema, "enum")) {
    requireValue(Array.isArray(schema.enum) && schema.enum.length > 0,
      `${schemaPath} has an invalid enum`);
  }
  if (Object.hasOwn(schema, "required")) {
    requireValue(
      Array.isArray(schema.required) &&
        schema.required.every((property) => typeof property === "string") &&
        new Set(schema.required).size === schema.required.length,
      `${schemaPath} has invalid required properties`,
    );
  }
  if (Object.hasOwn(schema, "additionalProperties")) {
    requireValue(typeof schema.additionalProperties === "boolean",
      `${schemaPath} has unsupported additionalProperties`);
  }
  for (const keyword of ["minLength", "maxLength"]) {
    if (Object.hasOwn(schema, keyword)) {
      requireValue(Number.isInteger(schema[keyword]) && schema[keyword] >= 0,
        `${schemaPath} has an invalid length bound`);
    }
  }
  if (Object.hasOwn(schema, "minLength") &&
      Object.hasOwn(schema, "maxLength")) {
    requireValue(schema.minLength <= schema.maxLength,
      `${schemaPath} has inconsistent length bounds`);
  }
  if (Object.hasOwn(schema, "pattern")) {
    requireValue(typeof schema.pattern === "string",
      `${schemaPath} has an invalid pattern`);
    new RegExp(schema.pattern, "u");
  }
  if (Object.hasOwn(schema, "format")) {
    requireValue(schema.format === "date-time",
      `${schemaPath} has an unsupported format`);
  }
  if (Object.hasOwn(schema, "properties")) {
    requireValue(isPlainObject(schema.properties),
      `${schemaPath} has invalid properties`);
    for (const [propertyName, propertySchema] of
      Object.entries(schema.properties)) {
      assertSupportedSchema(
        propertySchema,
        `${schemaPath}/properties/${escapeJsonPointer(propertyName)}`,
      );
    }
  }
  if (Object.hasOwn(schema, "oneOf")) {
    requireValue(Array.isArray(schema.oneOf) && schema.oneOf.length > 0,
      `${schemaPath} has an invalid oneOf`);
    schema.oneOf.forEach((subschema, index) =>
      assertSupportedSchema(subschema, `${schemaPath}/oneOf/${index}`));
  }
}

function validateWithSchema(instance, schema, context = {
  instancePath: "",
  schemaPath: "#",
}) {
  if (schema === true) return [];
  if (schema === false) return [validationError("falseSchema", context)];
  const errors = [];
  if (Object.hasOwn(schema, "type") &&
      !matchesJsonType(instance, schema.type)) {
    errors.push(validationError("type", context));
  }
  if (Object.hasOwn(schema, "const") &&
      !isDeepStrictEqual(instance, schema.const)) {
    errors.push(validationError("const", context));
  }
  if (Object.hasOwn(schema, "enum") &&
      !schema.enum.some((allowed) => isDeepStrictEqual(instance, allowed))) {
    errors.push(validationError("enum", context));
  }
  if (Object.hasOwn(schema, "oneOf")) {
    const matches = schema.oneOf.filter((subschema, index) =>
      validateWithSchema(instance, subschema, {
        instancePath: context.instancePath,
        schemaPath: `${context.schemaPath}/oneOf/${index}`,
      }).length === 0);
    if (matches.length !== 1) errors.push(validationError("oneOf", context));
  }
  if (typeof instance === "string") {
    const length = [...instance].length;
    if (Object.hasOwn(schema, "minLength") && length < schema.minLength) {
      errors.push(validationError("minLength", context));
    }
    if (Object.hasOwn(schema, "maxLength") && length > schema.maxLength) {
      errors.push(validationError("maxLength", context));
    }
    if (Object.hasOwn(schema, "pattern") &&
        !new RegExp(schema.pattern, "u").test(instance)) {
      errors.push(validationError("pattern", context));
    }
    if (schema.format === "date-time" && !isRfc3339DateTime(instance)) {
      errors.push(validationError("format", context));
    }
  }
  if (isPlainObject(instance)) {
    if (Array.isArray(schema.required)) {
      for (const propertyName of schema.required) {
        if (!Object.hasOwn(instance, propertyName)) {
          errors.push({
            ...validationError("required", context),
            missingProperty: propertyName,
          });
        }
      }
    }
    if (isPlainObject(schema.properties)) {
      for (const [propertyName, propertySchema] of
        Object.entries(schema.properties)) {
        if (Object.hasOwn(instance, propertyName)) {
          errors.push(...validateWithSchema(
            instance[propertyName],
            propertySchema,
            {
              instancePath:
                `${context.instancePath}/${escapeJsonPointer(propertyName)}`,
              schemaPath:
                `${context.schemaPath}/properties/${escapeJsonPointer(propertyName)}`,
            },
          ));
        }
      }
      if (schema.additionalProperties === false) {
        for (const propertyName of Object.keys(instance)) {
          if (!Object.hasOwn(schema.properties, propertyName)) {
            errors.push({
              ...validationError("additionalProperties", context),
              additionalProperty: propertyName,
            });
          }
        }
      }
    }
  }
  return errors;
}

function classifyRejection(errors) {
  requireValue(errors.length > 0, "rejection requires validation errors");
  const classes = new Set(errors.map((error) => {
    if (error.instancePath === "/contractVersion" &&
        (error.keyword === "type" || error.keyword === "const")) {
      return "contract-identifier";
    }
    if (error.keyword === "type") {
      return error.instancePath === "" ? "instance-type" : "field-type";
    }
    const value = {
      required: "required-field",
      minLength: "minimum-length",
      maxLength: "maximum-length",
      pattern: "pattern",
      format: "date-time-format",
      additionalProperties: "unknown-field",
    }[error.keyword];
    requireValue(Boolean(value),
      "Lico Arc schema produced an unclassified rejection");
    return value;
  }));
  requireValue(classes.size === 1,
    "Lico Arc conformance case has ambiguous rejection classes");
  return [...classes][0];
}

function matchesJsonType(value, type) {
  if (type === "null") return value === null;
  if (type === "boolean") return typeof value === "boolean";
  if (type === "object") return isPlainObject(value);
  if (type === "array") return Array.isArray(value);
  if (type === "number") return typeof value === "number" && Number.isFinite(value);
  if (type === "integer") {
    return typeof value === "number" &&
      Number.isFinite(value) &&
      Number.isInteger(value);
  }
  return type === "string" && typeof value === "string";
}

function isRfc3339DateTime(value) {
  const match = /^(?<year>\d{4})-(?<month>\d{2})-(?<day>\d{2})[Tt](?<hour>\d{2}):(?<minute>\d{2}):(?<second>\d{2})(?:\.\d+)?(?<zone>[Zz]|(?<offsetSign>[+-])(?<offsetHour>\d{2}):(?<offsetMinute>\d{2}))$/u.exec(value);
  if (!match) return false;
  const year = Number(match.groups.year);
  const month = Number(match.groups.month);
  const day = Number(match.groups.day);
  const hour = Number(match.groups.hour);
  const minute = Number(match.groups.minute);
  const second = Number(match.groups.second);
  if (month < 1 || month > 12 ||
      day < 1 || day > daysInMonth(year, month) ||
      hour > 23 || minute > 59 || second > 60) {
    return false;
  }
  let offsetMinutes = 0;
  if (match.groups.offsetSign) {
    const offsetHour = Number(match.groups.offsetHour);
    const offsetMinute = Number(match.groups.offsetMinute);
    if (offsetHour > 23 || offsetMinute > 59) return false;
    offsetMinutes = (match.groups.offsetSign === "-" ? -1 : 1) *
      ((offsetHour * 60) + offsetMinute);
  }
  if (second < 60) return true;
  const utcMinute =
    (((hour * 60) + minute - offsetMinutes) % 1440 + 1440) % 1440;
  return utcMinute === 1439;
}

function daysInMonth(year, month) {
  return [
    31,
    year % 4 === 0 && (year % 100 !== 0 || year % 400 === 0) ? 29 : 28,
    31, 30, 31, 30, 31, 31, 30, 31, 30, 31,
  ][month - 1];
}

function canonicalizeJson(value) {
  if (value === null || typeof value === "boolean" ||
      typeof value === "number" || typeof value === "string") {
    return JSON.stringify(value);
  }
  if (Array.isArray(value)) {
    return `[${value.map(canonicalizeJson).join(",")}]`;
  }
  requireValue(isPlainObject(value), "canonical JSON value is unsupported");
  return `{${Object.keys(value).sort().map((key) =>
    `${JSON.stringify(key)}:${canonicalizeJson(value[key])}`).join(",")}}`;
}

function sortJsonValue(value) {
  if (Array.isArray(value)) return value.map(sortJsonValue);
  if (isPlainObject(value)) {
    return Object.fromEntries(Object.keys(value).sort().map((key) =>
      [key, sortJsonValue(value[key])]));
  }
  return value;
}

function serializeArtifact(bundle) {
  return Buffer.from(`${JSON.stringify(bundle, null, 2)}\n`);
}

function rejectsBundle(bytes) {
  try {
    validateLicoArcV1BundleBytes(bytes);
    return false;
  } catch {
    return true;
  }
}

function validationError(keyword, { instancePath, schemaPath }) {
  return { keyword, instancePath, schemaPath: `${schemaPath}/${keyword}` };
}

function escapeJsonPointer(value) {
  return value.replaceAll("~", "~0").replaceAll("/", "~1");
}

function assertClosedObject(value, expectedKeys, label) {
  requireValue(
    isPlainObject(value) &&
      arraysEqual(Object.keys(value).sort(), [...expectedKeys].sort()),
    `${label} fields do not match the closed contract`,
  );
}

function isPlainObject(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function arraysEqual(left, right) {
  return Array.isArray(left) &&
    Array.isArray(right) &&
    left.length === right.length &&
    left.every((value, index) => value === right[index]);
}

function requireValue(condition, message) {
  if (!condition) throw new Error(message);
}
