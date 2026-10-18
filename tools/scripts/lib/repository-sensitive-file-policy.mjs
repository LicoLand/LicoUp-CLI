// Canonical sensitive-file policy shared by the local Git publication gates,
// ignore rules, push Ruleset construction, and the publication-guard regression.
// Both consumers import the same frozen extension authority and content
// detector; no runtime substitution exists.

export const SENSITIVE_EXTENSION_REASON = "sensitive_extension";
export const SENSITIVE_CONTENT_REASON = "sensitive_content";

// Common certificate, provisioning-profile, key-container, and private-key
// filename extensions, including Apple notary key files (.p8). Public-key-only
// suffixes (.pub, .asc, .gpg) are intentionally absent: they are not rejected
// solely by name, while certificate or private-key content markers still fail.
export const sensitiveExtensions = Object.freeze(new Set([
  ".cer",
  ".cert",
  ".csr",
  ".certsigningrequest",
  ".crt",
  ".der",
  ".pem",
  ".p7b",
  ".p7c",
  ".p12",
  ".pfx",
  ".pkcs12",
  ".key",
  ".p8",
  ".pk8",
  ".pkcs8",
  ".jks",
  ".keystore",
  ".keychain",
  ".keychain-db",
  ".mobileprovision",
  ".provisionprofile",
  ".provisioningprofile",
]));

// Fixed byte-marker table. A certificate or private key is recognized only as
// a complete PEM block (BEGIN marker, base64-style body, matching END marker),
// so source files that merely mention markers are never rejected while real
// key material is.
const PEM_DASHES = "---" + "--";
const pemBoundary = (kind, label) => `${PEM_DASHES}${kind} ${label}${PEM_DASHES}`;

export const sensitiveContentMarkers = Object.freeze([
  [pemBoundary("BEGIN", "CERTIFICATE"), pemBoundary("END", "CERTIFICATE")],
  [pemBoundary("BEGIN", "TRUSTED CERTIFICATE"), pemBoundary("END", "TRUSTED CERTIFICATE")],
  [pemBoundary("BEGIN", "PRIVATE KEY"), pemBoundary("END", "PRIVATE KEY")],
  [pemBoundary("BEGIN", "RSA PRIVATE KEY"), pemBoundary("END", "RSA PRIVATE KEY")],
  [pemBoundary("BEGIN", "EC PRIVATE KEY"), pemBoundary("END", "EC PRIVATE KEY")],
  [pemBoundary("BEGIN", "DSA PRIVATE KEY"), pemBoundary("END", "DSA PRIVATE KEY")],
  [pemBoundary("BEGIN", "ENCRYPTED PRIVATE KEY"), pemBoundary("END", "ENCRYPTED PRIVATE KEY")],
  [pemBoundary("BEGIN", "OPENSSH PRIVATE KEY"), pemBoundary("END", "OPENSSH PRIVATE KEY")],
  [pemBoundary("BEGIN", "PGP PRIVATE KEY BLOCK"), pemBoundary("END", "PGP PRIVATE KEY BLOCK")],
  [pemBoundary("BEGIN", "PKCS7"), pemBoundary("END", "PKCS7")],
]);

const MAX_PEM_BLOCK_BYTES = 4 * 1024 * 1024;
const MIN_PEM_BODY_BYTES = 16;
// PEM bodies are base64 with line breaks; encrypted blocks may also carry
// Proc-Type and DEK-Info header lines.
const PEM_BODY_BYTE = new Set(
  [..."ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/= \t\r\n:,-"]
    .map((char) => char.charCodeAt(0)),
);

export function normalizeSensitivePath(relativePath) {
  if (typeof relativePath !== "string" || relativePath.length === 0) return "";
  return relativePath.replaceAll("\\", "/").toLowerCase();
}

export function classifyPath(relativePath) {
  const normalized = normalizeSensitivePath(relativePath);
  const base = normalized.slice(normalized.lastIndexOf("/") + 1);
  for (const extension of sensitiveExtensions) {
    if (base.endsWith(extension)) {
      return Object.freeze({ verdict: "reject", reason: SENSITIVE_EXTENSION_REASON });
    }
  }
  return Object.freeze({ verdict: "pass", reason: null });
}

