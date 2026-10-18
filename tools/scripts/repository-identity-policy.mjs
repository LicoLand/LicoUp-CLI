#!/usr/bin/env node

import { execFileSync, spawn } from "node:child_process";
import {
  chmodSync,
  lstatSync,
  readFileSync,
  writeFileSync,
} from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath, pathToFileURL } from "node:url";
import {
  SensitiveContentScanner,
  classifyPath,
} from "./lib/repository-sensitive-file-policy.mjs";

const repoRoot = fileURLToPath(new URL("../../", import.meta.url));
// Windows filesystems carry no POSIX execute bit, so chmod on a managed hook
// cannot be observed there; Git for Windows runs core.hooksPath hooks through
// its bundled shell. Every other platform keeps the mandatory assertion.
const posixExecutableBitRequired = process.platform !== "win32";
const zeroObjectId = /^0+$/u;
const githubLoginPattern = /^[A-Za-z0-9](?:[A-Za-z0-9-]{0,37}[A-Za-z0-9])?$/u;
export const managedPolicyPaths = Object.freeze([
  ".githooks/pre-commit",
  ".githooks/commit-msg",
  ".githooks/pre-push",
  "package.json",
  "tools/client-release-template.json",
  "tools/scripts/client-gate-policy.mjs",
  "tools/scripts/client-gate.mjs",
  "tools/scripts/repository-rulesets.mjs",
  "tools/scripts/repository-identity-policy.mjs",
  "tools/scripts/lib/repository-sensitive-file-policy.mjs",
]);
const prohibitedAttributionTrailer =
  /(?:^|\r?\n)[ \t]*(?:co-authored-by|co-committed-by|signed-off-by|authored-by|assisted-by|generated-by|written-by|pair-programmed-by|contributed-by|reviewed-by|suggested-by|reported-by)[ \t]*:/iu;
const agentIdentityLine =
  /(?:^|\r?\n)[ \t]*(?:claude(?: code)?|cursor(?: agent)?|github copilot|copilot|codex|chatgpt|gemini|anthropic|openai|[^\r\n<]*(?:agent|bot))[^\r\n]*<[^\r\n>]+>/iu;
const agentIdentityValue =
  /(?:\[bot\]|(?:^|[^a-z0-9])(?:claude(?:[ ._+-]*code)?|cursor(?:[ ._+-]*agent)?|github[ ._+-]*copilot|copilot|codex|chatgpt|gemini|anthropic|openai|agent|bot)(?:[^a-z0-9]|$))/iu;
const forbiddenStagedPrefixes = Object.freeze([
  ".agents/",
  ".claude/",
  ".codex/",
  ".cursor/",
  ".kilo/",
  "docs/plans/",
  "docs/reports/",
  "scripts/local/",
  "skills/",
  "tools/local/",
]);

export class IdentityPolicyError extends Error {
  constructor(code, message) {
    super(message);
    this.name = "IdentityPolicyError";
    this.code = code;
  }
}

function reject(code, message) {
  throw new IdentityPolicyError(code, message);
}

// A reported notice is loud and non-blocking. It names a condition a human should judge;
// it never stops the operation. Keep self-integrity findings here rather than in `reject`.
function report(code, message) {
  process.stderr.write(`LicoUp identity gate (reported, not blocking): ${code}: ${message}\n`);
}

function run(command, args, options = {}) {
  try {
    return execFileSync(command, args, {
      cwd: repoRoot,
      encoding: "utf8",
      stdio: [options.input === undefined ? "ignore" : "pipe", "pipe", "pipe"],
      input: options.input,
    }).trim();
  } catch {
    reject(options.code || "COMMAND_FAILED", options.message || "A required command failed.");
  }
}

function runOptionalIn(command, args, cwd) {
  try {
    return execFileSync(command, args, {
      cwd,
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"],
    }).trim();
  } catch {
    return "";
  }
}

