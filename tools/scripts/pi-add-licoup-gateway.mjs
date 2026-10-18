#!/usr/bin/env node
/**
 * One-click: add LicoUp LLM Gateway to Pi Agent.
 * Writes models.licoup-gateway.json sidecar, then merges only
 * providers.licoup-gateway into models.json (never wipes other providers).
 *
 * Usage:
 *   npm run client:pi:add-gateway
 *   node tools/scripts/pi-add-licoup-gateway.mjs [--port 15722] [--config-root <abs>]
 *   node tools/scripts/pi-add-licoup-gateway.mjs --self-test
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
const SIDECAR_NAME = "models.licoup-gateway.json";
const MODELS_NAME = "models.json";

function parseArgs(argv) {
  const out = {
    port: "15722",
    configRoot: "",
    selfTest: false,
    help: false,
    skipMerge: false,
  };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === "--self-test") out.selfTest = true;
    else if (arg === "--help" || arg === "-h") out.help = true;
    else if (arg === "--skip-merge") out.skipMerge = true;
    else if (arg === "--port" && argv[i + 1]) out.port = argv[++i];
    else if (arg === "--config-root" && argv[i + 1]) out.configRoot = resolve(argv[++i]);
    else throw new Error(`unknown_argument:${arg}`);
  }
  return out;
}

function defaultPiAgentRoot() {
  const home = process.env.HOME;
  if (!home) throw new Error("pi_agent_root_unavailable");
  if (process.platform === "win32") {
    // Pi uses $HOME/.pi/agent on all platforms in first-party docs.
    return join(home, ".pi", "agent");
  }
  return join(home, ".pi", "agent");
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
  if (build.status !== 0) throw new Error("licoup_cli_build_failed");
  const cli = resolveLicoupCli();
  if (!cli) throw new Error("licoup_cli_missing");
  return cli;
}

function cliSupportsPiGateway(cli) {
  const result = spawnSync(
    cli,
    [
      "llm-gateway",
      "agent-config",
      "plan",
      "pi",
      "/synthetic/pi/agent",
      "--port",
      "15722",
    ],
    { encoding: "utf8", maxBuffer: 1024 * 1024 },
  );
  const text = `${result.stdout || ""}\n${result.stderr || ""}`;
  if (text.includes("llm_gateway_agent_adapter_unsupported")) return false;
  return (
    result.status === 0 ||
    text.includes("confirmationDigest") ||
    text.includes("models.licoup-gateway.json")
  );
}

function ensureLicoupCli({ forceBuild = false } = {}) {
  if (forceBuild) return buildLicoupCli();
  const cli = resolveLicoupCli();
  if (cli && cliSupportsPiGateway(cli)) return cli;
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
  return JSON.parse(String(result.stdout || "").trim());
}

function applySidecar({ cli, configRoot, port, allowEmpty = false }) {
  mkdirSync(configRoot, { recursive: true, mode: 0o700 });
  const plan = runCliJson(cli, [
    "llm-gateway",
    "agent-config",
    "plan",
    "pi",
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
  if (destination.endsWith(MODELS_NAME) && !destination.endsWith(SIDECAR_NAME)) {
    throw new Error("llm_gateway_agent_config_refuses_primary_pi_models");
  }
  const models = JSON.parse(plan.content)?.providers?.["licoup-gateway"]?.models || [];
  if (models.length === 0 && !allowEmpty) {
    throw new Error("llm_gateway_model_catalog_unavailable");
  }
  const applied = runCliJson(cli, [
    "llm-gateway",
    "agent-config",
    "apply",
    "pi",
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
  if (!parsed?.providers?.["licoup-gateway"]?.baseUrl) {
    throw new Error("llm_gateway_pi_sidecar_invalid");
  }
  return { plan, applied, destination, body, provider: parsed.providers["licoup-gateway"] };
}

function mergeIntoModelsJson({ configRoot, provider }) {
  const modelsPath = join(configRoot, MODELS_NAME);
  let existing = { providers: {} };
  if (existsSync(modelsPath)) {
    existing = JSON.parse(readFileSync(modelsPath, "utf8"));
    if (!existing || typeof existing !== "object" || Array.isArray(existing)) {
      throw new Error("pi_models_json_invalid");
    }
    if (!existing.providers || typeof existing.providers !== "object") {
      existing.providers = {};
    }
  }
  const before = JSON.stringify(existing.providers["licoup-gateway"] || null);
  existing.providers["licoup-gateway"] = provider;
  const after = JSON.stringify(existing.providers["licoup-gateway"]);
  const next = `${JSON.stringify(existing, null, 2)}\n`;
  writeFileSync(modelsPath, next, { mode: 0o600 });
  return {
    modelsPath,
    changed: before !== after,
    providerIds: Object.keys(existing.providers),
  };
}

function printUsage() {
  process.stdout.write(`Add LicoUp LLM Gateway to Pi Agent (sidecar + merge into models.json).

Usage:
  npm run client:pi:add-gateway
  node tools/scripts/pi-add-licoup-gateway.mjs [--port 15722] [--config-root <abs>]
  node tools/scripts/pi-add-licoup-gateway.mjs --self-test
`);
}

function selfTest() {
  const cli = ensureLicoupCli({ forceBuild: true });
  const root = mkdtempSync(join(tmpdir(), "licoup-pi-gateway-"));
  try {
    const keepPath = join(root, MODELS_NAME);
    writeFileSync(
      keepPath,
      `${JSON.stringify(
        {
          providers: {
            keepme: {
              baseUrl: "http://127.0.0.1:9/v1",
              api: "openai-completions",
              apiKey: "x",
              models: [{ id: "keep" }],
            },
          },
        },
        null,
        2,
      )}\n`,
      "utf8",
    );
    const { provider } = applySidecar({
      cli,
      configRoot: root,
      port: "1",
      allowEmpty: true,
    });
    const merged = mergeIntoModelsJson({ configRoot: root, provider });
    const after = JSON.parse(readFileSync(keepPath, "utf8"));
    if (!after.providers.keepme) throw new Error("existing_provider_wiped");
    if (!after.providers["licoup-gateway"]?.baseUrl?.includes("127.0.0.1:1")) {
      throw new Error("gateway_provider_missing");
    }
    if (!merged.providerIds.includes("keepme")) throw new Error("merge_ids");
    process.stdout.write("pi-add-licoup-gateway self-test: ok\n");
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
  const configRoot = args.configRoot || defaultPiAgentRoot();
  const cli = ensureLicoupCli();
  const { destination, provider } = applySidecar({
    cli,
    configRoot,
    port: String(port),
  });
  process.stdout.write(`Wrote Pi Gateway sidecar:\n  ${destination}\n`);
  if (!args.skipMerge) {
    const merged = mergeIntoModelsJson({ configRoot, provider });
    process.stdout.write(
      `Merged providers.licoup-gateway into models.json (${merged.changed ? "updated" : "unchanged"}):\n  ${merged.modelsPath}\n`,
    );
    process.stdout.write(`Provider ids now: ${merged.providerIds.join(", ")}\n`);
  }
  process.stdout.write(
    "Pick provider licoup-gateway in Pi (/model). Ensure LicoUp LLM Gateway is running (default http://127.0.0.1:15722).\n",
  );
}

try {
  main();
} catch (error) {
  const message = error instanceof Error ? error.message : String(error);
  process.stderr.write(`${message}\n`);
  process.exitCode = 1;
}
