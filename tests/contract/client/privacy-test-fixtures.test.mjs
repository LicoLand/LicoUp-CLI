import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import { isReviewedPrivacyFixture } from "../../../tools/scripts/lib/privacy-test-fixtures.mjs";

test("migration privacy fixtures require the exact owner, rule, full value and boundary", () => {
  const prefix = ["", "Users", "maintainer"].join("/");
  for (const suffix of ["Library/Application Support/LicoUp", "private/state.db"]) {
    const value = `${prefix}/${suffix}`;
    const input = { file: "crates/licoup/src/migrate/archive.rs", rule: "FORBIDDEN_MACOS_HOME_PATH",
      source: `error at ${value}"`, start: 9, match: prefix };
    assert.equal(isReviewedPrivacyFixture(input), true);
    assert.equal(isReviewedPrivacyFixture({ ...input, file: "other.rs" }), false);
    assert.equal(isReviewedPrivacyFixture({ ...input, rule: "FORBIDDEN_SECRET_ASSIGNMENT" }), false);
    for (const changed of [value + "/child", value + ".extra", value.replace("maintainer", "private-account")]) {
      assert.equal(isReviewedPrivacyFixture({ ...input, source: `error at ${changed}"` }), false);
    }
  }
});

test("private-key fixtures avoid repeated-byte constructor inputs", async () => {
  const files = [
    "crates/licoup/src/domain/collaboration_plugin/runner_signature.rs",
    "crates/licoup/src/domain/history_backup/tests.rs",
  ];
  const repeatedKey = /\b(?:SigningKey|RecoverySecret)::from_bytes\s*\(\s*&?\s*\[[^\[\];]+;\s*[^\[\];]+\]\s*,?\s*\)/gu;
  const findings = [];
  for (const file of files) {
    const source = await readFile(new URL(`../../../${file}`, import.meta.url), "utf8");
    for (const match of source.matchAll(repeatedKey)) {
      const line = source.slice(0, match.index).split("\n").length;
      findings.push(`${file}:${line}: generate private test key material at runtime`);
    }
  }
  assert.deepEqual(findings, []);
});
