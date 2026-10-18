import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const read = (path) => readFileSync(path, "utf8");

const CONTROL = "components/endpoint-collaboration/control";
const lib = read(`${CONTROL}/src/lib.rs`);
const manifest = read(`${CONTROL}/Cargo.toml`);
const control = read(`${CONTROL}/src/control/mod.rs`);
const owner = read(`${CONTROL}/src/control/owner.rs`);
const composerTests = read(`${CONTROL}/src/control/tests.rs`);
const settlement = read(`${CONTROL}/src/control/settlement.rs`);
const doc = read("docs/modules/endpoint-collaboration.md");

test("the settlement slice is part of the package and performs no effect itself", () => {
  assert.match(lib, /^pub mod control;$/mu);
  assert.match(control, /^pub mod settlement;$/mu);
  assert.match(control, /^pub mod owner;$/mu);
  // The package declares the port and never reaches for an operating-system or
  // process owner to perform the stop itself.
  assert.match(owner, /pub trait LocalWorkOwner/u);
  assert.match(owner, /fn request_stop\(/u);
  assert.match(owner, /fn force_stop\(/u);
  for (const source of [control, owner, settlement, composerTests]) {
    assert.doesNotMatch(source, /std::process|libc::|Command::new|kill\(/u);
  }
  assert.doesNotMatch(manifest, /licoup-native|nix|libc|windows-sys/u);
});

test("only locally admitted execution enters the local update gate", () => {
  assert.match(settlement, /pub enum ExecutionOwner\s*\{[^}]*LocalHost[^}]*PeerEndpoint[^}]*\}/su);
  assert.match(
    settlement,
    /pub const fn blocks_local_update\(&self\) -> bool\s*\{\s*matches!\(self\.owner, ExecutionOwner::LocalHost\)\s*&&\s*!self\.state\.is_confirmed\(\)\s*\}/u,
  );
  assert.match(settlement, /pub fn holds_local_work\(&self\) -> bool/u);
  assert.match(settlement, /pub fn blockers\(&self\) -> Vec<&TrackedExecution>/u);
  assert.match(settlement, /pub fn awaiting_peer\(&self\) -> Vec<&TrackedExecution>/u);
  // Merely awaiting a peer's result is not local execution.
  assert.match(
    settlement,
    /pub fn await_peer\(&mut self, execution: TrackedExecution\)/u,
  );
  assert.match(
    settlement,
    /SettlementRefusal::WrongOwner\s*\{\s*expected: ExecutionOwner::PeerEndpoint,/su,
  );
});

test("a peer request to work locally still passes local admission", () => {
  assert.match(settlement, /pub enum LocalAdmission\s*\{[^}]*Admitted[^}]*Refused\s*\{/su);
  assert.match(
    settlement,
    /pub fn admit_peer_requested\(\s*&mut self,\s*execution: TrackedExecution,\s*admission: LocalAdmission,?\s*\)/u,
  );
  assert.match(
    settlement,
    /LocalAdmission::Refused \{ reason \} => \{\s*Err\(SettlementRefusal::LocalAdmissionRefused \{ reason \}\)\s*\}/su,
  );
  assert.match(
    settlement,
    /SettlementRefusal::WrongOwner\s*\{\s*expected: ExecutionOwner::LocalHost,/su,
  );
});

test("a local observation is never proof of a remote outcome", () => {
  assert.match(
    settlement,
    /pub enum LocalObservation\s*\{[^}]*CarrierLost[^}]*GrantRevoked[^}]*Elapsed \{ seconds: u64 \}[^}]*\}/su,
  );
  assert.match(settlement, /pub const fn proves_remote_outcome\(self\) -> bool\s*\{\s*false\s*\}/u);
  assert.match(
    settlement,
    /pub fn note_local_observation\(\s*&mut self,\s*identity: &ExecutionIdentity,\s*observation: LocalObservation,?\s*\)/u,
  );
  const note = settlement.slice(
    settlement.indexOf("pub fn note_local_observation("),
    settlement.indexOf("pub fn apply_local_update("),
  );
  assert.doesNotMatch(note, /state = /u, "a local observation writes no state");
  assert.match(note, /proves_remote_outcome: observation\.proves_remote_outcome\(\)/u);
});

