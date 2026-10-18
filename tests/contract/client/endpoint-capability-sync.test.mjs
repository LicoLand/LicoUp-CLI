import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const read = (path) => readFileSync(path, "utf8");

const PACKAGE = "components/endpoint-collaboration";
const lib = read(`${PACKAGE}/src/lib.rs`);
const catalogue = read(`${PACKAGE}/src/capability_catalogue.rs`);
const durable = read(`${PACKAGE}/src/durable_delivery.rs`);
const manifest = read(`${PACKAGE}/Cargo.toml`);
const doc = read("docs/modules/endpoint-collaboration.md");

test("the catalogue is part of the optional package and not a second boundary", () => {
  assert.match(lib, /^pub mod capability_catalogue;$/mu);
  assert.match(lib, /^pub mod durable_delivery;$/mu);
  // The catalogue is announced into; it does not declare the capability, does
  // not decide availability and does not repeat the outbound cut.
  assert.match(lib, /pub const CAPABILITY_ID: &str = "endpoint\.collaboration\.v1";/u);
  assert.doesNotMatch(catalogue, /PACKAGE_ID|CAPABILITY_ID|OutboundRefusal|Availability/u);
  // The package reaches the kernel only through the consumer-owned ports.
  assert.match(manifest, /licoup-endpoint-core = \{ path = "\.\.\/\.\.\/crates\/licoup-endpoint-core" \}/u);
  assert.doesNotMatch(manifest, /licoarc|licoup-native|keyring|security-framework/u);
});

