#!/usr/bin/env node
// Readiness probe for the local model surface used by adapter verification.
//
// The endpoint and model are configuration, never constants: the committed
// `tools/local-llm.json` names the local Ollama surface, and every field can be
// overridden by an environment variable for a different machine. The probe
// exercises both standard chat surfaces the agent adapters use:
//
//   * OpenAI-compatible   POST <openai.baseUrl>/chat/completions
//   * Anthropic-compatible POST <anthropic.baseUrl>/v1/messages
//
// It prints one JSON document on stdout and never prints an API key.
import { readFileSync } from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath, pathToFileURL } from "node:url";

export const SCHEMA = "licoup.local-model-readiness.v1";
export const DEFAULT_CONFIG = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "../local-llm.json",
);

const ENV = {
  openai: {
    baseUrl: "LICOUP_LOCAL_LLM_OPENAI_BASE_URL",
    model: "LICOUP_LOCAL_LLM_OPENAI_MODEL",
    apiKey: "LICOUP_LOCAL_LLM_OPENAI_API_KEY",
  },
  anthropic: {
    baseUrl: "LICOUP_LOCAL_LLM_ANTHROPIC_BASE_URL",
    model: "LICOUP_LOCAL_LLM_ANTHROPIC_MODEL",
    apiKey: "LICOUP_LOCAL_LLM_ANTHROPIC_API_KEY",
  },
};

const PROMPT = "Reply with the single word: ok";

export class ConfigError extends Error {
  constructor(code) {
    super(code);
    this.code = code;
  }
}

function requiredEndpoint(document, protocol, env) {
  const section = document?.[protocol];
  if (!section || typeof section !== "object") throw new ConfigError("config_invalid");
  const read = (field) => {
    const override = env[ENV[protocol]?.[field]];
    if (typeof override === "string" && override.trim() !== "") return override.trim();
    const value = section[field];
    if (typeof value !== "string" || value.trim() === "") throw new ConfigError("config_invalid");
    return value.trim();
  };
  const baseUrl = read("baseUrl").replace(/\/+$/u, "");
  let parsed;
  try {
    parsed = new URL(baseUrl);
  } catch {
    throw new ConfigError("config_invalid");
  }
  if (!["http:", "https:"].includes(parsed.protocol) || parsed.username || parsed.password) {
    throw new ConfigError("config_invalid");
  }
  const model = read("model");
  const apiKey = (() => {
    const override = env[ENV[protocol].apiKey];
    if (typeof override === "string" && override !== "") return override;
    return typeof section.apiKey === "string" && section.apiKey !== "" ? section.apiKey : null;
  })();
  return { baseUrl, model, apiKey };
}

export function loadConfig(configPath = DEFAULT_CONFIG, env = process.env) {
  let document;
  try {
    document = JSON.parse(readFileSync(configPath, "utf8"));
  } catch {
    throw new ConfigError("config_unreadable");
  }
  if (document?.schema !== SCHEMA) throw new ConfigError("config_invalid");
  return {
    schema: SCHEMA,
    openai: requiredEndpoint(document, "openai", env),
    anthropic: requiredEndpoint(document, "anthropic", env),
  };
}

function classify(error) {
  if (error?.name === "AbortError") return "timeout";
  if (error instanceof ConfigError) return error.code;
  return "endpoint_unreachable";
}

function assistantText(body) {
  const choice = body?.choices?.[0]?.message?.content;
  if (typeof choice === "string" && choice.trim() !== "") return choice.trim();
  if (Array.isArray(choice)) {
    const text = choice
      .filter((part) => part?.type === "text" && typeof part.text === "string")
      .map((part) => part.text)
      .join("")
      .trim();
    if (text !== "") return text;
  }
  const anthropic = Array.isArray(body?.content)
    ? body.content
        .filter((part) => part?.type === "text" && typeof part.text === "string")
        .map((part) => part.text)
        .join("")
        .trim()
    : "";
  return anthropic === "" ? null : anthropic;
}

