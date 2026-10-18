#!/usr/bin/env node
import { spawnSync } from "node:child_process";
import { existsSync, statSync } from "node:fs";
import { resolve } from "node:path";

const repositoryRoot = resolve(import.meta.dirname, "../../..");
const corpusRoot = resolve(repositoryRoot, "tests/replay-corpus");
const syntheticRoot = resolve(repositoryRoot, "apps/desktop/test/fixtures/adapter-replay");
const directoryExists = (path) => existsSync(path) && statSync(path).isDirectory();

let activeFixtureRoot = null;
let source = null;
if (directoryExists(corpusRoot)) {
  activeFixtureRoot = corpusRoot;
  source = "developer-corpus";
} else if (directoryExists(syntheticRoot)) {
  activeFixtureRoot = syntheticRoot;
  source = "synthetic-fixtures";
}

if (!activeFixtureRoot) {
  process.stderr.write(
    `${JSON.stringify({
      ok: false,
      executed: false,
      status: "not_executed",
      reason: "neither replay corpus nor synthetic fixtures available; missing replay transcript cannot be reported as passed",
    })}\n`,
  );
  process.exit(1);
}

const commands = [
  [process.execPath, [resolve(import.meta.dirname, "verify-redaction.mjs"), activeFixtureRoot]],
  [process.execPath, [resolve(import.meta.dirname, "self-test.mjs")]],
];
for (const [command, args] of commands) {
  const result = spawnSync(command, args, { cwd: repositoryRoot, stdio: "inherit" });
  if (result.status !== 0) process.exit(result.status || 1);
}

// The parser replay runs as a crate unit test. Its output is captured because a
// filter that matches nothing still exits zero, and an empty replay must never
// be reported as a passed one.
const replay = spawnSync(
  process.execPath,
  [
    resolve(repositoryRoot, "tools/scripts/cargo-client.mjs"),
    "test",
    "--manifest-path",
    "crates/licoup-native/Cargo.toml",
    "--lib",
    "--",
    "platform::native_agent_parser::replay::",
  ],
  { cwd: repositoryRoot, encoding: "utf8" },
);
process.stdout.write(replay.stdout || "");
process.stderr.write(replay.stderr || "");
if (replay.status !== 0) process.exit(replay.status || 1);
const executed = replay.stdout?.match(/test result: ok\. (\d+) passed/) || replay.stderr?.match(/test result: ok\. (\d+) passed/);
if (!executed || Number(executed[1]) === 0) {
  process.stderr.write(
    `${JSON.stringify({
      ok: false,
      executed: false,
      status: "not_executed",
      reason: "no adapter replay test ran; an empty replay is not a passed replay",
    })}\n`,
  );
  process.exit(1);
}

process.stdout.write(
  `${JSON.stringify({ ok: true, executed: true, status: "passed", source })}\n`,
);