function runOptional(command, args) {
  return runOptionalIn(command, args, repoRoot);
}

export function isMergeInProgress(cwd = repoRoot) {
  return runOptionalIn("git", ["rev-parse", "-q", "--verify", "MERGE_HEAD"], cwd) !== "";
}

export function shouldRejectEmptyStagedCandidate(hasStagedPaths, mergeInProgress) {
  return hasStagedPaths === false && mergeInProgress !== true;
}

export function canonicalGitHubEmail(identity) {
  return `${identity.id}+${identity.login}@users.noreply.github.com`;
}

export function parseGitHubIdentity(raw) {
  const [login, id, ...extra] = raw.trim().split("\t");
  if (
    extra.length > 0 ||
    !githubLoginPattern.test(login || "") ||
    !/^[1-9][0-9]*$/u.test(id || "")
  ) {
    reject("GH_IDENTITY_INVALID", "GitHub returned an unusable account identity.");
  }
  return Object.freeze({ login, id });
}

// The canonical noreply address carries both facts the identity check needs:
//
//   <id>+<login>@users.noreply.github.com
//
// Reading them back out of the configured address is the same data the API used to
// supply, without a second credential, a network round trip, or a dependency on a
// command that may not be installed.
export function parseCanonicalGitHubEmail(email) {
  const match = /^([1-9][0-9]*)\+([^+@]+)@users\.noreply\.github\.com$/u.exec(
    (email || "").trim(),
  );
  if (!match) {
    return null;
  }
  return parseGitHubIdentity(`${match[2]}\t${match[1]}`);
}

export function boundedIdentityRead(operation, attempts = 3) {
  if (typeof operation !== "function" || !Number.isSafeInteger(attempts) || attempts < 1) {
    reject("READ_RETRY_INVALID", "The bounded identity read policy is invalid.");
  }
  let failure;
  for (let attempt = 1; attempt <= attempts; attempt += 1) {
    try {
      return operation();
    } catch (error) {
      failure = error;
      if (attempt < attempts) {
        Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, attempt * 250);
      }
    }
  }
  throw failure;
}

// The repository's identity is read from its own Git configuration.
//
// It used to come from `gh api /user`, which made every commit depend on a logged-in
// GitHub CLI and a working network connection. That is the wrong dependency twice
// over: a local commit has nothing to authenticate against the API, and the account a
// CLI happens to be logged into is not the account this repository publishes under. A
// disconnected or CLI-less machine could not commit at all, and the check that failed
// was the one whose answer was already on disk.
function currentGitHubIdentity() {
  const configured = effectiveGitConfig("user.email");
  const identity = parseCanonicalGitHubEmail(configured);
  if (!identity) {
    reject(
      "GIT_IDENTITY_UNCONFIGURED",
      `Repository user.email is not a canonical GitHub noreply address: ${configured || "(unset)"}. ` +
        "Run npm run repo:identity:install.",
    );
  }
  return identity;
}

function localGitConfig(key) {
  return runOptional("git", ["config", "--local", "--get", key]);
}

// The identity a commit is actually attributed to.
//
// `--local` answers a different question: whether this checkout carries its own
// override. Conflating the two made the gate fail on a correctly configured checkout
// whose identity arrives by path (`includeIf` on the workspace directory) rather than
// from a per-repository value, and the failure named a mismatch that did not exist.
// The check below is about what the commit will carry, so it reads what the commit
// will carry.
function effectiveGitConfig(key) {
  return runOptional("git", ["config", "--get", key]);
}

export function assertCommitMessage(message) {
  if (prohibitedAttributionTrailer.test(message)) {
    reject(
      "ATTRIBUTION_TRAILER_FORBIDDEN",
      "Attribution trailers are forbidden; the commit must have exactly one developer identity.",
    );
  }
  if (agentIdentityLine.test(message)) {
    reject(
      "AGENT_IDENTITY_FORBIDDEN",
      "An Agent-shaped identity line is forbidden in the commit message.",
    );
  }
}

