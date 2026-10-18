import { readSourceCheckBundle } from "../lib/source-check-bundle.mjs";
import { sanitizeError } from "./privacy.mjs";
import { readText } from "./io.mjs";

export async function evaluateSourceCheck(check) {
  try {
    const { files, source } = await readSourceCheckBundle(check, readText);
    const missingTokens = (check.tokens || []).filter((token) => !source.includes(token));
    const forbiddenPresent = (check.forbiddenTokens || []).filter((token) =>
      source.includes(token)
    );
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

export function selectedSourceChecksReady(sourceResults, ids) {
  const byId = new Map(sourceResults.map((result) => [result.id, result.ok]));
  return ids.every((id) => byId.get(id) === true);
}
