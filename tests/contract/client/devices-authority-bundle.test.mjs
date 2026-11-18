import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(fileURLToPath(new URL("../../..", import.meta.url)));
const read = (relativePath) => readFileSync(path.join(repoRoot, relativePath), "utf8");

test("the regular Rust suite receives the pinned LicoArc authority bundle", () => {
  const verify = read(".github/workflows/verify.yml");
  assert.ok(
    verify.includes(
      "https://raw.githubusercontent.com/LicoLand/LicoArc/e33b151a6d9870b73857f68d333f15010584cc2c/artifacts/v1/licoarc.bundle.json",
    ),
    "the bundle URL must name the reviewed LicoArc commit",
  );
  assert.ok(
    verify.includes("e47aa234dee48ca3f791c733b2514c93f1e912bce8f5754813879c6b197d6814"),
    "the downloaded bundle must be checked against its published SHA-256",
  );
  assert.ok(
    verify.includes("LICOARC_AUTHORITY_BUNDLE=$RUNNER_TEMP/licoarc.bundle.json"),
    "the bundle path must reach the test process through the documented environment",
  );
  assert.ok(
    verify.indexOf("Fetch the canonical LicoArc authority bundle") <
      verify.indexOf("node tools/scripts/cargo-client.mjs test"),
    "the bundle must be fetched before the workspace test command",
  );
});

test("the devices pairing and offline catch-up scenarios stay in the regular suite", () => {
  const tests = read("crates/licoup/src/domain/devices/tests.rs");
  for (const name of [
    "two_homes_pair_send_results_and_unpair",
    "two_endpoints_catch_up_after_offline_and_ignore_duplicate_delivery",
  ]) {
    assert.ok(tests.includes(`fn ${name}(`), `missing ${name}`);
  }
  assert.ok(
    tests.includes("session::AUTHORITY_BUNDLE_ENV"),
    "both scenarios must read the reviewed authority bundle, not a local copy",
  );
  assert.ok(
    tests.includes("LocalStation"),
    "the offline scenario must drive a local Station mailbox, not a direct call",
  );
  assert.ok(
    tests.includes("station.replay(&delivered, &proc_b, &seq_b, &handle_b)"),
    "the offline scenario must replay an ACKed delivery",
  );
});