test("only an authenticated receipt moves a state, forwards, and never backwards", () => {
  assert.match(settlement, /pub struct AuthenticatedReceipt/u);
  assert.match(
    settlement,
    /pub fn record_receipt\(\s*&mut self,\s*identity: &ExecutionIdentity,\s*receipt: AuthenticatedReceipt,?\s*\)/u,
  );
  assert.match(settlement, /SettlementRefusal::StaleReceipt/u);
  assert.match(settlement, /SettlementRefusal::NotTracked/u);
  assert.match(
    settlement,
    /fn receipt_transition_allowed\(from: RemoteOutcomeState, to: RemoteOutcomeState\) -> bool/u,
  );
  assert.match(
    settlement,
    /RemoteOutcomeState::Confirmed => false,/u,
    "a confirmed end is absorbing",
  );
  assert.match(
    settlement,
    /pub const fn permits_blind_retry\(&self\) -> bool\s*\{\s*false\s*\}/u,
  );
});

test("compatible local updates preserve what this host owes and incompatible ones change nothing", () => {
  assert.match(
    settlement,
    /update\.endpoint == self\.local_identity\.endpoint\s*&&\s*update\.generation >= self\.local_identity\.generation/u,
  );
  assert.match(settlement, /SettlementRefusal::IncompatibleLocalUpdate/u);
  const apply = settlement.slice(settlement.indexOf("pub fn apply_local_update("));
  assert.doesNotMatch(
    apply.slice(0, apply.indexOf("pub fn blockers")),
    /remove|clear|retain/u,
    "a local update neither abandons nor drops a tracked remote record",
  );
  assert.match(settlement, /pub fn durable_record\(&self\) -> SettlementRecord/u);
  assert.match(settlement, /pub fn restored\(record: SettlementRecord\) -> Result<Self, SettlementRefusal>/u);
  assert.match(settlement, /SettlementRefusal::RecordSchemaMismatch/u);
});

test("the shipped proofs are synthetic peers, cursors and receipts", () => {
  for (const proof of [
    "an_unreachable_peer_does_not_hold_the_local_update_gate",
    "a_real_unfinished_local_task_does_hold_the_gate_until_it_is_confirmed",
    "local_admission_still_decides_whether_peer_requested_work_runs_here",
    "a_peer_owned_execution_cannot_be_admitted_as_local_work_and_the_reverse",
    "local_carrier_loss_revocation_and_elapsed_time_prove_no_remote_outcome",
    "a_stale_writer_cannot_move_the_recorded_state_backwards",
    "cancel_and_result_ordering_is_explicit_and_a_confirmed_end_is_absorbing",
    "a_receipt_for_an_untracked_identity_is_refused",
    "a_compatible_local_update_preserves_identities_cursors_and_unknown_effects",
    "an_incompatible_local_update_changes_nothing_here",
    "neither_a_restart_nor_a_local_update_fabricates_a_settlement",
    "a_record_from_another_schema_is_not_adopted",
    "tracking_one_identity_twice_is_refused",
    "every_refusal_has_a_distinct_stable_reason",
  ]) {
    assert.match(settlement, new RegExp(`fn ${proof}\\(`, "u"));
  }
  assert.doesNotMatch(settlement, /licoarc::|reqwest|tokio|keyring|security-framework/u);
  for (const proof of [
    "a_duplicate_control_is_answered_from_the_record_and_asked_once",
    "an_uncertain_owner_leaves_the_effect_unknown_and_a_redrive_asks_nothing_again",
    "an_owner_that_reaches_outside_the_selected_scope_is_visible_as_unknown",
    "the_selection_bound_refuses_rather_than_truncating_a_large_subtree",
  ]) {
    assert.match(composerTests, new RegExp(`fn ${proof}\\(`, "u"));
  }
});

test("the owning module guidance states the control and settlement rules", () => {
  for (const statement of [
    "components/endpoint-collaboration/control/",
    "Remote work control and settlement",
    "Admission precedes effect",
    "A request is never proof of exit",
    "Only locally admitted execution enters the local idle guard",
    "A local observation is not a remote outcome",
  ]) {
    assert.match(doc, new RegExp(statement, "u"));
  }
});
