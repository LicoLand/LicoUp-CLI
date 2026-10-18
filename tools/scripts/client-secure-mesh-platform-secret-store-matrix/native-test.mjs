import { runCargoTestFilter } from "../lib/cargo-test-filter-runner.mjs";
import { repoRoot } from "./io.mjs";
import { sanitizeError } from "./privacy.mjs";

export function runNativeTest(filter) {
  return runCargoTestFilter({
    repoRoot,
    manifestPath: "crates/licoup-native/Cargo.toml",
    filter,
    sanitizeError
  });
}