export function isAgentIdentity(name, email) {
  return agentIdentityValue.test(`${name || ""} <${email || ""}>`);
}

export function assertCommitRecord(record) {
  if (isAgentIdentity(record.authorName, record.authorEmail)) {
    reject(
      "AGENT_AUTHOR_IDENTITY_FORBIDDEN",
      "An Agent must not appear as the commit Author.",
    );
  }
  if (isAgentIdentity(record.committerName, record.committerEmail)) {
    reject(
      "AGENT_COMMITTER_IDENTITY_FORBIDDEN",
      "An Agent must not appear as the commit Committer.",
    );
  }
  assertCommitMessage(record.message);
}

function assertConfiguredIdentity(identity) {
  const expectedEmail = canonicalGitHubEmail(identity);
  if (
    effectiveGitConfig("user.name") !== identity.login ||
    effectiveGitConfig("user.email") !== expectedEmail
  ) {
    reject(
      "LOCAL_IDENTITY_MISMATCH",
      "Repository Git identity does not match the identity this repository publishes under. Run npm run repo:identity:install.",
    );
  }
}

function gitPrivatePath(name) {
  const value = run("git", ["rev-parse", "--git-path", name], {
    code: "GIT_REPOSITORY_REQUIRED",
    message: "A Git repository is required.",
  });
  return path.resolve(repoRoot, value);
}

function stagedReceiptPath() {
  return gitPrivatePath("licoup-identity-receipt.json");
}

// The managed policy files are the tooling that decides what may be committed: the Git
// hooks, the sensitive-file policy, the ruleset planner and this script. A local edit to
// them is worth saying out loud. It is not worth stopping the commit.
//
// This check previously compared every file with a digest recorded by `install`. That made
// it satisfiable only while the checkout stayed on the exact revision present at install
// time: a branch switch, merge, rebase or pull changed those files legitimately and left
// the checkout unable to commit anything at all, while a deliberate edit could satisfy the
// check by running `install` again. It therefore cost a great deal and detected little.
//
// Comparing with the committed state at HEAD keeps the signal that matters — an
// uncommitted local edit to the gate's own machinery — with no revision-bound false
// positives, and needs no per-checkout installed state.
//
// Do not reintroduce install-time hashing, a manifest, or a blocking gate here. Drift is
// reported for a human to judge; the published result stays protected by the independent
// pull-request commit-identity workflow.
export function managedPolicyDrift(rootDirectory = repoRoot) {
  const drifted = new Set();
  const changed = runOptionalIn("git", ["diff", "--name-only", "HEAD", "--", ...managedPolicyPaths], rootDirectory);
  for (const entry of String(changed || "").split("\n").map((line) => line.trim()).filter(Boolean)) {
    drifted.add(entry);
  }
  const untracked = runOptionalIn(
    "git",
    ["ls-files", "--others", "--exclude-standard", "--", ...managedPolicyPaths],
    rootDirectory,
  );
  for (const entry of String(untracked || "").split("\n").map((line) => line.trim()).filter(Boolean)) {
    drifted.add(entry);
  }
  for (const relativePath of managedPolicyPaths) {
    const metadata = lstatSync(path.join(rootDirectory, relativePath), { throwIfNoEntry: false });
    if (!metadata || !metadata.isFile() || metadata.isSymbolicLink()) drifted.add(relativePath);
  }
  return [...drifted].sort();
}

