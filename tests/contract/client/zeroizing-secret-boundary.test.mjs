import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(fileURLToPath(new URL("../../..", import.meta.url)));
const nativeRoot = "crates/licoup/src";
const platformRoot = `${nativeRoot}/platform/secret_store`;
const catalogRoot = "tools/regression/client-module-catalog";
const RUST_WHITESPACE = String.raw`\s`;

async function read(relativePath) {
  return fs.readFile(path.join(repoRoot, relativePath), "utf8");
}

async function exists(relativePath) {
  try {
    await fs.access(path.join(repoRoot, relativePath));
    return true;
  } catch {
    return false;
  }
}

async function rustSources(relativePath) {
  const absolute = path.join(repoRoot, relativePath);
  const entries = await fs.readdir(absolute, { withFileTypes: true });
  const sources = [];
  for (const entry of entries) {
    const child = path.join(relativePath, entry.name);
    if (entry.isDirectory()) {
      if (entry.name !== "tests") sources.push(...await rustSources(child));
    } else if (entry.isFile() && entry.name.endsWith(".rs")) {
      sources.push([child, await read(child)]);
    }
  }
  return sources;
}

function implementationBlocks(source, traitName) {
  const blocks = [];
  const marker = `impl ${traitName} for `;
  let cursor = 0;
  while ((cursor = source.indexOf(marker, cursor)) >= 0) {
    const open = source.indexOf("{", cursor + marker.length);
    assert.notEqual(open, -1, `unterminated ${traitName} implementation`);
    let depth = 0;
    let end = open;
    for (; end < source.length; end += 1) {
      if (source[end] === "{") depth += 1;
      if (source[end] === "}") depth -= 1;
      if (depth === 0) {
        end += 1;
        break;
      }
    }
    blocks.push(source.slice(cursor, end));
    cursor = end;
  }
  return blocks;
}

function rustFunctionBlock(source, functionName) {
  const pattern = new RegExp(
    `\\bfn${RUST_WHITESPACE}+${functionName}(?:${RUST_WHITESPACE}*<[^>{}]*>)?${RUST_WHITESPACE}*\\(`,
    "u",
  );
  const match = pattern.exec(source);
  assert.ok(match, `missing Rust function ${functionName}`);
  const open = source.indexOf("{", match.index + match[0].length);
  assert.notEqual(open, -1, `unterminated Rust function ${functionName}`);
  let depth = 0;
  for (let cursor = open; cursor < source.length; cursor += 1) {
    if (source[cursor] === "{") depth += 1;
    if (source[cursor] === "}") depth -= 1;
    if (depth === 0) return source.slice(match.index, cursor + 1);
  }
  assert.fail(`unterminated Rust function ${functionName}`);
}

test("every secret-store implementation transfers bounded owned SecretBytes", async () => {
  const port = await read(`${nativeRoot}/core/secret_store/port.rs`);
  assert.match(
    port,
    /fn set_secret\([^)]*secret:\s*SecretBytes\)\s*->\s*Result<\(\)>/su,
  );
  assert.match(
    port,
    /fn get_secret\([^)]*\)\s*->\s*Result<Option<SecretBytes>>/su,
  );
  for (const forbidden of [
    /secret:\s*&str/u,
    /Result<Option<String>>/u,
    /Zeroizing<String>/u,
  ]) {
    assert.doesNotMatch(port, forbidden);
  }

  const allNativeSources = await rustSources(nativeRoot);
  const implementations = allNativeSources.flatMap(([relativePath, source]) =>
    implementationBlocks(source, "SecretStore")
      .map((block) => [relativePath, block]));
  assert.ok(implementations.length > 0);
  for (const [relativePath, block] of implementations) {
    assert.doesNotMatch(block, /secret:\s*&str/u, relativePath);
    assert.doesNotMatch(block, /Result<Option<String>>/u, relativePath);
    assert.match(block, /secret:\s*SecretBytes/u, relativePath);
    assert.match(block, /Result<Option<SecretBytes>>/u, relativePath);
  }
});

test("the platform secret store has no String compatibility secret payload", async () => {
  const platform = (await rustSources(platformRoot))
    .map(([relativePath, source]) => `// ${relativePath}\n${source}`)
    .join("\n");
  assert.ok(platform.includes("SecretBytes"), "platform secret store must use SecretBytes");
  assert.doesNotMatch(platform, /Option<String>\s*>\s*\{\s*[^}]*secret/isu);
  assert.doesNotMatch(platform, /secret:\s*&str/u);
  assert.doesNotMatch(platform, /Zeroizing<String>/u);
});

test("legacy keyring implementation and catalog residue are absent", async () => {
  assert.equal(
    await exists(`${platformRoot}/platform_backends/keyring.rs`),
    false,
  );
  const catalogSources = await rustSources(catalogRoot).catch(() => []);
  const catalogEntries = await fs.readdir(path.join(repoRoot, catalogRoot), {
    withFileTypes: true,
  });
  const topLevelCatalog = await Promise.all(catalogEntries
    .filter((entry) => entry.isFile() && entry.name.endsWith(".mjs"))
    .map((entry) => read(`${catalogRoot}/${entry.name}`)));
  const groupEntries = await fs.readdir(path.join(repoRoot, catalogRoot, "groups"), {
    withFileTypes: true,
  });
  const groups = await Promise.all(groupEntries
    .filter((entry) => entry.isFile() && entry.name.endsWith(".mjs"))
    .map((entry) => read(`${catalogRoot}/groups/${entry.name}`)));
  const catalog = [
    ...catalogSources.map(([, source]) => source),
    ...topLevelCatalog,
    ...groups,
  ].join("\n");

  assert.equal(catalog.includes("platform_backends/keyring.rs"), false);
  assert.equal(catalog.includes("secret_store/keyring"), false);
});
