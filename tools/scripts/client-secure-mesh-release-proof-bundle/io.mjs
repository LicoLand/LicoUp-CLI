import { createHash } from "node:crypto";
import fs from "node:fs/promises";
import path from "node:path";
import { repoRoot } from "./config.mjs";

export async function readText(relativePath) {
  return fs.readFile(path.join(repoRoot, relativePath), "utf8");
}

export async function readJson(relativePath) {
  return JSON.parse(await readText(relativePath));
}

export async function readJsonIfPresent(relativePath) {
  try {
    return await readJson(relativePath);
  } catch {
    return null;
  }
}

export async function sha256FileIfPresent(relativePath) {
  try {
    const text = await fs.readFile(path.join(repoRoot, relativePath), "utf8");
    return `sha256:${createHash("sha256").update(text, "utf8").digest("hex")}`;
  } catch (error) {
    if (error?.code === "ENOENT") {
      return "";
    }
    throw error;
  }
}
