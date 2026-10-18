// Fixed refusal-test inputs, constructed in the migration owner's unit tests.
// Matching is limited to the exact source file, rule, complete value and literal
// boundary. Similar account names, paths and appended values remain findings.
const migrationPaths = [
  "/Users/maintainer/Library/Application Support/LicoUp",
  "/Users/maintainer/private/state.db",
];
const sourcePaths = new Set([
  "crates/licoup-migrate/src/archive.rs",
  "tools/scripts/lib/privacy-test-fixtures.mjs",
]);

export function isReviewedPrivacyFixture({ file, rule, source, start, match }) {
  if (rule !== "FORBIDDEN_MACOS_HOME_PATH" || !sourcePaths.has(file)) return false;
  return migrationPaths.some((value) => value.startsWith(match) &&
    source.startsWith(value, start) &&
    /^(?:$|[\r\n"'`),;:])/.test(source.slice(start + value.length)));
}
