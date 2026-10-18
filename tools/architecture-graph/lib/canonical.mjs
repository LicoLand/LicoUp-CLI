import { createHash } from "node:crypto";
import path from "node:path";

/**
 * Canonical JSON with the same byte form as the Python reference tool's
 * `json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":"))`.
 *
 * Agreeing on this form matters: the development ledger binds claims to
 * `graph_digest` / task fingerprints, and both tools must name the same graph
 * version or a claim would silently describe a different graph.
 *
 * Only JSON produced by `JSON.parse` from these graph documents is expected here,
 * so numbers are integers or finite decimals that round-trip through
 * `Number.prototype.toString` exactly like Python's shortest-repr for `int`.
 */
export function canonicalJson(value) {
  if (value === null || typeof value === "boolean" || typeof value === "number") {
    return JSON.stringify(value);
  }
  if (typeof value === "string") {
    return JSON.stringify(value);
  }
  if (Array.isArray(value)) {
    return `[${value.map(canonicalJson).join(",")}]`;
  }
  if (value === undefined) {
    throw new TypeError("canonical JSON cannot encode undefined");
  }
  const keys = Object.keys(value).sort();
  const parts = [];
  for (const key of keys) {
    if (value[key] === undefined) continue;
    parts.push(`${JSON.stringify(key)}:${canonicalJson(value[key])}`);
  }
  return `{${parts.join(",")}}`;
}

export function sha256Hex(value) {
  return createHash("sha256").update(value).digest("hex");
}

export function digestOf(value) {
  return sha256Hex(Buffer.from(canonicalJson(value), "utf8"));
}

export class GraphError extends Error {
  constructor(message) {
    super(message);
    this.name = "GraphError";
  }
}

export function requireFact(condition, message) {
  if (!condition) throw new GraphError(message);
}

/**
 * A graph identity must be a repository-relative, normalized path. It is a
 * declaration of source ownership, never an operating-system sandbox.
 */
export function assertRepoRelativePath(text, label = "path") {
  requireFact(typeof text === "string" && text.length > 0, `${label} must be a non-empty string`);
  requireFact(!text.includes("\\"), `${label} must use forward slashes: ${text}`);
  requireFact(!text.includes("\0"), `${label} must not contain NUL: ${JSON.stringify(text)}`);
  requireFact(!text.includes(":"), `${label} must not contain a drive or scheme separator: ${text}`);
  requireFact(text.startsWith("~") === false, `${label} must not start with a home alias: ${text}`);
  requireFact(!path.posix.isAbsolute(text), `${label} must be repository-relative: ${text}`);
  const trimmed = text.replace(/\/+$/u, "");
  const bare = trimmed.endsWith("/**") ? trimmed.slice(0, -3) : trimmed;
  requireFact(!/[*?[\]]/u.test(bare), `${label} only supports an exact path or a terminal /** : ${text}`);
  requireFact(bare.includes("//") === false, `${label} must not contain empty path segments: ${text}`);
  const segments = bare.split("/");
  requireFact(
    segments.every((segment) => segment.length > 0 && segment !== "." && segment !== ".."),
    `${label} must not contain relative segments: ${text}`,
  );
  // Accepted spellings only: `a/b`, `a/b/`, `a/b/**`. Anything else is a
  // declaration that cannot be compared or hashed consistently.
  requireFact(
    text === bare || text === `${bare}/` || text === `${bare}/**`,
    `${label} must already be normalized: ${text}`,
  );
  return text;
}

export function resolveInsideRoot(root, relativePath, label = "path") {
  assertRepoRelativePath(relativePath, label);
  const trimmed = relativePath.replace(/\/\*\*$/u, "").replace(/\/$/u, "");
  const resolved = path.resolve(root, trimmed);
  const relative = path.relative(root, resolved);
  requireFact(
    relative.length > 0 && !relative.startsWith("..") && !path.isAbsolute(relative),
    `${label} escapes the repository root: ${relativePath}`,
  );
  return resolved;
}

/**
 * Repository path overlap used for source scopes. A trailing `/` or `/**` means
 * "this path or anything below it"; anything else is exact. Comparison folds
 * case because a case-insensitive developer filesystem is the conservative
 * assumption for a conflict, and this is a declaration check, not a filesystem
 * access-control decision.
 */
export function pathsOverlap(left, right) {
  const shape = (value) => {
    const prefix = value.endsWith("/") || value.endsWith("/**");
    return { prefix, bare: value.replace(/\/\*\*$/u, "").replace(/\/+$/u, "").toLowerCase() };
  };
  const a = shape(left);
  const b = shape(right);
  if (!a.prefix && !b.prefix) return a.bare === b.bare;
  if (a.prefix && b.prefix) {
    return a.bare === b.bare || a.bare.startsWith(`${b.bare}/`) || b.bare.startsWith(`${a.bare}/`);
  }
  const prefix = a.prefix ? a : b;
  const exact = a.prefix ? b : a;
  return exact.bare === prefix.bare || exact.bare.startsWith(`${prefix.bare}/`);
}

/** Conservative test for "this declaration names a real repository path". */
export function looksLikeRepoPath(text) {
  return typeof text === "string"
    && /^[A-Za-z0-9][A-Za-z0-9._-]*(?:\/[A-Za-z0-9][A-Za-z0-9._-]*)*\/?$/u.test(text);
}
