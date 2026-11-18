import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import {
  archiveFormatFor,
  archiveName,
  binaryNames,
  formatGithubOutput,
  releaseAssets,
} from "../../../tools/scripts/release/cli-release-assets.mjs";

const repoRoot = path.resolve(fileURLToPath(new URL("../../..", import.meta.url)));
const read = (relativePath) => readFileSync(path.join(repoRoot, relativePath), "utf8");

const releaseWorkflow = read(".github/workflows/release.yml");
const buildWorkflow = read(".github/workflows/cli-release-build.yml");
const verifyWorkflow = read(".github/workflows/verify.yml");

const VERSION = "9.8.7";
const TARGETS = [
  ["aarch64-apple-darwin", "macos-15"],
  ["x86_64-unknown-linux-gnu", "ubuntu-24.04"],
  ["x86_64-pc-windows-msvc", "windows-2025"],
];

test("the release workflow builds every supported target on its native host", () => {
  assert.ok(
    releaseWorkflow.includes("uses: ./.github/workflows/cli-release-build.yml"),
    "the release workflow must reuse the shared native build",
  );
  const matrix = releaseWorkflow.slice(
    releaseWorkflow.indexOf("matrix:"),
    releaseWorkflow.indexOf("uses: ./.github/workflows/cli-release-build.yml"),
  );
  const pairs = [...matrix.matchAll(/- target: (\S+)\s+os: (\S+)/gu)].map((match) => [
    match[1],
    match[2],
  ]);
  assert.deepEqual(pairs, TARGETS);
  assert.match(releaseWorkflow, /needs: \[build\]/u);
});

test("Linux and Windows release archives use platform-correct names", () => {
  assert.equal(
    archiveName({ version: VERSION, target: "x86_64-unknown-linux-gnu" }),
    `licoup-${VERSION}-x86_64-unknown-linux-gnu.tar.gz`,
  );
  assert.equal(
    archiveName({ version: VERSION, target: "x86_64-pc-windows-msvc" }),
    `licoup-${VERSION}-x86_64-pc-windows-msvc.zip`,
  );
  assert.equal(
    archiveName({ version: VERSION, target: "aarch64-apple-darwin" }),
    `licoup-${VERSION}-aarch64-apple-darwin.tar.gz`,
  );
  assert.equal(archiveFormatFor("x86_64-pc-windows-msvc"), "zip");
  assert.equal(archiveFormatFor("x86_64-unknown-linux-gnu"), "tar.gz");
  assert.deepEqual(binaryNames("x86_64-pc-windows-msvc"), {
    cli: "licoup.exe",
    mcp: "lico-subagent-mcp.exe",
  });
  assert.deepEqual(binaryNames("x86_64-unknown-linux-gnu"), {
    cli: "licoup",
    mcp: "lico-subagent-mcp",
  });
});

test("the release plan emits the archive, checksum and binary names", () => {
  const assets = releaseAssets({ version: VERSION, target: "x86_64-pc-windows-msvc" });
  assert.equal(assets.checksum, `licoup-${VERSION}-x86_64-pc-windows-msvc.zip.sha256`);
  assert.equal(assets.format, "zip");
  const output = formatGithubOutput(assets);
  for (const line of [
    `archive=${assets.archive}`,
    `checksum=${assets.checksum}`,
    "format=zip",
    "cli_binary=licoup.exe",
    "mcp_binary=lico-subagent-mcp.exe",
  ]) {
    assert.ok(output.includes(`${line}\n`), `step output must contain ${line}`);
  }
  assert.throws(
    () => archiveName({ version: VERSION, target: "../escape" }),
    /cli_release_target_invalid/u,
  );
  assert.throws(
    () => archiveName({ version: VERSION, target: "x86_64-unknown-linux-gnu", format: "tar.xz" }),
    /cli_release_archive_format_invalid/u,
  );
});

test("the shared build workflow archives with native tools and one contract", () => {
  assert.match(buildWorkflow, /workflow_call:/u);
  assert.match(buildWorkflow, /runs-on: \$\{\{ inputs\.os \}\}/u);
  assert.ok(buildWorkflow.includes("build/crates/licoup/target/${TARGET}/release"));
  assert.ok(buildWorkflow.includes("tools/scripts/release/cli-release-contract.mjs"));
  assert.ok(buildWorkflow.includes("tools/scripts/cargo-client.mjs build --release --locked"));
  assert.ok(buildWorkflow.includes("tar -C"));
  assert.ok(buildWorkflow.includes("Compress-Archive"));
  assert.ok(buildWorkflow.includes("sha256sum"));
  assert.ok(buildWorkflow.includes("shasum -a 256"));
  assert.ok(buildWorkflow.includes("Get-FileHash"));
  assert.ok(
    buildWorkflow.includes("WriteAllText"),
    "Windows checksums must be written with LF for Linux verification",
  );
  assert.ok(buildWorkflow.includes("persist-credentials: false"));
  assert.ok(buildWorkflow.includes("if-no-files-found: error"));
  assert.ok(
    buildWorkflow.includes("actions/upload-artifact@ea165f8d65b6e75b540449e92b4886f43607fa02"),
    "artifact upload must stay pinned",
  );
});

test("aggregation publishes every archive and one combined SHA256SUMS on tags", () => {
  assert.ok(releaseWorkflow.includes("actions/download-artifact@d3f86a106a0bac45b974a628896c90dbdf5c8093"));
  assert.match(
    releaseWorkflow,
    /if: github\.event_name == 'push' && startsWith\(github\.ref, 'refs\/tags\/v'\)/u,
  );
  assert.ok(releaseWorkflow.includes("sha256sum -c"));
  assert.ok(releaseWorkflow.includes("tr -d '\\r'"));
  assert.ok(releaseWorkflow.includes("SHA256SUMS"));
  assert.ok(releaseWorkflow.includes("gh release create"));
  assert.ok(releaseWorkflow.includes("--verify-tag"));
  assert.ok(releaseWorkflow.includes("contents: write"));
  assert.ok(releaseWorkflow.includes("contents: read"));
  assert.ok(releaseWorkflow.includes("persist-credentials: false"));
});

test("verification builds Linux and Windows without publishing", () => {
  assert.match(
    verifyWorkflow,
    /if: github\.event_name == 'pull_request' \|\| github\.event_name == 'workflow_dispatch'/u,
  );
  assert.ok(verifyWorkflow.includes("uses: ./.github/workflows/cli-release-build.yml"));
  for (const [target, os] of TARGETS.slice(1)) {
    assert.ok(verifyWorkflow.includes(`target: ${target}`));
    assert.ok(verifyWorkflow.includes(`os: ${os}`));
  }
  assert.ok(!verifyWorkflow.includes("gh release create"));
});
