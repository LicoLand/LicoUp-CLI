#!/usr/bin/env node
// Evidence attachment to the existing canonical runner, not another test runner.
// It never changes the selected command, exit code, lease or verification gates.
import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { main } from "../../../tools/scripts/client-module-regression.mjs";
import { executeClientModules, runClientRegressionCommand } from "../../../tools/regression/client-module-execution.mjs";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");
const outputRoot = path.join(root, "build/reports");
if (!fs.statSync(outputRoot).isDirectory()) throw new Error("existing build/reports required");
const prefix = "v71-x2-g2";
const report = `build/reports/${prefix}-focused.json`;
const digest = (bytes) => createHash("sha256").update(bytes).digest("hex");
const extensions = new Set([".rs", ".toml", ".json", ".mjs", ".py", ".c", ".sh", ".lock"]);
const directories = [
  "crates", // build dependencies as well as the candidate, so concurrent Rust changes are visible
  "tests/integration/extension_isolation",
  "sdk/agent-adapter/python", "sdk/agent-adapter/samples/minimal-specialist",
  "tools/regression",
];
const individual = [
  "Cargo.toml", "Cargo.lock", "rust-toolchain.toml", ".cargo/config.toml",
  "tools/scripts/client-module-regression.mjs",
  "apps/desktop/scripts/client-architecture/checks/native/crate-core-and-facade-bounds.mjs",
  "tests/contract/client/client-architecture-modules.test.mjs",
];
function snapshot() {
  const files = new Map();
  function visit(relative) {
    const full = path.join(root, relative);
    const stat = fs.lstatSync(full);
    if (stat.isSymbolicLink()) throw new Error(`source symlink needs explicit review: ${relative}`);
    if (stat.isDirectory()) {
      for (const entry of fs.readdirSync(full).sort()) {
        if (["target", "build", ".git", "__pycache__", "node_modules"].includes(entry)) continue;
        visit(`${relative}/${entry}`);
      }
    } else if (extensions.has(path.extname(relative))) {
      files.set(relative, digest(fs.readFileSync(full)));
    }
  }
  for (const relative of directories) visit(relative);
  for (const relative of individual) if (fs.existsSync(path.join(root, relative))) visit(relative);
  const hashes = Object.fromEntries([...files].sort(([a], [b]) => a.localeCompare(b)));
  return { sha256: digest(JSON.stringify(hashes)), files: hashes };
}
function safeText(text) {
  for (const [value, replacement] of [
    [root, "$WORKSPACE"], [os.tmpdir(), "$TMP"],
    [`/private${os.tmpdir()}`, "$TMP"], [os.homedir(), "$HOME"],
  ].sort(([a], [b]) => b.length - a.length)) text = text.split(value).join(replacement);
  return text;
}
function write(name, text) {
  fs.writeFileSync(path.join(outputRoot, name), text, { mode: 0o600 });
  return { path: `build/reports/${name}`, sha256: digest(text) };
}
const before = snapshot();
const started = new Date().toISOString();
const invocations = [];
const exitCode = await main(["--module", "rust.platform.extension-isolation", "--report", report], {
  executor: (selected, options) => executeClientModules(selected, {
    ...options,
    commandRunner: (batch) => runClientRegressionCommand(batch, {
      repoRoot: root,
      environment: { ...process.env, RUST_TEST_NOCAPTURE: "1" },
      spawnImpl: (program, args, options) => {
        const index = invocations.length;
        const entry = { command: safeText([program, ...args].join(" ")), stdout: [], stderr: [], exit: null, signal: null };
        invocations.push(entry);
        const child = spawn(program, args, options);
        child.stdout.on("data", (chunk) => entry.stdout.push(chunk));
        child.stderr.on("data", (chunk) => entry.stderr.push(chunk));
        child.on("close", (code, signal) => {
          entry.exit = code;
          entry.signal = signal;
          entry.stdoutText = safeText(Buffer.concat(entry.stdout).toString("utf8"));
          entry.stderrText = safeText(Buffer.concat(entry.stderr).toString("utf8"));
          entry.stdoutRef = write(`${prefix}-${index}.stdout.log`, entry.stdoutText);
          entry.stderrRef = write(`${prefix}-${index}.stderr.log`, entry.stderrText);
        });
        return child;
      },
    }),
  }),
});
const after = snapshot();
const text = invocations.map((entry) => `${entry.stdoutText ?? ""}\n${entry.stderrText ?? ""}`).join("\n");
const branches = text.split("\n").filter((line) => line.includes("X2_PROBE") || line.includes("X2_RUNTIME"));
const expected = [
  "creation trusted-local fork-setsid created 0 0", "creation restricted fork-setsid denied 1 -1",
  "creation trusted-local fork-setpgid created 0 0", "creation restricted fork-setpgid denied 1 -1",
  "creation trusted-local posix_spawn created 0 0", "creation restricted posix_spawn denied 1 -1",
  "creation trusted-local vfork-exec created 0 0", "creation restricted vfork-exec denied 1 -1",
  'interpreter trusted-local {"exit":0,"outcome":"created","route":"os.posix_spawn"}',
  'interpreter restricted {"errno":1,"outcome":"denied","route":"os.posix_spawn"}',
  'interpreter trusted-local {"exit":0,"outcome":"created","route":"subprocess"}',
  'interpreter restricted {"errno":1,"outcome":"denied","route":"subprocess"}',
  "closed-stdio route=setsid", "closed-stdio route=setpgid",
  "runtime-missing acceptance=failed-as-required", "X2_RUNTIME python=executed",
];
const missing = expected.filter((marker) => !text.includes(marker));
const matchedCandidate = before.sha256 === after.sha256;
const verdict = exitCode === 0 && matchedCandidate && missing.length === 0;
const evidence = {
  schema: "licoup.x2.focused-evidence.v1", startedAt: started, completedAt: new Date().toISOString(),
  status: verdict ? "passed" : "not-proven", canonicalExitCode: exitCode,
  canonicalReport: { path: report, sha256: fs.existsSync(path.join(root, report)) ? digest(fs.readFileSync(path.join(root, report))) : null },
  candidateBefore: before, candidateAfter: after, matchedCandidate, missingBranchEvidence: missing,
  branches,
  invocations: invocations.map((entry) => ({ command: entry.command, exit: entry.exit, signal: entry.signal, stdout: entry.stdoutRef, stderr: entry.stderrRef })),
  limits: ["Only the listed source inputs are fingerprinted; no mtime-based identity claim.", "Fixture cleanup is not host reclamation or authority to clear unknown predecessors.", "Logs retain all child output with local path prefixes normalized; no environment or claim token is stored."],
};
write(`${prefix}-evidence.json`, `${JSON.stringify(evidence, null, 2)}\n`);
console.log(`isolation evidence=${evidence.status} candidateStable=${matchedCandidate} missingBranches=${missing.length}`);
process.exitCode = verdict ? 0 : (exitCode || 1);
