import fs from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

export const repoRoot = path.resolve(fileURLToPath(new URL("../../..", import.meta.url)));

export async function readText(relativePath) {
  return fs.readFile(path.join(repoRoot, relativePath), "utf8");
}

export async function readJsonIfPresent(relativePath) {
  try {
    return JSON.parse(await readText(relativePath));
  } catch {
    return null;
  }
}