function reportPolicyNotices() {
  if (localGitConfig("core.hooksPath") !== ".githooks") {
    report(
      "HOOKS_PATH_NOT_ENABLED",
      "Repository-controlled Git hooks are not enabled, so this gate is not protecting ordinary "
      + "commits. Run npm run repo:identity:install.",
    );
  }
  for (const relativePath of managedPolicyPaths) {
    if (!relativePath.startsWith(".githooks/")) continue;
    const metadata = lstatSync(path.join(repoRoot, relativePath), { throwIfNoEntry: false });
    if (
      posixExecutableBitRequired
      && metadata?.isFile()
      && (metadata.mode & 0o111) === 0
    ) {
      report("HOOK_NOT_EXECUTABLE", `${relativePath} is not executable, so Git will not run it.`);
    }
  }
  const drifted = managedPolicyDrift();
  if (drifted.length > 0) {
    report(
      "POLICY_FILE_DRIFT",
      `Managed policy file(s) differ from the committed revision: ${drifted.join(", ")}. `
      + "This is reported, never blocked; review the local edit before relying on this gate.",
    );
  }
}

function stagedPaths() {
  const output = run("git", [
    "diff",
    "--cached",
    "--name-only",
    "-z",
    "--diff-filter=ACMRTUXB",
  ]);
  return output ? output.split("\0").filter(Boolean) : [];
}

function assertStagedCandidate() {
  const paths = stagedPaths();
  if (shouldRejectEmptyStagedCandidate(paths.length > 0, isMergeInProgress())) {
    reject("EMPTY_STAGED_CANDIDATE", "There are no staged source changes to commit.");
  }
  for (const relativePath of paths) {
    const normalized = relativePath.replaceAll("\\", "/");
    if (forbiddenStagedPrefixes.some((prefix) => normalized.startsWith(prefix))) {
      reject("LOCAL_ONLY_PATH_STAGED", "A local-only path is staged for publication.");
    }
    const indexEntry = runOptional("git", ["ls-files", "-s", "--", relativePath]);
    if (!indexEntry.startsWith("120000 ")) continue;
    const target = run("git", ["show", `:${relativePath}`], {
      code: "STAGED_SYMLINK_UNREADABLE",
      message: "A staged symbolic link cannot be inspected.",
    });
    const normalizedTarget = target.replaceAll("\\", "/");
    if (
      path.posix.isAbsolute(normalizedTarget) ||
      normalizedTarget.split("/").includes("..")
    ) {
      reject("STAGED_SYMLINK_UNSAFE", "A staged symbolic link escapes the repository.");
    }
  }
}

export function parseRawDiffEntries(output) {
  // Parses `git diff --cached --raw -z` and `git diff-tree -r --root -z`
  // output. Each entry is `:<src_mode> <dst_mode> <src_oid> <dst_oid> <status>`
  // followed by NUL-terminated path tokens; rename and copy entries carry the
  // source path and then the destination path.
  const tokens = output.split("\0").filter((token) => token.length > 0);
  const entries = [];
  let index = 0;
  while (index < tokens.length) {
    const fields = tokens[index];
    if (!fields.startsWith(":")) {
      index += 1;
      continue;
    }
    const parts = fields.slice(1).split(" ");
    const status = parts[4] || "";
    const isRenameOrCopy = status.startsWith("R") || status.startsWith("C");
    const pathCount = isRenameOrCopy ? 2 : 1;
    const paths = tokens.slice(index + 1, index + 1 + pathCount);
    if (paths.some((candidate) => candidate === undefined)) {
      reject("DIFF_ENTRY_INVALID", "A Git diff entry is invalid.");
    }
    entries.push({
      path: isRenameOrCopy ? paths[1] || paths[0] : paths[0],
      status: status[0] || "",
      srcOid: parts[2] || "",
      dstOid: parts[3] || "",
    });
    index += 1 + pathCount;
  }
  return entries;
}

function stagedEntries() {
  const output = run("git", [
    "diff",
    "--cached",
    "--raw",
    "-z",
    "--abbrev=40",
    "--diff-filter=ACMRTUXB",
  ]);
  return parseRawDiffEntries(output);
}

