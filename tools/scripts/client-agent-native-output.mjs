#!/usr/bin/env node

import { readFileSync, readdirSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");

// Surfaces that dispatch, admit, translate or settle a native Agent turn.
const guardedSurfaces = [
  "crates/licoup-native/src/domain/assistant_continuity",
  "crates/licoup-native/src/domain/client_conversation",
  "crates/licoup-native/src/platform",
  "crates/licoup-conversation/src/continuity",
  "apps/desktop/lib/src/platform",
  "apps/desktop/lib/src/protocol",
];

const scanExtensions = [".rs", ".dart", ".mjs", ".ts"];

// A reply-format imposition names one of these keys on a native Agent turn.
// Quoted matches only: reading an embedded object stays allowed.
const forbiddenKeys = [
  "outputSchema",
  "output_schema",
  "responseFormat",
  "response_format",
  "responseSchema",
  "response_schema",
  "textFormat",
  "text_format",
  "jsonSchema",
  "json_schema",
  "json_object",
];

// The relay boundary is stated once per level. Keep every statement present so
// removing the rule fails the source gate instead of passing review silently.
const requiredStatements = [
  ["PRODUCT.md", "never asks the agent for a format"],
  ["PRODUCT.zh-CN.md", "绝不要求格式"],
  ["CONTRIBUTING.md", "never ask an agent to answer in a format"],
  ["CONTRIBUTING.zh-CN.md", "绝不要求 agent 按 licoup 定义的格式回答"],
  [
    "docs/architecture/AGENT-ADAPTERS-ARCHITECTURE.md",
    "no imposed reply format",
  ],
  [
    "docs/architecture/AGENT-ADAPTERS-ARCHITECTURE.zh-CN.md",
    "禁止强制回复格式",
  ],
  ["docs/architecture/CONTINUOUS-ASSISTANT.md", "as the agent produced it"],
  [
    "docs/architecture/CONTINUOUS-ASSISTANT.zh-CN.md",
    "只按 agent 原本产出的样子显示它的回复",
  ],
];

function fail(code) {
  throw new Error(code);
}

function normalize(text) {
  return text.replace(/\s+/gu, " ").toLowerCase();
}

function collectFiles(directory) {
  const found = [];
  for (const entry of readdirSync(join(root, directory), {
    withFileTypes: true,
  })) {
    const path = `${directory}/${entry.name}`;
    if (entry.isDirectory()) {
      found.push(...collectFiles(path));
    } else if (scanExtensions.some((extension) => entry.name.endsWith(extension))) {
      found.push(path);
    }
  }
  return found;
}

function scanImposedFormats() {
  const violations = [];
  for (const directory of guardedSurfaces) {
    for (const path of collectFiles(directory)) {
      const source = readFileSync(join(root, path), "utf8");
      const lines = source.split(/\r?\n/u);
      for (const [index, line] of lines.entries()) {
        for (const key of forbiddenKeys) {
          if (!line.includes(`"${key}"`) && !line.includes(`'${key}'`)) continue;
          if (line.trimStart().startsWith("//") || line.trimStart().startsWith("*")) continue;
          violations.push({ path, line: index + 1, key });
        }
      }
    }
  }
  return violations;
}

function scanRequiredStatements() {
  const missing = [];
  for (const [path, phrase] of requiredStatements) {
    const source = normalize(readFileSync(join(root, path), "utf8"));
    if (!source.includes(normalize(phrase))) missing.push(path);
  }
  return missing;
}

try {
  const violations = scanImposedFormats();
  if (violations.length > 0) {
    const first = violations[0];
    process.stdout.write(`${JSON.stringify({
      schemaVersion: "licoup.agent-native-output.receipt.v1",
      ok: false,
      errorCode: "agent_native_output_format_imposed",
      path: relative(root, join(root, first.path)),
      line: first.line,
      key: first.key,
      violationCount: violations.length,
    })}\n`);
    process.exitCode = 1;
  } else {
    const missing = scanRequiredStatements();
    if (missing.length > 0) {
      process.stdout.write(`${JSON.stringify({
        schemaVersion: "licoup.agent-native-output.receipt.v1",
        ok: false,
        errorCode: "agent_native_output_boundary_statement_missing",
        path: missing[0],
        missingCount: missing.length,
      })}\n`);
      process.exitCode = 1;
    } else {
      process.stdout.write(`${JSON.stringify({
        schemaVersion: "licoup.agent-native-output.receipt.v1",
        ok: true,
        surfaces: guardedSurfaces.length,
        forbiddenKeys: forbiddenKeys.length,
        boundaryStatements: requiredStatements.length,
        imposedReplyFormats: 0,
      })}\n`);
    }
  }
} catch (error) {
  process.stdout.write(`${JSON.stringify({
    schemaVersion: "licoup.agent-native-output.receipt.v1",
    ok: false,
    errorCode: "agent_native_output_check_failed",
  })}\n`);
  process.exitCode = 1;
}
