import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const read = (path) => readFileSync(path, "utf8");

const CLEANUP = "components/endpoint-collaboration/cleanup";
const manifest = read(`${CLEANUP}/Cargo.toml`);
const lib = read(`${CLEANUP}/src/lib.rs`);
const stage = read(`${CLEANUP}/src/cleanup/stage.rs`);
const journal = read(`${CLEANUP}/src/cleanup/journal.rs`);
const receipt = read(`${CLEANUP}/src/cleanup/receipt.rs`);
const fileOwner = read(`${CLEANUP}/src/cleanup/file_owner.rs`);
const target = read(`${CLEANUP}/src/cleanup/target.rs`);
const tests = read(`${CLEANUP}/src/cleanup/tests.rs`);

test("the cleanup slice is its own workspace and composes the existing owners", () => {
  assert.match(manifest, /^\[workspace\]$/mu);
  assert.match(
    manifest,
    /^name = "licoup-endpoint-collaboration-cleanup"$/mu,
  );
  // The data-home, private-file, admission-lock and inventory owner. The
  // credential custody owner, the authenticated replacement-endpoint authority
  // and the erase loop stay in `licoup-native`; this slice never deletes a
  // credential and never reaches for a second walker, writer or keychain.
  assert.match(
    manifest,
    /licoup-foundation = \{ path = "\.\.\/\.\.\/\.\.\/crates\/licoup-foundation" \}/u,
  );
  assert.doesNotMatch(manifest, /keyring|security-framework|windows-sys|libsecret/u);
  assert.doesNotMatch(manifest, /licoup-native/u);
  assert.doesNotMatch(
    read(`${CLEANUP}/src/cleanup/mod.rs`),
    /cleanup_authority|SecretStoreHandle/u,
  );

  // The slice is a module tree with one owner per concern, and the library
  // surface publishes each owner's types instead of a second implementation.
  const moduleRoot = read(`${CLEANUP}/src/cleanup/mod.rs`);
  for (const module of ["file_owner", "journal", "receipt", "stage", "target"]) {
    assert.match(moduleRoot, new RegExp(`^mod ${module};$`, "mu"));
  }
  for (const exported of [
    "PrivateDataRootFileOwner",
    "CleanupFileOwner",
    "CleanupJournalStore",
    "CleanupStage",
    "CleanupReceiptPath",
    "FileStageReceipt",
    "FileStage",
    "CleanupInventory",
    "CleanupTarget",
  ]) {
    assert.match(lib, new RegExp(`\\b${exported}\\b`, "u"));
  }
});

test("the file stage stops at the file boundary and cannot reach terminal stages", () => {
  // The stage vocabulary is ordered and the file stage settles at
  // `FilesSettled`; completion is a later stage that decides its own
  // preconditions.
  const order = [...journal.matchAll(/^\s{4}(Admitted|WritersQuiesced|FilesSettled|CredentialsSettled|TerminalSettlement|Complete),$/gmu)].map(
    ([, name]) => name,
  );
  assert.deepEqual(order, [
    "Admitted",
    "WritersQuiesced",
    "FilesSettled",
    "CredentialsSettled",
    "TerminalSettlement",
    "Complete",
  ]);
  assert.match(journal, /cleanup_stage_transition_refused/u);
  assert.match(journal, /cleanup_journal_revision_stale/u);

  // The only stage the runner advances to is `WritersQuiesced` and
  // `FilesSettled`.
  const advances = [...stage.matchAll(/advance\(CleanupStage::(\w+)\)/gu)].map(([, name]) => name);
  assert.deepEqual([...new Set(advances)].sort(), ["FilesSettled", "WritersQuiesced"]);
  assert.doesNotMatch(stage, /CleanupStage::(Complete|TerminalSettlement|CredentialsSettled)/u);
});

test("a file-stage receipt is partial by construction and never admits", () => {
  assert.match(receipt, /pub const fn complete\(&self\) -> bool \{\s*false\s*\}/u);
  assert.match(receipt, /pub const fn admission_restored\(&self\) -> bool \{\s*false\s*\}/u);
  // No conversion turns a file-stage receipt into a finished cleanup.
  assert.doesNotMatch(receipt, /impl From<FileStageReceipt>/u);
  assert.doesNotMatch(receipt, /From<&FileStageReceipt>/u);
  assert.match(receipt, /A rejected or unavailable path stays rejected/u);
  // Delivery reports a refusal instead of swallowing it.
  assert.match(receipt, /Unavailable \{ code: String \}/u);
});

test("removals are confined to the frozen inventory and never follow a link", () => {
  // The removal set is exactly the frozen entries: the stage iterates the
  // inventory, never a directory walk.
  assert.match(stage, /for entry in inventory\.entries\(\)/u);
  assert.doesNotMatch(stage, /read_dir|walk_dir|remove_dir_all/u);
  assert.match(target, /cleanup_inventory_capacity_exceeded/u);
  assert.match(target, /cleanup_inventory_case_collision/u);
  assert.match(target, /canonical_relative_posix/u);

  // The production owner composes the repository's own admission lock and
  // bounded private-file primitives, refuses a link instead of resolving it,
  // and refuses a non-empty directory instead of recursing.
  assert.match(fileOwner, /licoup_foundation::core::full_data_root_archive::ADMISSION_LOCK_PATH/u);
  assert.match(fileOwner, /licoup_foundation::platform::file_security::sync_directory/u);
  assert.match(fileOwner, /cleanup_writers_running/u);
  assert.match(fileOwner, /cleanup_entry_symlink_refused/u);
  assert.match(fileOwner, /cleanup_directory_not_empty/u);
  assert.doesNotMatch(fileOwner, /remove_dir_all/u);
});

test("the shipped proofs are synthetic and never touch a real root or credential", () => {
  // Fixtures are in-memory or disposable temporary roots.
  assert.match(tests, /std::env::temp_dir\(\)/u);
  assert.match(tests, /fixture-in-memory-files/u);
  assert.match(tests, /fixture-restricted-control-path/u);
  assert.match(tests, /fn drop\(&mut self\)/u);
  // The proofs the node claims: restart at each boundary, external data
  // preserved, a refused entry stays pending, a partial receipt.
  for (const proof of [
    "a_restart_at_each_meaningful_boundary_resumes_without_removing_twice",
    "the_file_stage_settles_every_frozen_entry_and_leaves_unlisted_data_alone",
    "a_file_stage_receipt_stays_partial_and_cannot_report_completion",
    "a_recorded_removal_that_no_longer_holds_stops_the_stage_instead_of_advancing",
    "an_unavailable_restricted_path_is_reported_and_never_read_as_success",
    "the_private_data_root_owner_refuses_a_live_writer",
  ]) {
    assert.match(tests, new RegExp(`fn ${proof}\\(`, "u"));
  }
});
