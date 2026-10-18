export class ReducerError extends Error {
  constructor(code, details = []) {
    super(code);
    this.name = "ReducerError";
    this.code = code;
    // Structured diagnosis for a developer-facing report: field names, digest
    // values and repo-relative paths only, never conversation content.
    this.details = details;
  }
}

export function fail(code) {
  throw new ReducerError(code);
}
