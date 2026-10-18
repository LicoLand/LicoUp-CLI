#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import process from "node:process";

const offline = process.argv.slice(2).includes("--offline");

function npmAuditEnv() {
  const env = { ...process.env };
  delete env.npm_config_allow_scripts;
  delete env.NPM_CONFIG_ALLOW_SCRIPTS;
  if (offline) env.npm_config_offline = "true";
  return env;
}

function run(command, args, env = process.env) {
  const result = spawnSync(command, args, {
    cwd: process.cwd(),
    env,
    shell: process.platform === "win32",
    stdio: "inherit",
  });
  if (result.status !== 0) process.exit(result.status ?? 1);
}

run("npm", ["audit", "--omit=dev"], npmAuditEnv());
run("node", ["tools/scripts/client-rustsec-exceptions.mjs"]);
run("cargo", [
  "audit",
  "--file",
  "Cargo.lock",
  ...(offline ? ["--no-fetch"] : []),
  "--deny",
  "warnings",
]);
