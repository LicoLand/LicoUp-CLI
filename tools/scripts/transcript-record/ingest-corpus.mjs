#!/usr/bin/env node
import { existsSync, mkdtempSync, mkdirSync, rmSync, unlinkSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { adapterIds, scenarioClasses } from "./shared.mjs";
import { record } from "./record-projections.mjs";

const repositoryRoot = resolve(import.meta.dirname, "../../..");
const corpusArgument = process.argv.slice(2).find((argument) => !argument.startsWith("--"));
const corpusRoot = resolve(corpusArgument || join(repositoryRoot, "tests/replay-corpus"));
const nativeBin = resolve(process.env.LICO_NATIVE_BIN || join(repositoryRoot, "build/crates/licoup-native/target/debug/licoup-cli"));
const refresh = process.argv.includes("--refresh");
const privateRoot = mkdtempSync(join(tmpdir(), "lico-history-ingest-"));
const run = (script, args) => {
  const result = spawnSync(process.execPath, [join(import.meta.dirname, script), ...args], {
    cwd: repositoryRoot,
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
  });
  if (result.status !== 0) throw new Error(`${script}_failed:${result.stderr.trim()}`);
};

try {
  for (const adapterId of adapterIds) {
    const adapterRoot = join(corpusRoot, adapterId);
    const catalogCache = join(privateRoot, `${adapterId}-catalog.json`);
    mkdirSync(adapterRoot, { recursive: true });
    for (const scenario of scenarioClasses) {
      const raw = join(privateRoot, `${adapterId}-${scenario}.json`);
      const candidate = join(adapterRoot, `${scenario}.json`);
      if (refresh && existsSync(candidate)) unlinkSync(candidate);
      if (existsSync(candidate)) continue;
      run("record.mjs", [
        "--adapter", adapterId,
        "--scenario", scenario,
        "--output", raw,
        "--native-bin", nativeBin,
        "--catalog-cache", catalogCache,
      ]);
      run("redact.mjs", [raw, candidate]);
    }
  }
  // A redacted candidate carries frames but no projections: only the real
  // adapter parsers can say what those frames mean.
  record(corpusRoot);
  process.stdout.write(`ingested ${adapterIds.length * scenarioClasses.length} redacted commit candidates; raw history removed\n`);
} finally {
  rmSync(privateRoot, { recursive: true, force: true });
}