export function sensitiveRulesetExtensions() {
  return Object.freeze([...sensitiveExtensions].map((extension) => extension.slice(1)));
}

// Streaming content detector. Marker matches are discovered from a bounded
// carry, while each marker type keeps only candidate byte offsets. Invalid
// body bytes discard every open candidate immediately. Candidate queues are
// pruned at the fixed block limit, so neither input size nor chunk boundaries
// change the verdict or cause unbounded buffering.
export class SensitiveContentScanner {
  constructor() {
    this.result = Object.freeze({ verdict: "pass", reason: null });
    this.longestMarker = 0;
    for (const [begin, end] of sensitiveContentMarkers) {
      this.longestMarker = Math.max(this.longestMarker, begin.length, end.length);
    }
    this.carry = Buffer.alloc(0);
    this.processedBytes = 0;
    this.candidates = sensitiveContentMarkers.map(() => ({ starts: [], head: 0 }));
  }

  feed(chunk) {
    if (this.result.verdict === "reject") return this.result;
    const input = Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk);
    if (input.length === 0) return this.result;
    const buffer = this.carry.length > 0 ? Buffer.concat([this.carry, input]) : input;
    const carryLength = this.carry.length;
    const events = new Map();
    const addEvents = (marker, markerIndex, type) => {
      let from = 0;
      while (from < buffer.length) {
        const start = buffer.indexOf(marker, from);
        if (start < 0) break;
        const end = start + marker.length - 1;
        if (end >= carryLength) {
          const event = { markerIndex, type, markerLength: marker.length };
          const existing = events.get(end);
          if (existing) existing.push(event);
          else events.set(end, [event]);
        }
        from = start + 1;
      }
    };
    sensitiveContentMarkers.forEach(([begin, end], markerIndex) => {
      addEvents(begin, markerIndex, "begin");
      addEvents(end, markerIndex, "end");
    });

    for (let inputIndex = 0; inputIndex < input.length; inputIndex += 1) {
      const absoluteIndex = this.processedBytes + inputIndex;
      if (!PEM_BODY_BYTE.has(input[inputIndex])) {
        this.candidates.forEach((queue) => {
          queue.starts = [];
          queue.head = 0;
        });
      }
      const combinedIndex = carryLength + inputIndex;
      for (const event of events.get(combinedIndex) || []) {
        const queue = this.candidates[event.markerIndex];
        if (event.type === "begin") {
          const bodyStart = absoluteIndex + 1;
          while (
            queue.head < queue.starts.length &&
            bodyStart - queue.starts[queue.head] > MAX_PEM_BLOCK_BYTES
          ) {
            queue.head += 1;
          }
          queue.starts.push(bodyStart);
          if (queue.head > 1024 && queue.head * 2 > queue.starts.length) {
            queue.starts = queue.starts.slice(queue.head);
            queue.head = 0;
          }
          continue;
        }
        const bodyEnd = absoluteIndex - event.markerLength + 1;
        while (
          queue.head < queue.starts.length &&
          bodyEnd - queue.starts[queue.head] > MAX_PEM_BLOCK_BYTES
        ) {
          queue.head += 1;
        }
        if (
          queue.head < queue.starts.length &&
          bodyEnd - queue.starts[queue.head] >= MIN_PEM_BODY_BYTES
        ) {
          this.result = Object.freeze({ verdict: "reject", reason: SENSITIVE_CONTENT_REASON });
          return this.result;
        }
        if (queue.head > 1024 && queue.head * 2 > queue.starts.length) {
          queue.starts = queue.starts.slice(queue.head);
          queue.head = 0;
        }
      }
    }
    this.processedBytes += input.length;
    const keep = Math.min(this.longestMarker - 1, buffer.length);
    this.carry = keep > 0 ? Buffer.from(buffer.subarray(buffer.length - keep)) : Buffer.alloc(0);
    return this.result;
  }

  finish() {
    return this.result;
  }
}