function outgoingCommitEntries(commits) {
  const records = [];
  for (const commit of commits) {
    const output = run("git", [
      "diff-tree",
      "-r",
      "--root",
      "-m",
      "-z",
      "--abbrev=40",
      "--no-commit-id",
      commit,
    ], {
      code: "OUTGOING_DIFF_UNAVAILABLE",
      message: "An outgoing commit diff could not be inspected.",
    });
    records.push({ commit, entries: parseRawDiffEntries(output) });
  }
  return records;
}

// Pure staged-object check: sensitive destination paths fail before any
// content read, and only newly introduced object identifiers are returned.
export function stagedObjectChecks(entries) {
  const readOids = new Set();
  for (const entry of entries) {
    if (entry.status === "D") continue;
    if (classifyPath(entry.path).verdict === "reject") {
      return Object.freeze({ status: "reject", code: "SENSITIVE_PATH_STAGED", readOids: [] });
    }
    const { srcOid, dstOid } = entry;
    if (!zeroObjectId.test(dstOid) && dstOid !== srcOid) readOids.add(dstOid);
  }
  return Object.freeze({ status: "pass", readOids: Object.freeze([...readOids]) });
}

// Pure outgoing-history check across every introduced destination path and
// newly introduced blob, so add-then-delete and rename histories cannot evade
// the gate; duplicate object identifiers are read at most once.
export function outgoingObjectChecks(commitRecords) {
  const readOids = new Set();
  for (const record of commitRecords) {
    for (const entry of record.entries) {
      if (entry.status === "D") continue;
      if (classifyPath(entry.path).verdict === "reject") {
        return Object.freeze({ status: "reject", code: "SENSITIVE_PATH_OUTGOING", readOids: [] });
      }
      const { srcOid, dstOid } = entry;
      if (!zeroObjectId.test(dstOid) && dstOid !== srcOid) readOids.add(dstOid);
    }
  }
  return Object.freeze({ status: "pass", readOids: Object.freeze([...readOids]) });
}

const SENSITIVE_READ_CHUNK_BYTES = 64 * 1024;

class CatFileBatchReader {
  constructor() {
    this.child = spawn("git", ["cat-file", "--batch"], {
      cwd: repoRoot,
      stdio: ["pipe", "pipe", "pipe"],
    });
    this.buffer = Buffer.alloc(0);
    this.done = false;
    this.iterator = null;
    this.stderr = "";
    this.child.stderr.on("data", (chunk) => {
      this.stderr += chunk;
    });
    this.exitPromise = new Promise((resolve) => {
      this.child.on("error", (error) => resolve({ error }));
      this.child.on("close", (exitCode) => resolve({ exitCode }));
    });
  }

  async fill() {
    if (this.iterator === null) this.iterator = this.child.stdout[Symbol.asyncIterator]();
    const next = await this.iterator.next();
    if (next.done) {
      this.done = true;
      return;
    }
    this.buffer = Buffer.concat([this.buffer, next.value]);
  }

  async readLine() {
    let newline = this.buffer.indexOf(0x0a);
    while (newline < 0) {
      await this.fill();
      if (this.done) return null;
      newline = this.buffer.indexOf(0x0a);
    }
    const line = this.buffer.subarray(0, newline);
    this.buffer = this.buffer.subarray(newline + 1);
    return line.toString("utf8");
  }

  async readExactly(count) {
    while (this.buffer.length < count) {
      await this.fill();
      if (this.done) return null;
    }
    const bytes = this.buffer.subarray(0, count);
    this.buffer = this.buffer.subarray(count);
    return bytes;
  }
}

