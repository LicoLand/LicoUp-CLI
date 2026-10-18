import { spawn } from "node:child_process";
import { chmodSync, existsSync, lstatSync, mkdirSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { defaultMaxOutputBytes, defaultTimeoutMs, disposableProfileSeedEntries, disposableProfileSeedMaxBytes, disposableProfileSeedMaxDepth, disposableProfileSeedMaxFiles, workspaceRoot } from "./constants.mjs";
import { AcceptanceError, requireFact } from "./errors.mjs";

export function createPrivateWrapper(directory, realBinary) {
  const wrapperPath = join(directory, "acp-runtime-wrapper");
  const capturePath = join(directory, "argv-capture");
  writeFileSync(wrapperPath, [
    "#!/bin/sh",
    "{",
    "  printf '%s%s\\n' '__NO_HISTORY__=' \"${CLAUDE_CODE_SKIP_PROMPT_HISTORY-}\"",
    "  printf '%s\\n' '__INVOCATION__'",
    "  for argument in \"$@\"; do printf '%s\\n' \"$argument\"; done",
    "} >> \"$LICO_ACP_ARGV_CAPTURE\"",
    "exec \"$LICO_ACP_REAL_BINARY\" \"$@\"",
    "",
  ].join("\n"), { mode: 0o700 });
  chmodSync(wrapperPath, 0o700);
  return {
    wrapperPath,
    capturePath,
    environment: {
      ...process.env,
      LICO_ACP_ARGV_CAPTURE: capturePath,
      LICO_ACP_REAL_BINARY: realBinary,
    },
  };
}

export function copyDisposableProfileSeed(source, destination, state, depth = 0) {
  requireFact(depth <= disposableProfileSeedMaxDepth, "disposable_profile_seed_limit");
  const metadata = lstatSync(source);
  requireFact(!metadata.isSymbolicLink(), "disposable_profile_seed_symlink");
  if (metadata.isDirectory()) {
    mkdirSync(destination, { recursive: true, mode: 0o700 });
    chmodSync(destination, 0o700);
    for (const name of readdirSync(source)) {
      copyDisposableProfileSeed(
        join(source, name),
        join(destination, name),
        state,
        depth + 1,
      );
    }
    return;
  }
  requireFact(metadata.isFile(), "disposable_profile_seed_unsupported");
  requireFact(
    state.files < disposableProfileSeedMaxFiles
      && state.bytes + metadata.size <= disposableProfileSeedMaxBytes,
    "disposable_profile_seed_limit",
  );
  const contents = readFileSync(source);
  state.files += 1;
  state.bytes += contents.length;
  writeFileSync(destination, contents, { mode: 0o600 });
  chmodSync(destination, 0o600);
}

export function seedDisposableProfile(context) {
  if (!context.disposableDataRoot) return false;
  requireFact(
    dirname(context.disposableDataRoot) === context.temporaryDirectory,
    "disposable_profile_path_unsafe",
  );
  mkdirSync(context.disposableDataRoot, { recursive: true, mode: 0o700 });
  chmodSync(context.disposableDataRoot, 0o700);
  const sourceRoot = context.disposableSeedSource;
  if (!sourceRoot || !existsSync(sourceRoot)) return false;
  requireFact(
    resolve(sourceRoot) !== resolve(context.disposableDataRoot),
    "disposable_profile_seed_unsafe",
  );
  const sourceMetadata = lstatSync(sourceRoot);
  requireFact(
    sourceMetadata.isDirectory() && !sourceMetadata.isSymbolicLink(),
    "disposable_profile_seed_unsafe",
  );
  const state = { files: 0, bytes: 0 };
  for (const name of disposableProfileSeedEntries) {
    const source = join(sourceRoot, name);
    if (!existsSync(source)) continue;
    copyDisposableProfileSeed(
      source,
      join(context.disposableDataRoot, name),
      state,
    );
  }
  return state.files > 0;
}

export function scanBoundedNoFollow(root, needles, limits = {}) {
  const maxDepth = limits.maxDepth || 8;
  const maxFiles = limits.maxFiles || 256;
  const maxBytes = limits.maxBytes || 4 * 1024 * 1024;
  const normalizedNeedles = needles
    .map((value) => String(value || ""))
    .filter((value) => value.length > 0);
  const result = { complete: true, found: false, files: 0, bytes: 0 };
  if (!root || !existsSync(root)) return result;
  const visit = (path, depth) => {
    requireFact(depth <= maxDepth, "persistence_scan_limit");
    const metadata = lstatSync(path);
    requireFact(!metadata.isSymbolicLink(), "persistence_scan_symlink");
    if (metadata.isDirectory()) {
      for (const name of readdirSync(path)) visit(join(path, name), depth + 1);
      return;
    }
    requireFact(metadata.isFile(), "persistence_scan_unsupported");
    requireFact(
      result.files < maxFiles && result.bytes + metadata.size <= maxBytes,
      "persistence_scan_limit",
    );
    const contents = readFileSync(path);
    result.files += 1;
    result.bytes += contents.length;
    const text = contents.toString("utf8");
    if (normalizedNeedles.some((needle) => text.includes(needle))) result.found = true;
  };
  visit(root, 0);
  return result;
}

export function runBoundedProcess(executable, args, options = {}) {
  const timeoutMs = options.timeoutMs || defaultTimeoutMs;
  const maxOutputBytes = options.maxOutputBytes || defaultMaxOutputBytes;
  const stdinText = options.stdinText || "";
  return new Promise((resolveRun, rejectRun) => {
    let stdout = Buffer.alloc(0);
    let stdoutBytes = 0;
    let stderrBytes = 0;
    let settled = false;
    let limitExceeded = false;
    const child = spawn(executable, args, {
      cwd: options.cwd || workspaceRoot,
      env: options.environment || process.env,
      stdio: ["pipe", "pipe", "pipe"],
    });
    const finishError = (code) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      child.kill();
      rejectRun(new AcceptanceError(code));
    };
    const timer = setTimeout(() => finishError("process_timeout"), timeoutMs);
    child.once("error", () => finishError("process_start_failed"));
    child.stdout.on("data", (chunk) => {
      stdoutBytes += chunk.length;
      if (stdoutBytes > maxOutputBytes) {
        limitExceeded = true;
        child.kill();
        return;
      }
      stdout = Buffer.concat([stdout, chunk]);
    });
    child.stderr.on("data", (chunk) => {
      stderrBytes += chunk.length;
      if (stderrBytes > maxOutputBytes) {
        limitExceeded = true;
        child.kill();
      }
    });
    child.once("close", (statusCode) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      if (limitExceeded) {
        rejectRun(new AcceptanceError("process_output_limit"));
        return;
      }
      resolveRun({
        statusCode,
        stdout: stdout.toString("utf8"),
        stdoutBytes,
        stderrBytes,
      });
    });
    child.stdin.on("error", () => {});
    child.stdin.end(stdinText);
  });
}
