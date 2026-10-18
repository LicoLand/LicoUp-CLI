import fs from "node:fs";
import path from "node:path";

import Ajv2020 from "ajv/dist/2020.js";
import addFormats from "ajv-formats";

import { GraphError, requireFact } from "./canonical.mjs";

/**
 * Structural checking delegates to the repository's existing JSON Schema
 * validation library (ajv + ajv-formats, already a devDependency and already
 * used by tools/scripts/*). This module deliberately contains no schema
 * interpretation of its own: it only binds the plan's `*.schema.json` documents
 * to that library and formats the resulting errors.
 *
 * JSON Schema proves shape. It cannot prove that an ID resolves, that a
 * relation type is meaningful, or that a development DAG is acyclic; those stay
 * in graph-model.mjs and are not a substitute for each other.
 */

const MAX_DOCUMENT_BYTES = 16 * 1024 * 1024;

export function readJsonDocument(filePath, label = filePath) {
  const stats = fs.statSync(filePath);
  requireFact(stats.isFile(), `${label} is not a file`);
  requireFact(stats.size <= MAX_DOCUMENT_BYTES, `${label} exceeds the 16 MiB graph document bound`);
  let parsed;
  try {
    parsed = JSON.parse(fs.readFileSync(filePath, "utf8"));
  } catch (error) {
    throw new GraphError(`${label} is not valid JSON: ${error.message}`);
  }
  requireFact(parsed !== null && typeof parsed === "object" && !Array.isArray(parsed),
    `${label} must be a JSON object`);
  return parsed;
}

let compiled = null;

function validatorFor(schemaPath) {
  compiled ??= new Map();
  if (!compiled.has(schemaPath)) {
    const schema = JSON.parse(fs.readFileSync(schemaPath, "utf8"));
    const ajv = new Ajv2020({ allErrors: true, strict: true, strictRequired: false });
    addFormats(ajv);
    compiled.set(schemaPath, ajv.compile(schema));
  }
  return compiled.get(schemaPath);
}

export function schemaPathBeside(documentPath) {
  const parsed = path.parse(documentPath);
  return path.join(parsed.dir, `${parsed.name}.schema.json`);
}

function formatErrors(errors) {
  return (errors ?? [])
    .map((error) => `${error.instancePath || "/"} ${error.message ?? "is invalid"}`)
    .sort();
}

/**
 * Validate one graph document against the schema that sits beside it.
 * A missing sibling schema is an error: silently skipping validation would let
 * a document drift away from its published shape.
 */
export function checkStructure({ document, documentPath, schemaPath = schemaPathBeside(documentPath) }) {
  requireFact(fs.existsSync(schemaPath), `missing schema beside ${documentPath}: ${schemaPath}`);
  const validate = validatorFor(schemaPath);
  if (validate(document)) return { document: documentPath, schema: schemaPath, valid: true, errors: [] };
  return {
    document: documentPath,
    schema: schemaPath,
    valid: false,
    errors: formatErrors(validate.errors),
  };
}

export function requireStructure({ document, documentPath, schemaPath }) {
  const result = checkStructure({ document, documentPath, schemaPath });
  if (!result.valid) {
    throw new GraphError(`${documentPath} does not match ${result.schema}: ${result.errors.join("; ")}`);
  }
  return result;
}
