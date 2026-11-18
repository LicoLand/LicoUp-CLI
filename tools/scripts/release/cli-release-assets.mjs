#!/usr/bin/env node
// Shared release-asset naming for the CLI release and verification workflows.
// Every exported function is pure so the contract tests can plan Linux and
// Windows archives on macOS without cross-compiled binaries.
import { appendFileSync } from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath, pathToFileURL } from "node:url";

const TARGET_PATTERN = /^[a-z0-9_]+-[a-z0-9_]+-[a-z0-9_-]+$/u;
const VERSION_PATTERN = /^[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?$/u;
const ARCHIVE_FORMATS = new Set(["tar.gz", "zip"]);
const USAGE =
  "usage: cli-release-assets.mjs plan --version VERSION --target TRIPLE [--github-output FILE]";

export function isWindowsTarget(target) {
  if (typeof target !== "string" || !TARGET_PATTERN.test(target)) {
    throw new Error("cli_release_target_invalid");
  }
  return target.includes("-windows-");
}

export function archiveFormatFor(target) {
  return isWindowsTarget(target) ? "zip" : "tar.gz";
}

export function binaryNames(target) {
  const suffix = isWindowsTarget(target) ? ".exe" : "";
  return { cli: `licoup${suffix}`, mcp: `lico-subagent-mcp${suffix}` };
}

export function archiveName({ version, target, format = archiveFormatFor(target) }) {
  if (typeof version !== "string" || !VERSION_PATTERN.test(version)) {
    throw new Error("cli_release_version_invalid");
  }
  isWindowsTarget(target);
  if (!ARCHIVE_FORMATS.has(format)) throw new Error("cli_release_archive_format_invalid");
  return `licoup-${version}-${target}.${format}`;
}

export function releaseAssets({ version, target }) {
  const binaries = binaryNames(target);
  const format = archiveFormatFor(target);
  const archive = archiveName({ version, target, format });
  return {
    archive,
    checksum: `${archive}.sha256`,
    format,
    cli: binaries.cli,
    mcp: binaries.mcp,
  };
}

// GitHub's step-output file understands `key=value` lines and a trailing
// newline; keep the rendering here so the workflow never re-spells a name.
export function formatGithubOutput(assets) {
  return [
    `archive=${assets.archive}`,
    `checksum=${assets.checksum}`,
    `format=${assets.format}`,
    `cli_binary=${assets.cli}`,
    `mcp_binary=${assets.mcp}`,
  ].join("\n") + "\n";
}

function parseArgs(argv) {
  const [command, ...rest] = argv;
  const options = {};
  for (let index = 0; index < rest.length; index += 2) {
    const name = rest[index];
    const value = rest[index + 1];
    if (!name?.startsWith("--") || value === undefined) throw new Error(USAGE);
    options[name.slice(2)] = value;
  }
  return { command, options };
}

function main(argv) {
  const { command, options } = parseArgs(argv);
  if (command !== "plan" || !options.version || !options.target) throw new Error(USAGE);
  const assets = releaseAssets(options);
  if (options["github-output"]) {
    appendFileSync(options["github-output"], formatGithubOutput(assets));
    return;
  }
  process.stdout.write(`${JSON.stringify(assets)}\n`);
}

const invoked = process.argv[1] ? pathToFileURL(path.resolve(process.argv[1])).href : "";
if (invoked === import.meta.url) {
  try {
    main(process.argv.slice(2));
  } catch (error) {
    process.stderr.write(`${error?.message || "cli_release_assets_failed"}\n`);
    process.exitCode = 1;
  }
}