test("an announcement records what a peer offers and grants no execution authority", () => {
  // Recording is one entry point, and it answers with the entry's own revision.
  assert.match(catalogue, /pub fn announce\(/u);
  assert.match(catalogue, /pub enum AnnouncementOutcome/u);
  // Authority is a separate, explicit, local decision.
  assert.match(catalogue, /pub fn authority\(&self, peer: &PeerKey, capability: &str\)/u);
  assert.match(catalogue, /pub fn grant_authority\(/u);
  const authority = catalogue.slice(
    catalogue.indexOf("pub fn authority(&self, peer: &PeerKey, capability: &str)"),
    catalogue.indexOf("pub fn grant_authority("),
  );
  assert.match(authority, /CapabilityAuthority::NotAnnounced/u);
  assert.match(authority, /CapabilityAuthority::AnnouncedNotAuthorized/u);
  assert.match(authority, /entry\.authorized\.contains\(capability\)/u);
  // Only the explicit grant inserts into the granted set; `announce` never does.
  const announce = catalogue.slice(
    catalogue.indexOf("pub fn announce("),
    catalogue.indexOf("pub fn reconcile("),
  );
  assert.doesNotMatch(announce, /authorized\.insert/u);
  assert.match(catalogue, /pub fn withdraw_authority\(/u);
});

test("device and group identities stay distinct and a group is not re-owned", () => {
  assert.match(catalogue, /pub struct DeviceIdentity\(String\);/u);
  assert.match(catalogue, /pub struct GroupIdentity\(String\);/u);
  assert.match(catalogue, /pub struct PeerKey \{\s*device: DeviceIdentity,\s*group: GroupIdentity,\s*\}/u);
  assert.match(catalogue, /CatalogueRefusal::GroupOwnerMismatch/u);
  assert.match(catalogue, /pub fn group_owner\(&self, group: &GroupIdentity\)/u);
  // The group is claimed once, by the device that announced it first.
  assert.match(catalogue, /\.or_insert_with\(\|\| announcement\.peer\.device\(\)\.clone\(\)\)/u);
});

test("stale announcements and rotation rollbacks are explicit answers", () => {
  assert.match(
    catalogue,
    /StaleAnnouncement \{ announced: u64, held: u64 \}/u,
  );
  assert.match(catalogue, /endpoint_capability_stale_announcement/u);
  assert.match(
    catalogue,
    /RotationEpochRollback \{ announced: u64, held: u64 \}/u,
  );
  assert.match(catalogue, /endpoint_capability_rotation_epoch_rollback/u);
  // Both refusals carry the value the peer sent and the value this client holds,
  // and both are decided before anything is written.
  const announce = catalogue.slice(
    catalogue.indexOf("pub fn announce("),
    catalogue.indexOf("pub fn reconcile("),
  );
  assert.ok(
    announce.indexOf("RotationEpochRollback") < announce.indexOf("self.rotations"),
    "the epoch is checked before the catalogue is written",
  );
  assert.ok(
    announce.indexOf("StaleAnnouncement") < announce.indexOf("self.peers.entry"),
    "a stale announcement is refused before the entry is written",
  );
  // Offline catch-up answers every announcement instead of losing the batch.
  assert.match(catalogue, /pub fn reconcile\(/u);
  assert.match(catalogue, /\.map\(\|announcement\| self\.announce\(announcement\)\)/u);
});

test("protected material and required capabilities refuse a downgrade", () => {
  assert.match(catalogue, /RequiredCapabilityRemoved \{ capability: String \}/u);
  assert.match(
    catalogue,
    /ProtectedMaterialOutstanding \{\s*capability: String,\s*material: usize,?\s*\}/u,
  );
  assert.match(catalogue, /endpoint_capability_required_removed/u);
  assert.match(catalogue, /endpoint_capability_protected_material_outstanding/u);
  // Material leaves only through the settlement of its own identity.
  assert.match(catalogue, /pub fn settle_protected_material\(&mut self, peer: &PeerKey, id: &str\)/u);
  const settle = catalogue.slice(
    catalogue.indexOf("pub fn settle_protected_material("),
    catalogue.indexOf("pub fn revision("),
  );
  assert.match(settle, /reference\.id\(\) != id/u);
  // A rotation is a device fact: it keeps the entry, the grants and the material.
  assert.match(catalogue, /pub fn rotation_epoch\(&self, device: &DeviceIdentity\)/u);
  assert.match(
    catalogue,
    /self\s*\.rotations\s*\.insert\(\s*announcement\.peer\.device\(\)\.clone\(\),\s*announcement\.rotation_epoch,?\s*\)/u,
  );
});

test("the catalogue performs no cryptographic, store or transport operation", () => {
  assert.doesNotMatch(
    catalogue,
    /KeyCustody|CustodyHandle|KeyMutation|Transport|compare_and_swap|submit\(/u,
  );
  assert.doesNotMatch(catalogue, /licoup_endpoint_core/u);
  assert.doesNotMatch(catalogue, /fs::|File::|std::net|reqwest|serde/u);
  // The durable protocol store is the only place a delivery unit is committed
  // or settled, and only an accepted attempt settles one.
  assert.match(durable, /pub const fn may_settle\(outcome: TransportOutcome\) -> bool/u);
  assert.match(durable, /matches!\(outcome, TransportOutcome::Accepted\)/u);
});

test("the shipped proofs are synthetic peers and capability announcements", () => {
  for (const proof of [
    "an_announcement_records_what_a_peer_offers_and_grants_nothing",
    "only_an_explicit_local_grant_produces_authority",
    "a_stale_announcement_is_an_explicit_answer_not_a_silent_drop",
    "a_device_and_a_group_are_separate_identities",
    "a_group_is_not_re_owned_by_another_device",
    "a_required_capability_cannot_be_downgraded_away",
    "unknown_optional_is_preserved_and_unknown_required_refuses",
    "pending_protected_material_survives_catalogue_changes_and_refuses_removal",
    "offline_catch_up_reports_every_answer_including_the_refusals",
    "a_rotation_keeps_the_device_its_grants_and_its_material",
    "a_rotation_epoch_rollback_is_refused_with_both_epochs",
    "the_rotation_epoch_belongs_to_the_device_and_not_to_one_group",
  ]) {
    assert.match(catalogue, new RegExp(`fn ${proof}\\(`, "u"));
  }
  assert.doesNotMatch(catalogue, /licoarc::|reqwest|tokio|keyring/u);
});

test("the owning module guidance states the catalogue rules", () => {
  for (const statement of [
    "Capability synchronisation",
    "An \\*\\*announcement is not authority\\*\\*",
    "A \\*\\*stale announcement is an explicit answer\\*\\*",
    "A \\*\\*rotation epoch only advances\\*\\*",
    "A \\*\\*required capability cannot be downgraded away\\*\\*",
    "None of this changes a session",
  ]) {
    assert.match(doc, new RegExp(statement, "u"));
  }
});
