#!/usr/bin/env node
// Managed native performance smoke runner.
//
// Delegates benchmark execution to the canonical managed Cargo lease
// (tools/scripts/cargo-client.mjs) and records structural counters instead of
// workstation-specific latency. Smoke mode uses Criterion quick mode so the
// evidence stays bounded and deterministic.

import { spawn } from "node:child_process";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(fileURLToPath(new URL("../..", import.meta.url)));
const benchmark = "native_backend";
const manifestPath = "crates/licoup-native/Cargo.toml";
const cargoClient = path.join(repoRoot, "tools", "scripts", "cargo-client.mjs");
const SMOKE_MAX_MS = 20 * 60_000;
const FULL_MAX_MS = 60 * 60_000;
const OUTPUT_LIMIT = 1_048_576;

const args = process.argv.slice(2);
const smoke = args.includes("--smoke");
const unexpected = args.filter((argument) => argument !== "--smoke");
if (unexpected.length > 0) {
  process.stderr.write(
    `native performance runner rejects arguments: ${unexpected.join(" ")}\n`,
  );
  process.exitCode = 2;
} else {
  run();
}

function run() {
  const benchArgs = ["bench", "--manifest-path", manifestPath, "--bench", benchmark];
  if (smoke) benchArgs.push("--", "--quick");

  const startedAt = Date.now();
  let output = "";
  let finished = false;

  function settle(exitCode, note = "") {
    if (finished) return;
    finished = true;
    const durationMs = Date.now() - startedAt;
    const startedCases = new Set(
      [...output.matchAll(/Benchmarking\s+(\S+?):/g)].map((match) => match[1]),
    ).size;
    const completedCases = [...output.matchAll(/\btime:\s*\[/g)].length;
    const summary = [
      "native performance evidence:",
      `  mode: ${smoke ? "smoke" : "full"}`,
      `  benchmark: ${benchmark}`,
      `  cases_started: ${startedCases}`,
      `  cases_completed: ${completedCases}`,
      `  exit_code: ${exitCode}`,
      `  duration_ms: ${durationMs}`,
    ];
    if (note) summary.push(`  note: ${note}`);
    process.stdout.write(`${summary.join("\n")}\n`);
    process.exitCode = exitCode;
  }

  const child = spawn(process.execPath, [cargoClient, ...benchArgs], {
    cwd: repoRoot,
    stdio: ["ignore", "pipe", "pipe"],
  });

  child.stdout.on("data", (chunk) => {
    if (output.length < OUTPUT_LIMIT) output += chunk;
    process.stdout.write(chunk);
  });
  child.stderr.on("data", (chunk) => {
    if (output.length < OUTPUT_LIMIT) output += chunk;
    process.stderr.write(chunk);
  });

  const ceilingMs = smoke ? SMOKE_MAX_MS : FULL_MAX_MS;
  const ceiling = setTimeout(() => {
    child.kill("SIGKILL");
    settle(1, `exceeded bounded ceiling of ${ceilingMs} ms`);
  }, ceilingMs);
  ceiling.unref();

  child.once("error", (error) => {
    clearTimeout(ceiling);
    settle(1, `failed to start managed runner: ${error.message}`);
  });

  child.once("exit", (code, signal) => {
    clearTimeout(ceiling);
    if (signal) {
      settle(1, `managed runner terminated by ${signal}`);
      return;
    }
    settle(code ?? 1);
  });
}
