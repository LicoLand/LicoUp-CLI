#!/usr/bin/env node
// The CLI release contract (`contract.json`) shipped inside every release
// archive. The Desktop repository pins one release and refuses it when the
// conversation protocol digest or the command catalog does not match what it
// invokes, so both repositories compute the digest with the same function.
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdirSync, readFileSync, readdirSync, statSync, writeFileSync } from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath, pathToFileURL } from "node:url";

export const STDIO_PROTOCOL = "licoup.stdio.v1";
export const CONVERSATION_PROTOCOL_ROOT = "schemas/conversation_protocol";

const repositoryRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");

function listFiles(root, relative) {
  const absolute = path.join(root, relative);
  const files = [];
  for (const entry of readdirSync(absolute, { withFileTypes: true })) {
    const child = `${relative}/${entry.name}`;
    if (entry.isDirectory()) files.push(...listFiles(root, child));
    else if (entry.isFile()) files.push(child);
  }
  return files;
}

// SHA-256 over `path\0sha256(file)\n` for every file under the protocol tree,
// sorted by its repository-relative `/`-separated path.
export function conversationProtocolDigest(root = repositoryRoot) {
  const files = listFiles(root, CONVERSATION_PROTOCOL_ROOT).sort();
  if (files.length === 0) throw new Error("conversation_protocol_tree_empty");
  const digest = createHash("sha256");
  for (const file of files) {
    const content = createHash("sha256").update(readFileSync(path.join(root, file))).digest("hex");
    digest.update(`${file}\0${content}\n`);
  }
  return digest.digest("hex");
}

export function cliVersion(root = repositoryRoot) {
  const manifest = JSON.parse(readFileSync(path.join(root, "tools/cli-version.json"), "utf8"));
  if (typeof manifest.productVersion !== "string" || manifest.productVersion.length === 0) {
    throw new Error("cli_version_manifest_invalid");
  }
  return manifest.productVersion;
}

// The contract for one built binary: its own command catalog, never a copy.
export function buildContract({ catalog, target, version, digest }) {
  if (!catalog || !Array.isArray(catalog.commands) || catalog.commands.length === 0) {
    throw new Error("cli_command_catalog_invalid");
  }
  if (catalog.rpc?.protocol !== STDIO_PROTOCOL) throw new Error("cli_stdio_protocol_mismatch");
  if (typeof target !== "string" || !/^[a-z0-9_]+-[a-z0-9_]+-[a-z0-9_-]+$/u.test(target)) {
    throw new Error("cli_release_target_invalid");
  }
  return {
    cliVersion: version,
    target,
    stdioProtocol: STDIO_PROTOCOL,
    conversationProtocolDigest: digest,
    commands: catalog.commands,
  };
}

function parseArgs(argv) {
  const options = {};
  for (let index = 0; index < argv.length; index += 2) {
    const [name, value] = [argv[index], argv[index + 1]];
    if (!["--binary", "--target", "--output"].includes(name) || !value) {
      throw new Error("usage: cli-release-contract.mjs --binary PATH --target TRIPLE --output FILE");
    }
    options[name.slice(2)] = value;
  }
  if (!options.binary || !options.target || !options.output) {
    throw new Error("usage: cli-release-contract.mjs --binary PATH --target TRIPLE --output FILE");
  }
  return options;
}

function main(argv) {
  const options = parseArgs(argv);
  if (!statSync(options.binary).isFile()) throw new Error("cli_binary_missing");
  const catalog = JSON.parse(execFileSync(options.binary, ["commands"], {
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
  }));
  const contract = buildContract({
    catalog,
    target: options.target,
    version: cliVersion(),
    digest: conversationProtocolDigest(),
  });
  mkdirSync(path.dirname(path.resolve(options.output)), { recursive: true });
  writeFileSync(options.output, `${JSON.stringify(contract, null, 2)}\n`);
}

const invoked = process.argv[1] ? pathToFileURL(path.resolve(process.argv[1])).href : "";
if (invoked === import.meta.url) {
  try {
    main(process.argv.slice(2));
  } catch (error) {
    process.stderr.write(`${error?.message || "cli_release_contract_failed"}\n`);
    process.exitCode = 1;
  }
}
