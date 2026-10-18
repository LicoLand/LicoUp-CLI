import assert from "node:assert/strict";
import test from "node:test";
import { isReviewedPrivacyFixture } from "../../../tools/scripts/lib/privacy-test-fixtures.mjs";

test("migration privacy fixtures require the exact owner, rule, full value and boundary", () => {
  const prefix = ["", "Users", "maintainer"].join("/");
  for (const suffix of ["Library/Application Support/LicoUp", "private/state.db"]) {
    const value = `${prefix}/${suffix}`;
    const input = { file: "crates/licoup-migrate/src/archive.rs", rule: "FORBIDDEN_MACOS_HOME_PATH",
      source: `error at ${value}"`, start: 9, match: prefix };
    assert.equal(isReviewedPrivacyFixture(input), true);
    assert.equal(isReviewedPrivacyFixture({ ...input, file: "other.rs" }), false);
    assert.equal(isReviewedPrivacyFixture({ ...input, rule: "FORBIDDEN_SECRET_ASSIGNMENT" }), false);
    for (const changed of [value + "/child", value + ".extra", value.replace("maintainer", "private-account")]) {
      assert.equal(isReviewedPrivacyFixture({ ...input, source: `error at ${changed}"` }), false);
    }
  }
});