async function postJson(url, headers, payload, timeoutMs) {
  const response = await fetch(url, {
    method: "POST",
    headers: { "content-type": "application/json", ...headers },
    body: JSON.stringify(payload),
    signal: AbortSignal.timeout(timeoutMs),
  });
  const text = await response.text();
  let body = null;
  try {
    body = JSON.parse(text);
  } catch {
    body = null;
  }
  return { status: response.status, body, text };
}

export async function probeOpenAi(endpoint, timeoutMs = 120_000) {
  const started = Date.now();
  try {
    const headers = endpoint.apiKey ? { authorization: `Bearer ${endpoint.apiKey}` } : {};
    const { status, body } = await postJson(
      `${endpoint.baseUrl}/chat/completions`,
      headers,
      {
        model: endpoint.model,
        messages: [{ role: "user", content: PROMPT }],
        stream: false,
        max_tokens: 32,
        think: false,
      },
      timeoutMs,
    );
    if (status < 200 || status >= 300) return { status: "failed", reasonCode: `http_${status}`, latencyMs: Date.now() - started };
    const reply = assistantText(body);
    if (reply === null) return { status: "failed", reasonCode: "invalid_response", latencyMs: Date.now() - started };
    return { status: "passed", reasonCode: null, latencyMs: Date.now() - started, reply };
  } catch (error) {
    return { status: "failed", reasonCode: classify(error), latencyMs: Date.now() - started };
  }
}

export async function probeAnthropic(endpoint, timeoutMs = 120_000) {
  const started = Date.now();
  try {
    const { status, body } = await postJson(
      `${endpoint.baseUrl}/v1/messages`,
      { "anthropic-version": "2023-06-01", "x-api-key": endpoint.apiKey || "local-model" },
      {
        model: endpoint.model,
        max_tokens: 32,
        messages: [{ role: "user", content: PROMPT }],
      },
      timeoutMs,
    );
    if (status < 200 || status >= 300) return { status: "failed", reasonCode: `http_${status}`, latencyMs: Date.now() - started };
    const reply = assistantText(body);
    if (reply === null) return { status: "failed", reasonCode: "invalid_response", latencyMs: Date.now() - started };
    return { status: "passed", reasonCode: null, latencyMs: Date.now() - started, reply };
  } catch (error) {
    return { status: "failed", reasonCode: classify(error), latencyMs: Date.now() - started };
  }
}

function parseArgs(argv) {
  const options = { config: DEFAULT_CONFIG, timeoutMs: 120_000 };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--config" || arg === "--timeout-ms") {
      const value = argv[++index];
      if (!value) throw new ConfigError("argument_missing");
      if (arg === "--config") options.config = path.resolve(value);
      else {
        const timeout = Number(value);
        if (!Number.isFinite(timeout) || timeout <= 0) throw new ConfigError("argument_invalid");
        options.timeoutMs = timeout;
      }
    } else {
      throw new ConfigError("argument_unsupported");
    }
  }
  return options;
}

export async function run(argv = process.argv.slice(2), env = process.env) {
  let options;
  let config;
  try {
    options = parseArgs(argv);
    config = loadConfig(options.config, env);
  } catch (error) {
    return {
      schema: SCHEMA,
      status: "failed",
      reasonCode: classify(error),
      endpoints: [],
    };
  }
  const [openai, anthropic] = await Promise.all([
    probeOpenAi(config.openai, options.timeoutMs),
    probeAnthropic(config.anthropic, options.timeoutMs),
  ]);
  const endpoints = [
    { protocol: "openai-chat-completions", baseUrl: config.openai.baseUrl, model: config.openai.model, ...openai },
    { protocol: "anthropic-messages", baseUrl: config.anthropic.baseUrl, model: config.anthropic.model, ...anthropic },
  ];
  const failed = endpoints.find((endpoint) => endpoint.status !== "passed");
  return {
    schema: SCHEMA,
    status: failed ? "failed" : "passed",
    reasonCode: failed?.reasonCode || null,
    endpoints,
  };
}

const invoked = process.argv[1] ? pathToFileURL(path.resolve(process.argv[1])).href : "";
if (invoked === import.meta.url) {
  const report = await run();
  process.stdout.write(`${JSON.stringify(report)}\n`);
  process.exitCode = report.status === "passed" ? 0 : 1;
}
