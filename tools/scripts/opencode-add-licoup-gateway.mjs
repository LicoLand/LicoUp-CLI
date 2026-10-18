#!/usr/bin/env node
/**
 * One-click: add a LicoUp LLM Gateway provider sidecar for OpenCode.
 * Never rewrites opencode.json / opencode.jsonc — writes opencode.licoup-gateway.json only.
 *
 * Usage:
 *   npm run client:opencode:add-gateway
 *   node tools/scripts/opencode-add-licoup-gateway.mjs [--port 15722] [--config-root <abs>]
 *   node tools/scripts/opencode-add-licoup-gateway.mjs --self-test
 */
import { spawnSync } from "node:child_process";
import {
  existsSync,
  mkdtempSync,
  mkdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const repoRoot = resolve(fileURLToPath(new URL("../..", import.meta.url)));
const SIDECAR_NAME = "opencode.licoup-gateway.json";

function parseArgs(argv) {
  const out = { port: "15722", configRoot: "", selfTest: false, help: false };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === "--self-test") out.selfTest = true;
    else if (arg === "--help" || arg === "-h") out.help = true;
    else if (arg === "--port" && argv[i + 1]) {
      out.port = argv[++i];
    } else if (arg === "--config-root" && argv[i + 1]) {
      out.configRoot = resolve(argv[++i]);
    } else {
      throw new Error(`unknown_argument:${arg}`);
    }
  }
  return out;
}

function defaultOpenCodeConfigRoot() {
  if (process.platform === "win32") {
    const appData = process.env.APPDATA;
    if (!appData) throw new Error("opencode_config_root_unavailable");
    return join(appData, "opencode");
  }
  const xdg = process.env.XDG_CONFIG_HOME;
  if (xdg) return join(xdg, "opencode");
  const home = process.env.HOME;
  if (!home) throw new Error("opencode_config_root_unavailable");
  return join(home, ".config", "opencode");
}

function resolveLicoupCli() {
  const candidates = [
    process.env.LICO_CLIENT_PATH,
    join(repoRoot, "build", "crates", "licoup-native", "target", "debug", "licoup-cli"),
    join(repoRoot, "crates", "licoup-native", "target", "debug", "licoup-cli"),
    join(repoRoot, "target", "debug", "licoup-cli"),
    join(repoRoot, "build", "crates", "licoup-native", "target", "release", "licoup-cli"),
    join(repoRoot, "target", "release", "licoup-cli"),
    join(
      repoRoot,
      "build",
      "apps",
      "desktop",
      "runnable",
      "macos",
      "release",
      "LicoUp.app",
      "Contents",
      "MacOS",
      "licoup-cli",
    ),
  ].filter(Boolean);
  return candidates.find((path) => existsSync(path)) || "";
}

function buildLicoupCli() {
  const build = spawnSync(
    process.execPath,
    [
      join(repoRoot, "tools/scripts/cargo-client.mjs"),
      "build",
      "--manifest-path",
      "crates/licoup-native/Cargo.toml",
      "--bin",
      "licoup-cli",
    ],
    { cwd: repoRoot, encoding: "utf8", stdio: "inherit" },
  );
  if (build.status !== 0) {
    throw new Error("licoup_cli_build_failed");
  }
  const cli = resolveLicoupCli();
  if (!cli) throw new Error("licoup_cli_missing");
  return cli;
}

function cliSupportsOpenCodeGateway(cli) {
  const result = spawnSync(
    cli,
    [
      "llm-gateway",
      "agent-config",
      "plan",
      "opencode",
      "/synthetic/opencode-config-root",
      "--port",
      "15722",
    ],
    { encoding: "utf8", maxBuffer: 1024 * 1024 },
  );
  const text = `${result.stdout || ""}\n${result.stderr || ""}`;
  if (text.includes("llm_gateway_agent_adapter_unsupported")) return false;
  // Plan may still fail on path checks; unsupported is the only hard reject here.
  return result.status === 0 || text.includes("confirmationDigest") || text.includes("opencode.licoup-gateway.json");
}

function ensureLicoupCli({ forceBuild = false } = {}) {
  if (forceBuild) return buildLicoupCli();
  let cli = resolveLicoupCli();
  if (cli && cliSupportsOpenCodeGateway(cli)) return cli;
  return buildLicoupCli();
}

function runCliJson(cli, args, input) {
  const result = spawnSync(cli, args, {
    encoding: "utf8",
    maxBuffer: 4 * 1024 * 1024,
    stdio: [input === undefined ? "ignore" : "pipe", "pipe", "pipe"],
    input,
  });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    const detail = String(result.stderr || result.stdout || "").trim();
    throw new Error(detail || `licoup_cli_failed:${args.join(" ")}`);
  }
  const text = String(result.stdout || "").trim();
  return JSON.parse(text);
}