async function scanBlobContents(blobIds, code) {
  const uniqueIds = [...new Set(blobIds)];
  if (uniqueIds.length === 0) return "pass";
  const reader = new CatFileBatchReader();
  reader.child.stdin.write(`${uniqueIds.join("\n")}\n`);
  reader.child.stdin.end();
  try {
    for (const blobId of uniqueIds) {
      const header = await reader.readLine();
      if (header === null) {
        const outcome = await reader.exitPromise;
        if (outcome.error) throw outcome.error;
        reject("SENSITIVE_OBJECT_UNREADABLE", "A staged or outgoing object could not be inspected.");
      }
      const parts = header.split(" ");
      if (parts[0] !== blobId || parts[1] !== "blob" || !/^[0-9]+$/u.test(parts[2] || "")) {
        reject("SENSITIVE_OBJECT_UNREADABLE", "A staged or outgoing object could not be inspected.");
      }
      const size = Number(parts[2]);
      const scanner = new SensitiveContentScanner();
      let remaining = size;
      while (remaining > 0) {
        const chunk = await reader.readExactly(Math.min(remaining, SENSITIVE_READ_CHUNK_BYTES));
        if (chunk === null) {
          reject("SENSITIVE_OBJECT_UNREADABLE", "A staged or outgoing object could not be inspected.");
        }
        if (scanner.feed(chunk).verdict === "reject") {
          reject(code, "A staged or outgoing object is rejected by the sensitive-file policy.");
        }
        remaining -= chunk.length;
      }
      const trailer = await reader.readExactly(1);
      if (trailer === null) {
        reject("SENSITIVE_OBJECT_UNREADABLE", "A staged or outgoing object could not be inspected.");
      }
    }
    const outcome = await reader.exitPromise;
    if (outcome.error) throw outcome.error;
    if (outcome.exitCode !== 0 || reader.stderr.trim() !== "") {
      reject("SENSITIVE_OBJECT_UNREADABLE", "A staged or outgoing object could not be inspected.");
    }
  } finally {
    reader.child.kill();
  }
  return "pass";
}

function writeStagedReceipt() {
  const tree = run("git", ["write-tree"], {
    code: "STAGED_TREE_UNAVAILABLE",
    message: "The staged tree could not be materialized.",
  });
  writeFileSync(
    stagedReceiptPath(),
    `${JSON.stringify({ schemaVersion: 1, tree })}\n`,
    { encoding: "utf8", mode: 0o600 },
  );
}

function assertStagedReceipt() {
  let receipt;
  try {
    receipt = JSON.parse(readFileSync(stagedReceiptPath(), "utf8"));
  } catch {
    reject("STAGED_RECEIPT_MISSING", "The pre-commit identity receipt is missing.");
  }
  const tree = run("git", ["write-tree"], {
    code: "STAGED_TREE_UNAVAILABLE",
    message: "The staged tree could not be materialized.",
  });
  if (receipt.schemaVersion !== 1 || receipt.tree !== tree) {
    reject("STAGED_TREE_CHANGED", "The staged tree changed after the pre-commit gate.");
  }
}

function readCommitRecord(commit) {
  const output = run(
    "git",
    ["show", "-s", "--format=%an%x00%ae%x00%cn%x00%ce%x00%B%x00", commit],
    {
      code: "COMMIT_UNREADABLE",
      message: "An outgoing commit could not be inspected.",
    },
  );
  const [authorName, authorEmail, committerName, committerEmail, message] =
    output.split("\0");
  if ([authorName, authorEmail, committerName, committerEmail, message].some((v) => v === undefined)) {
    reject("COMMIT_METADATA_INVALID", "An outgoing commit has invalid metadata.");
  }
  return { authorName, authorEmail, committerName, committerEmail, message };
}

export function outgoingCommits(input, remoteName, execute = run) {
  const commits = new Set();
  for (const line of input.split(/\r?\n/u)) {
    if (!line.trim()) continue;
    const fields = line.trim().split(/\s+/u);
    if (fields.length !== 4) {
      reject("PUSH_UPDATE_INVALID", "Git supplied an invalid push update.");
    }
    const [, localObjectId, , remoteObjectId] = fields;
    if (zeroObjectId.test(localObjectId)) continue;
    const args = ["rev-list", localObjectId];
    if (zeroObjectId.test(remoteObjectId)) {
      args.push("--not", `--remotes=${remoteName}`);
    } else {
      args.push(`^${remoteObjectId}`);
    }
    const result = execute("git", args, {
      code: "OUTGOING_RANGE_UNAVAILABLE",
      message: "The complete outgoing commit range could not be inspected.",
    });
    for (const commit of result.split(/\r?\n/u).filter(Boolean)) commits.add(commit);
  }
  return [...commits];
}

