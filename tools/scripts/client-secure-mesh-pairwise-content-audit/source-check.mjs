import path from "node:path";
import fs from "node:fs/promises";
import { readSourceCheckBundle } from "../lib/source-check-bundle.mjs";
import { sanitizeError } from "./privacy.mjs";

export function createReadText(repoRoot) {
  return async function readText(relativePath) {
    return fs.readFile(path.join(repoRoot, relativePath), "utf8");
  };
}

export function functionBody(source, name) {
  const start = source.indexOf(`fn ${name}`);
  if (start < 0) {
    return "";
  }
  const braceStart = source.indexOf("{", start);
  if (braceStart < 0) {
    return "";
  }
  let depth = 0;
  for (let index = braceStart; index < source.length; index += 1) {
    const char = source[index];
    if (char === "{") depth += 1;
    if (char === "}") {
      depth -= 1;
      if (depth === 0) {
        return source.slice(braceStart, index + 1);
      }
    }
  }
  return "";
}

export async function evaluateSourceCheck(check, readText) {
  try {
    const { files, source } = await readSourceCheckBundle(check, readText);
    const scopedSource = check.functionName ? functionBody(source, check.functionName) : source;
    const missingTokens = (check.tokens || []).filter((token) => !scopedSource.includes(token));
    const forbiddenPresent = (check.forbiddenTokens || []).filter((token) => scopedSource.includes(token));
    return {
      id: check.id,
      file: check.file,
      files,
      ok: missingTokens.length === 0 && forbiddenPresent.length === 0,
      missingTokens,
      forbiddenPresent
    };
  } catch (error) {
    return {
      id: check.id,
      file: check.file,
      files: check.files || [check.file],
      ok: false,
      missingTokens: [],
      forbiddenPresent: [],
      error: sanitizeError(error)
    };
  }
}