function applyGatewaySidecar({ cli, configRoot, port, allowEmpty = false }) {
  mkdirSync(configRoot, { recursive: true, mode: 0o700 });
  const plan = runCliJson(cli, [
    "llm-gateway",
    "agent-config",
    "plan",
    "opencode",
    configRoot,
    "--port",
    String(port),
  ]);
  if (!plan?.confirmationDigest || !plan?.destination) {
    throw new Error("llm_gateway_agent_config_plan_invalid");
  }
  const destination = String(plan.destination);
  if (!destination.endsWith(SIDECAR_NAME)) {
    throw new Error("llm_gateway_agent_config_destination_unexpected");
  }
  // Refuse to touch the user's primary OpenCode config files.
  for (const name of ["opencode.json", "opencode.jsonc"]) {
    if (destination.endsWith(name)) {
      throw new Error("llm_gateway_agent_config_refuses_primary_opencode_config");
    }
  }
  const plannedProvider = JSON.parse(plan.content)?.provider?.["licoup-gateway"];
  const models = Object.entries(plannedProvider?.models || {}).map(([id, value]) => ({
    id,
    name: String(value?.name || id),
  }));
  if (models.length === 0 && !allowEmpty) {
    throw new Error("llm_gateway_model_catalog_unavailable");
  }
  const applied = runCliJson(cli, [
    "llm-gateway",
    "agent-config",
    "apply",
    "opencode",
    configRoot,
    "--port",
    String(port),
    "--confirmation",
    plan.confirmationDigest,
    "--confirmed",
    "--stdin-json",
    "true",
  ], JSON.stringify({ models }));
  const body = readFileSync(destination, "utf8");
  const parsed = JSON.parse(body);
  if (!parsed?.provider?.["licoup-gateway"]?.options?.baseURL) {
    throw new Error("llm_gateway_opencode_sidecar_invalid");
  }
  return { plan, applied, destination, body };
}

function printUsage() {
  process.stdout.write(`Add a LicoUp LLM Gateway OpenCode provider sidecar (does not overwrite opencode.json/jsonc).

Usage:
  npm run client:opencode:add-gateway
  node tools/scripts/opencode-add-licoup-gateway.mjs [--port 15722] [--config-root <abs>]
  node tools/scripts/opencode-add-licoup-gateway.mjs --self-test

After apply, point OpenCode at the sidecar so it merges with your existing config:
  export OPENCODE_CONFIG=<sidecar-path>
`);
}

function selfTest() {
  const cli = ensureLicoupCli({ forceBuild: true });
  const root = mkdtempSync(join(tmpdir(), "licoup-opencode-gateway-"));
  try {
    // Existing primary config must remain untouched.
    const primary = join(root, "opencode.jsonc");
    writeFileSync(primary, '{\n  "model": "keep-me"\n}\n', "utf8");
    const { destination, body } = applyGatewaySidecar({
      cli,
      configRoot: root,
      port: "1",
      allowEmpty: true,
    });
    if (!existsSync(destination)) throw new Error("sidecar_missing");
    if (!body.includes("127.0.0.1:1/v1")) throw new Error("sidecar_base_url");
    if (!body.includes("@ai-sdk/openai-compatible")) throw new Error("sidecar_npm");
    const primaryAfter = readFileSync(primary, "utf8");
    if (!primaryAfter.includes("keep-me")) throw new Error("primary_overwritten");
    process.stdout.write("opencode-add-licoup-gateway self-test: ok\n");
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
}

function main() {
  const args = parseArgs(process.argv.slice(2));
  if (args.help) {
    printUsage();
    return;
  }
  if (args.selfTest) {
    selfTest();
    return;
  }
  const port = Number(args.port);
  if (!Number.isInteger(port) || port <= 0 || port > 65535) {
    throw new Error("llm_gateway_port_invalid");
  }
  const configRoot = args.configRoot || defaultOpenCodeConfigRoot();
  const cli = ensureLicoupCli();
  const { destination } = applyGatewaySidecar({
    cli,
    configRoot,
    port: String(port),
  });
  process.stdout.write(`Wrote OpenCode Gateway sidecar (left opencode.json/jsonc untouched):\n  ${destination}\n\n`);
  process.stdout.write("Enable it by merging via OPENCODE_CONFIG (session):\n");
  process.stdout.write(`  export OPENCODE_CONFIG=${JSON.stringify(destination)}\n\n`);
  process.stdout.write("Then start OpenCode and pick provider licoup-gateway / a listed model.\n");
  process.stdout.write("Ensure LicoUp LLM Gateway is running first (default http://127.0.0.1:15722).\n");
}

try {
  main();
} catch (error) {
  const message = error instanceof Error ? error.message : String(error);
  process.stderr.write(`${message}\n`);
  process.exitCode = 1;
}