function verifyLocalIdentityAndPolicy() {
  const identity = currentGitHubIdentity();
  assertConfiguredIdentity(identity);
  reportPolicyNotices();
  return identity;
}

function install() {
  const identity = currentGitHubIdentity();
  run("git", ["config", "--local", "user.name", identity.login]);
  run("git", ["config", "--local", "user.email", canonicalGitHubEmail(identity)]);
  run("git", ["config", "--local", "core.hooksPath", ".githooks"]);
  for (const relativePath of managedPolicyPaths) {
    if (relativePath.startsWith(".githooks/")) {
      chmodSync(path.join(repoRoot, relativePath), 0o755);
    }
  }
  assertConfiguredIdentity(identity);
  // Nothing is recorded for later comparison: the policy is read from the committed
  // revision on every run, so there is no per-checkout state to refresh.
  reportPolicyNotices();
  process.stdout.write("identity_policy=installed\n");
}

async function preCommit() {
  verifyLocalIdentityAndPolicy();
  assertStagedCandidate();
  const checks = stagedObjectChecks(stagedEntries());
  if (checks.status === "reject") {
    reject(checks.code, "The staged candidate is rejected by the sensitive-file policy.");
  }
  await scanBlobContents(checks.readOids, "SENSITIVE_OBJECT_STAGED");
  writeStagedReceipt();
}

function commitMessage(messagePath) {
  verifyLocalIdentityAndPolicy();
  assertStagedReceipt();
  let message;
  try {
    message = readFileSync(messagePath, "utf8");
  } catch {
    reject("COMMIT_MESSAGE_UNREADABLE", "The proposed commit message cannot be inspected.");
  }
  assertCommitMessage(message);
}

async function prePush(remoteName) {
  verifyLocalIdentityAndPolicy();
  const input = readFileSync(0, "utf8");
  const commits = outgoingCommits(input, remoteName);
  for (const commit of commits) {
    assertCommitRecord(readCommitRecord(commit));
  }
  const checks = outgoingObjectChecks(outgoingCommitEntries(commits));
  if (checks.status === "reject") {
    reject(checks.code, "The outgoing history is rejected by the sensitive-file policy.");
  }
  await scanBlobContents(checks.readOids, "SENSITIVE_OBJECT_OUTGOING");
  process.stdout.write(`identity_policy=passed commits=${commits.length}\n`);
}

async function main() {
  const [command, ...args] = process.argv.slice(2);
  switch (command) {
    case "install":
      install();
      break;
    case "verify":
      verifyLocalIdentityAndPolicy();
      process.stdout.write("identity_policy=passed\n");
      break;
    case "pre-commit":
      await preCommit();
      break;
    case "commit-msg":
      if (args.length !== 1) reject("USAGE", "commit-msg requires one message path.");
      commitMessage(args[0]);
      break;
    case "pre-push":
      if (args.length < 1) reject("USAGE", "pre-push requires a remote name.");
      await prePush(args[0]);
      break;
    default:
      reject("USAGE", "Use install, verify, pre-commit, commit-msg, or pre-push.");
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch((error) => {
    const code = error instanceof IdentityPolicyError ? error.code : "UNEXPECTED_FAILURE";
    const message =
      error instanceof IdentityPolicyError ? error.message : "The identity policy failed closed.";
    process.stderr.write(`LicoUp identity gate: ${code}: ${message}\n`);
    process.exitCode = 1;
  });
}
