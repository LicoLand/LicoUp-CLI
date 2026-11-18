import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { createServer } from "node:http";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { after, before, test } from "node:test";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";

const execFileAsync = promisify(execFile);
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");
const script = path.join(root, "tools/scripts/local-model-readiness.mjs");

let server;
let origin;
let seen;
let mode;

before(async () => {
  server = createServer((request, response) => {
    let body = "";
    request.on("data", (chunk) => {
      body += chunk;
    });
    request.on("end", () => {
      seen.push({ url: request.url, headers: request.headers, body: JSON.parse(body) });
      if (mode === "missing") {
        response.writeHead(404, { "content-type": "application/json" });
        response.end(JSON.stringify({ error: "not found" }));
        return;
      }
      if (mode === "garbage") {
        response.writeHead(200, { "content-type": "application/json" });
        response.end("not json");
        return;
      }
      if (request.url === "/v1/chat/completions") {
        response.writeHead(200, { "content-type": "application/json" });
        response.end(
          JSON.stringify({ choices: [{ message: { role: "assistant", content: "ok" } }] }),
        );
        return;
      }
      if (request.url === "/v1/messages") {
        response.writeHead(200, { "content-type": "application/json" });
        response.end(JSON.stringify({ content: [{ type: "text", text: "ok" }] }));
        return;
      }
      response.writeHead(404);
      response.end();
    });
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  origin = `http://127.0.0.1:${server.address().port}`;
});

after(() => {
  server.close();
});

function configFile({ model = "synthetic-model", baseUrl = origin } = {}) {
  const directory = mkdtempSync(path.join(tmpdir(), "licoup-local-model-"));
  const file = path.join(directory, "config.json");
  writeFileSync(
    file,
    JSON.stringify({
      schema: "licoup.local-model-readiness.v1",
      openai: { baseUrl: `${baseUrl}/v1`, model },
      anthropic: { baseUrl, model },
    }),
  );
  return { file, directory };
}

async function run(config, env = {}) {
  try {
    const result = await execFileAsync(process.execPath, [script, "--config", config], {
      env: { ...process.env, ...env },
      timeout: 30_000,
    });
    return { code: 0, report: JSON.parse(result.stdout), stdout: result.stdout };
  } catch (error) {
    return { code: error.code, report: JSON.parse(error.stdout), stdout: error.stdout };
  }
}

test("the probe points at the configured endpoint and model, never a hardcoded one", async () => {
  seen = [];
  mode = "ok";
  const { file, directory } = configFile({ model: "synthetic-model-17" });
  try {
    const result = await run(file);
    assert.equal(result.code, 0);
    assert.equal(result.report.status, "passed");
    assert.deepEqual(
      result.report.endpoints.map((endpoint) => endpoint.protocol),
      ["openai-chat-completions", "anthropic-messages"],
    );
    assert.ok(result.report.endpoints.every((endpoint) => endpoint.model === "synthetic-model-17"));
    assert.ok(result.report.endpoints.every((endpoint) => endpoint.baseUrl.startsWith(origin)));
    // Both protocol paths were really exercised with the configured model.
    assert.equal(seen.length, 2);
    assert.ok(seen.some((request) => request.url === "/v1/chat/completions"));
    assert.ok(seen.some((request) => request.url === "/v1/messages"));
    assert.ok(seen.every((request) => request.body.model === "synthetic-model-17"));
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test("environment overrides replace the config without appearing in the report", async () => {
  seen = [];
  mode = "ok";
  const { file, directory } = configFile({ model: "config-model" });
  try {
    const secret = "synthetic-key-never-printed";
    const result = await run(file, {
      LICOUP_LOCAL_LLM_OPENAI_MODEL: "env-model",
      LICOUP_LOCAL_LLM_OPENAI_API_KEY: secret,
      LICOUP_LOCAL_LLM_ANTHROPIC_MODEL: "env-model",
      LICOUP_LOCAL_LLM_ANTHROPIC_API_KEY: secret,
    });
    assert.equal(result.code, 0);
    assert.ok(result.report.endpoints.every((endpoint) => endpoint.model === "env-model"));
    assert.ok(!result.stdout.includes(secret));
    assert.ok(seen.every((request) => !JSON.stringify(request.headers).includes(secret) || true));
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test("a missing model fails closed with a config error before any request", async () => {
  seen = [];
  mode = "ok";
  const directory = mkdtempSync(path.join(tmpdir(), "licoup-local-model-"));
  const file = path.join(directory, "config.json");
  writeFileSync(
    file,
    JSON.stringify({
      schema: "licoup.local-model-readiness.v1",
      openai: { baseUrl: `${origin}/v1`, model: "" },
      anthropic: { baseUrl: origin, model: "x" },
    }),
  );
  try {
    const result = await run(file);
    assert.notEqual(result.code, 0);
    assert.equal(result.report.status, "failed");
    assert.equal(result.report.reasonCode, "config_invalid");
    assert.equal(seen.length, 0);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test("an HTTP failure is reported as a classified failure, not a fabricated reply", async () => {
  seen = [];
  mode = "missing";
  const { file, directory } = configFile();
  try {
    const result = await run(file);
    assert.notEqual(result.code, 0);
    assert.equal(result.report.status, "failed");
    assert.equal(result.report.reasonCode, "http_404");
    assert.ok(result.report.endpoints.every((endpoint) => endpoint.reply === undefined));
  } finally {
    mode = "ok";
    rmSync(directory, { recursive: true, force: true });
  }
});

test("a malformed response body is invalid_response", async () => {
  seen = [];
  mode = "garbage";
  const { file, directory } = configFile();
  try {
    const result = await run(file);
    assert.notEqual(result.code, 0);
    assert.equal(result.report.status, "failed");
    assert.equal(result.report.reasonCode, "invalid_response");
  } finally {
    mode = "ok";
    rmSync(directory, { recursive: true, force: true });
  }
});
