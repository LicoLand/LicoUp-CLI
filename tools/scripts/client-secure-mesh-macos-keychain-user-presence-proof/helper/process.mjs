import process from "node:process";
import { repoRoot } from "../constants.mjs";

export function commandOptions(timeout) {
  return {
    cwd: repoRoot,
    env: process.env,
    encoding: "utf8",
    maxBuffer: 16 * 1024 * 1024,
    timeout,
  };
}
