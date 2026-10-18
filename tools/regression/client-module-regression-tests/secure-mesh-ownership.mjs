import {
  assert,
  test,
  CLIENT_MODULE_CATALOG,
  selectModulesForChangedPaths,
  ids,
  sourceFiles,
} from "./support.mjs";

test("approval modules retain leaf-owned inputs and exact command filters", () => {
  const filters = new Map([
    ["rust.core.secure-mesh.approval.request",
      "core::secure_mesh_approval::tests::request::"],
    ["rust.core.secure-mesh.approval.fanout",
      "core::secure_mesh_approval::tests::fanout::"],
    ["rust.core.secure-mesh.approval.response",
      "core::secure_mesh_approval::tests::response::"],
    ["rust.core.secure-mesh.approval.capability",
      "core::secure_mesh_approval::tests::capability::"],
  ]);
  for (const [id, filter] of filters) {
    const module = CLIENT_MODULE_CATALOG.find((candidate) => candidate.id === id);
    assert.equal(module.command.args.at(-1), filter);
    assert.equal(module.inputs.includes(
      "crates/licoup-native/src/core/secure_mesh_approval.rs"), false);
  }
});

test("BadTower station transport has complete focused ownership", async () => {
  const module = CLIENT_MODULE_CATALOG.find((candidate) =>
    candidate.id === "rust.platform.badtower-station");
  assert.equal(module.command.args.at(-1), "platform::badtower_station::tests::");
  const preciseInputs = new Set(module.inputs);
  const splitSources = await sourceFiles(
    "crates/licoup-native/src/platform/badtower_station",
    ".rs",
  );
  for (const relativePath of splitSources) {
    assert.equal(preciseInputs.has(relativePath), true,
      `BadTower station source must have a focused regression owner: ${relativePath}`);
  }
});

test("mobile relay panel leaves retain exact widget tests and bounded catalog ownership", () => {
  const filters = new Map([
    ["flutter.feature.mobile-relay.panel-composition",
      "test/mobile_relay_panel/composition_test.dart"],
    ["flutter.feature.mobile-relay.panel-pairing",
      "test/mobile_relay_panel/pairing_test.dart"],
    ["flutter.feature.mobile-relay.panel-qr",
      "test/mobile_relay_panel/qr_test.dart"],
    ["flutter.feature.mobile-relay.panel-scan",
      "test/mobile_relay_panel/scan_test.dart"],
    ["flutter.feature.mobile-relay.panel-trust",
      "test/mobile_relay_panel/trust_test.dart"],
  ]);
  const modules = CLIENT_MODULE_CATALOG.filter((candidate) =>
    candidate.id.startsWith("flutter.feature.mobile-relay.panel-"));
  assert.equal(modules.length, filters.size);
  for (const [id, testPath] of filters) {
    const module = CLIENT_MODULE_CATALOG.find((candidate) => candidate.id === id);
    assert.equal(module.command.args.at(-1), testPath);
  }

  const sourceCheck = CLIENT_MODULE_CATALOG.find((candidate) =>
    candidate.id === "regression.mobile-relay-panel-source-bundle");
  const ownedInputs = new Set([
    ...modules.flatMap((module) => module.inputs),
    ...sourceCheck.inputs,
  ]);
  for (const relativePath of [
    "apps/desktop/lib/src/frontend/features/mobile_relay/ui/mobile_relay_panel.dart",
    "apps/desktop/lib/src/frontend/features/mobile_relay/ui/mobile_relay_panel/composition.dart",
    "apps/desktop/lib/src/frontend/features/mobile_relay/ui/mobile_relay_panel/pairing.dart",
    "apps/desktop/lib/src/frontend/features/mobile_relay/ui/mobile_relay_panel/qr.dart",
    "apps/desktop/lib/src/frontend/features/mobile_relay/ui/mobile_relay_panel/scan.dart",
    "apps/desktop/lib/src/frontend/features/mobile_relay/ui/mobile_relay_panel/trust.dart",
  ]) {
    assert.equal(ownedInputs.has(relativePath), true,
      `mobile relay panel source must have a focused regression owner: ${relativePath}`);
  }

  const mobileRelayFoundation = CLIENT_MODULE_CATALOG.find((candidate) =>
    candidate.id === "flutter.feature.mobile-relay");
  assert.equal(mobileRelayFoundation.inputs.includes(
    "apps/desktop/lib/src/frontend/features/mobile_relay/**"), false);
  assert.equal(mobileRelayFoundation.inputs.includes(
    "apps/desktop/lib/src/frontend/features/mobile_relay/ui/mobile_relay_panel.dart"), false);
});

test("key transparency workflows retain precise changed-file ownership", async () => {
  const sourceBundleId = "regression.key-transparency-source-bundle";
  const selections = new Map([
    ["authority/challenge.rs", "rust.domain.mobile-relay.key-transparency.authority"],
    ["authority/reset.rs", "rust.domain.mobile-relay.key-transparency.authority"],
    ["publication.rs", "rust.domain.mobile-relay.key-transparency.publication"],
    ["revocation.rs", "rust.domain.mobile-relay.key-transparency.revocation"],
    ["provision.rs", "rust.domain.mobile-relay.key-transparency.provision"],
    ["self_monitor.rs", "rust.domain.mobile-relay.key-transparency.monitor-gossip"],
    ["gossip.rs", "rust.domain.mobile-relay.key-transparency.monitor-gossip"],
    ["status.rs", "rust.domain.mobile-relay.key-transparency.status-config"],
  ]);
  for (const [leaf, moduleId] of selections) {
    assert.deepEqual(ids(selectModulesForChangedPaths([
      `crates/licoup-native/src/domain/mobile_relay/key_transparency/${leaf}`,
    ])), ["regression.rust-format", "regression.rust-clippy", sourceBundleId, "architecture.client-boundaries", moduleId]);
  }

  const filters = new Map([
    ["rust.domain.mobile-relay.key-transparency",
      "domain::mobile_relay::key_transparency::tests::"],
    ["rust.domain.mobile-relay.key-transparency.authority",
      "domain::mobile_relay::key_transparency::tests::authority::"],
    ["rust.domain.mobile-relay.key-transparency.publication",
      "domain::mobile_relay::key_transparency::tests::publication::"],
    ["rust.domain.mobile-relay.key-transparency.revocation",
      "domain::mobile_relay::key_transparency::tests::revocation::"],
    ["rust.domain.mobile-relay.key-transparency.provision",
      "domain::mobile_relay::key_transparency::tests::provision::"],
    ["rust.domain.mobile-relay.key-transparency.monitor-gossip",
      "domain::mobile_relay::key_transparency::tests::monitor_gossip::"],
    ["rust.domain.mobile-relay.key-transparency.status-config",
      "domain::mobile_relay::key_transparency::tests::status::"],
  ]);
  const modules = CLIENT_MODULE_CATALOG.filter((candidate) =>
    candidate.id.startsWith("rust.domain.mobile-relay.key-transparency"));
  assert.equal(modules.length, filters.size);
  for (const [moduleId, filter] of filters) {
    const module = CLIENT_MODULE_CATALOG.find((candidate) => candidate.id === moduleId);
    assert.equal(module.command.args.at(-1), filter);
  }

  const ownedInputs = new Set(modules.flatMap((module) => module.inputs));
  const splitSources = await sourceFiles(
    "crates/licoup-native/src/domain/mobile_relay/key_transparency",
    ".rs",
  );
  for (const relativePath of [
    "crates/licoup-native/src/domain/mobile_relay/key_transparency.rs",
    ...splitSources,
  ]) {
    assert.equal(ownedInputs.has(relativePath), true,
      `key-transparency source must have a precise regression owner: ${relativePath}`);
  }

  const sourceBundle = CLIENT_MODULE_CATALOG.find((candidate) =>
    candidate.id === sourceBundleId);
  assert.deepEqual(sourceBundle.command.args, [
    "--test",
    "tests/contract/client/key-transparency-source-bundle.test.mjs",
  ]);
});

test("local material leaves retain precise changed-file ownership", async () => {
  const sourceBundleId = "regression.local-material-source-bundle";
  const modulePrefix = "rust.domain.mobile-relay.endpoint-trust.local-material";
  const selections = new Map([
    ["identity_generation.rs", `${modulePrefix}.identity`],
    ["material_mutation.rs", `${modulePrefix}.identity`],
    ["prekey_generation.rs", `${modulePrefix}.prekey-inventory`],
    ["prekey_inventory.rs", `${modulePrefix}.prekey-inventory`],
    ["rotation.rs", `${modulePrefix}.rotation`],
    ["protocol_reset.rs", `${modulePrefix}.protocol-reset`],
    ["state_codec.rs", `${modulePrefix}.state-codec`],
    ["descriptor.rs", `${modulePrefix}.descriptor-accessors`],
    ["accessors.rs", `${modulePrefix}.descriptor-accessors`],
  ]);
  for (const [leaf, moduleId] of selections) {
    assert.deepEqual(ids(selectModulesForChangedPaths([
      `crates/licoup-native/src/domain/mobile_relay/endpoint_trust/local_material/${leaf}`,
    ])), ["regression.rust-format", "regression.rust-clippy", sourceBundleId, "architecture.client-boundaries", moduleId]);
  }

  const filters = new Map([
    [modulePrefix, "domain::mobile_relay::endpoint_trust::local_material::tests::"],
    [`${modulePrefix}.identity`,
      "domain::mobile_relay::endpoint_trust::local_material::tests::generation::"],
    [`${modulePrefix}.prekey-inventory`,
      "domain::mobile_relay::endpoint_trust::local_material::tests::inventory::"],
    [`${modulePrefix}.rotation`,
      "domain::mobile_relay::endpoint_trust::local_material::tests::rotation::"],
    [`${modulePrefix}.protocol-reset`,
      "domain::mobile_relay::endpoint_trust::local_material::tests::protocol_reset::"],
    [`${modulePrefix}.state-codec`,
      "domain::mobile_relay::endpoint_trust::local_material::tests::state_codec::"],
    [`${modulePrefix}.descriptor-accessors`,
      "domain::mobile_relay::endpoint_trust::local_material::tests::descriptor::"],
  ]);
  const modules = CLIENT_MODULE_CATALOG.filter((candidate) =>
    candidate.id === modulePrefix || candidate.id.startsWith(`${modulePrefix}.`));
  assert.equal(modules.length, filters.size);
  for (const [moduleId, filter] of filters) {
    const module = CLIENT_MODULE_CATALOG.find((candidate) => candidate.id === moduleId);
    assert.equal(module.command.args.at(-1), filter);
  }

  const ownedInputs = new Set(modules.flatMap((module) => module.inputs));
  const splitSources = await sourceFiles(
    "crates/licoup-native/src/domain/mobile_relay/endpoint_trust/local_material",
    ".rs",
  );
  for (const relativePath of [
    "crates/licoup-native/src/domain/mobile_relay/endpoint_trust/local_material.rs",
    ...splitSources,
  ]) {
    assert.equal(ownedInputs.has(relativePath), true,
      `local-material source must have a precise regression owner: ${relativePath}`);
  }

  const sourceBundle = CLIENT_MODULE_CATALOG.find((candidate) =>
    candidate.id === sourceBundleId);
  assert.deepEqual(sourceBundle.command.args, [
    "--test",
    "tests/contract/client/local-material-source-bundle.test.mjs",
  ]);
});

test("directory transparency leaves retain precise changed-file ownership", async () => {
  const sourceBundleId = "regression.directory-transparency-source-bundle";
  const modulePrefix = "rust.domain.mobile-relay.endpoint-trust.directory-transparency";
  const selections = new Map([
    ["claim.rs", `${modulePrefix}.claim`],
    ["config.rs", `${modulePrefix}.config-purpose`],
    ["clock.rs", `${modulePrefix}.clock`],
    ["freshness.rs", `${modulePrefix}.freshness`],
    ["verifier.rs", `${modulePrefix}.verifier`],
    ["ensure.rs", `${modulePrefix}.verifier`],
    ["authority.rs", `${modulePrefix}.authority-open`],
    ["authorization/peer.rs", `${modulePrefix}.peer-authorization`],
    ["authorization/local.rs", `${modulePrefix}.local-authorization`],
    ["authorization/exact.rs", `${modulePrefix}.exact-authorization`],
    ["test_support.rs", `${modulePrefix}.test-authority`],
  ]);
  for (const [leaf, moduleId] of selections) {
    assert.deepEqual(ids(selectModulesForChangedPaths([
      `crates/licoup-native/src/domain/mobile_relay/endpoint_trust/directory_transparency/${leaf}`,
    ])), ["regression.rust-format", "regression.rust-clippy", sourceBundleId, "architecture.client-boundaries", moduleId]);
  }

  const filters = new Map([
    [modulePrefix, "domain::mobile_relay::endpoint_trust::directory_transparency::tests::"],
    [`${modulePrefix}.claim`,
      "domain::mobile_relay::endpoint_trust::directory_transparency::tests::claim::"],
    [`${modulePrefix}.config-purpose`,
      "domain::mobile_relay::endpoint_trust::directory_transparency::tests::config::"],
    [`${modulePrefix}.clock`,
      "domain::mobile_relay::endpoint_trust::directory_transparency::tests::clock::"],
    [`${modulePrefix}.freshness`,
      "domain::mobile_relay::endpoint_trust::directory_transparency::tests::freshness::"],
    [`${modulePrefix}.verifier`,
      "domain::mobile_relay::endpoint_trust::directory_transparency::tests::verifier::"],
    [`${modulePrefix}.authority-open`,
      "domain::mobile_relay::endpoint_trust::directory_transparency::tests::authority::"],
    [`${modulePrefix}.peer-authorization`,
      "domain::mobile_relay::endpoint_trust::directory_transparency::tests::peer_authorization::"],
    [`${modulePrefix}.local-authorization`,
      "domain::mobile_relay::endpoint_trust::directory_transparency::tests::local_authorization::"],
    [`${modulePrefix}.exact-authorization`,
      "domain::mobile_relay::endpoint_trust::directory_transparency::tests::exact_authorization::"],
    [`${modulePrefix}.test-authority`,
      "domain::mobile_relay::endpoint_trust::directory_transparency::tests::test_support::"],
  ]);
  const modules = CLIENT_MODULE_CATALOG.filter((candidate) =>
    candidate.id === modulePrefix || candidate.id.startsWith(`${modulePrefix}.`));
  assert.equal(modules.length, filters.size);
  for (const [moduleId, filter] of filters) {
    const module = CLIENT_MODULE_CATALOG.find((candidate) => candidate.id === moduleId);
    assert.equal(module.command.args.at(-1), filter);
  }

  const ownedInputs = new Set(modules.flatMap((module) => module.inputs));
  const splitSources = await sourceFiles(
    "crates/licoup-native/src/domain/mobile_relay/endpoint_trust/directory_transparency",
    ".rs",
  );
  for (const relativePath of [
    "crates/licoup-native/src/domain/mobile_relay/endpoint_trust/directory_transparency.rs",
    ...splitSources,
  ]) {
    assert.equal(ownedInputs.has(relativePath), true,
      `directory-transparency source must have a precise regression owner: ${relativePath}`);
  }

  const sourceBundle = CLIENT_MODULE_CATALOG.find((candidate) =>
    candidate.id === sourceBundleId);
  assert.deepEqual(sourceBundle.command.args, [
    "--test",
    "tests/contract/client/directory-transparency-source-bundle.test.mjs",
  ]);
});

test("pairwise session leaves retain precise changed-file ownership", async () => {
  const sourceBundleId = "regression.pairwise-session-source-bundle";
  const modulePrefix = "rust.domain.mobile-relay.pairwise-session";
  const selections = new Map([
    ["status_projection.rs", `${modulePrefix}.status-projection`],
    ["response.rs", `${modulePrefix}.response-replay`],
    ["payload.rs", `${modulePrefix}.payload`],
    ["crypto_operation.rs", `${modulePrefix}.crypto-operation`],
    ["transaction.rs", `${modulePrefix}.transaction`],
    ["handshake.rs", `${modulePrefix}.handshake`],
    ["store.rs", `${modulePrefix}.store`],
  ]);
  for (const [leaf, moduleId] of selections) {
    assert.deepEqual(ids(selectModulesForChangedPaths([
      `crates/licoup-native/src/domain/mobile_relay/pairwise_session/${leaf}`,
    ])), ["regression.rust-format", "regression.rust-clippy", sourceBundleId, "architecture.client-boundaries", moduleId]);
  }

  const filters = new Map([
    [modulePrefix, "domain::mobile_relay::pairwise_session::tests::"],
    [`${modulePrefix}.scenarios`, "domain::mobile_relay::tests::pairwise_session::"],
    [`${modulePrefix}.status-projection`,
      "domain::mobile_relay::pairwise_session::tests::status_projection::"],
    [`${modulePrefix}.response-replay`,
      "domain::mobile_relay::pairwise_session::tests::response::"],
    [`${modulePrefix}.payload`, "domain::mobile_relay::pairwise_session::tests::payload::"],
    [`${modulePrefix}.crypto-operation`,
      "domain::mobile_relay::pairwise_session::tests::crypto_operation::"],
    [`${modulePrefix}.transaction`,
      "domain::mobile_relay::pairwise_session::tests::transaction::"],
    [`${modulePrefix}.handshake`,
      "domain::mobile_relay::pairwise_session::tests::handshake::"],
    [`${modulePrefix}.store`, "domain::mobile_relay::pairwise_session::tests::store::"],
  ]);
  const modules = CLIENT_MODULE_CATALOG.filter((candidate) =>
    candidate.id === modulePrefix || candidate.id.startsWith(`${modulePrefix}.`));
  assert.equal(modules.length, filters.size);
  for (const [moduleId, filter] of filters) {
    const module = CLIENT_MODULE_CATALOG.find((candidate) => candidate.id === moduleId);
    assert.equal(module.command.args.at(-1), filter);
  }

  const ownedInputs = new Set(modules.flatMap((module) => module.inputs));
  const splitSources = await sourceFiles(
    "crates/licoup-native/src/domain/mobile_relay/pairwise_session",
    ".rs",
  );
  for (const relativePath of [
    "crates/licoup-native/src/domain/mobile_relay/pairwise_session.rs",
    ...splitSources,
  ]) {
    assert.equal(ownedInputs.has(relativePath), true,
      `pairwise-session source must have a precise regression owner: ${relativePath}`);
  }

  const sourceBundle = CLIENT_MODULE_CATALOG.find((candidate) =>
    candidate.id === sourceBundleId);
  assert.deepEqual(sourceBundle.command.args, [
    "--test",
    "tests/contract/client/pairwise-session-source-bundle.test.mjs",
  ]);
});

test("relay operations leaves retain precise changed-file ownership", async () => {
  const sourceBundleId = "regression.relay-operations-source-bundle";
  const modulePrefix = "rust.domain.mobile-relay.relay-operations";
  const selections = new Map([
    ["command_handlers/check_in.rs", `${modulePrefix}.command-handlers`],
    ["command_handlers/create.rs", `${modulePrefix}.command-handlers`],
    ["command_handlers/poll.rs", `${modulePrefix}.command-handlers`],
    ["command_handlers/result.rs", `${modulePrefix}.command-handlers`],
    ["station.rs", `${modulePrefix}.station`],
    ["mailbox.rs", `${modulePrefix}.mailbox`],
    ["envelope.rs", `${modulePrefix}.envelope`],
    ["delivery.rs", `${modulePrefix}.delivery`],
    ["status.rs", `${modulePrefix}.status`],
    ["allow_list.rs", `${modulePrefix}.allow-list`],
  ]);
  for (const [leaf, moduleId] of selections) {
    assert.deepEqual(ids(selectModulesForChangedPaths([
      `crates/licoup-native/src/domain/mobile_relay/relay_operations/${leaf}`,
    ])), ["regression.rust-format", "regression.rust-clippy", sourceBundleId, "architecture.client-boundaries", moduleId]);
  }

  const filters = new Map([
    [modulePrefix, "domain::mobile_relay::relay_operations::tests::"],
    [`${modulePrefix}.scenario.session-device-restore`,
      "domain::mobile_relay::tests::relay_operations::session_device_restore::"],
    [`${modulePrefix}.scenario.envelope-roundtrip`,
      "domain::mobile_relay::tests::relay_operations::envelope_roundtrip::"],
    [`${modulePrefix}.scenario.identity-replay-safety`,
      "domain::mobile_relay::tests::relay_operations::identity_replay_safety::"],
    [`${modulePrefix}.scenario.local-result-authorization`,
      "domain::mobile_relay::tests::relay_operations::local_result_authorization::"],
    [`${modulePrefix}.command-handlers`,
      "domain::mobile_relay::relay_operations::tests::"],
    [`${modulePrefix}.station`, "domain::mobile_relay::relay_operations::tests::station::"],
    [`${modulePrefix}.mailbox`, "domain::mobile_relay::relay_operations::tests::mailbox::"],
    [`${modulePrefix}.envelope`, "domain::mobile_relay::relay_operations::tests::envelope::"],
    [`${modulePrefix}.delivery`, "domain::mobile_relay::relay_operations::tests::delivery::"],
    [`${modulePrefix}.status`, "domain::mobile_relay::relay_operations::tests::status::"],
    [`${modulePrefix}.allow-list`,
      "domain::mobile_relay::relay_operations::tests::allow_list::"],
  ]);
  const modules = CLIENT_MODULE_CATALOG.filter((candidate) =>
    candidate.id === modulePrefix || candidate.id.startsWith(`${modulePrefix}.`));
  assert.equal(modules.length, filters.size);
  for (const [moduleId, filter] of filters) {
    const module = CLIENT_MODULE_CATALOG.find((candidate) => candidate.id === moduleId);
    assert.equal(module.command.args.at(-1), filter);
  }

  const ownedInputs = new Set(modules.flatMap((module) => module.inputs));
  const splitSources = await sourceFiles(
    "crates/licoup-native/src/domain/mobile_relay/relay_operations",
    ".rs",
  );
  for (const relativePath of [
    "crates/licoup-native/src/domain/mobile_relay/relay_operations.rs",
    ...splitSources,
  ]) {
    assert.equal(ownedInputs.has(relativePath), true,
      `relay-operations source must have a precise regression owner: ${relativePath}`);
  }

  const scenarioSelections = new Map([
    ["relay_operations.rs", `${modulePrefix}.scenario.session-device-restore`],
    ["relay_operations/session_device_restore.rs",
      `${modulePrefix}.scenario.session-device-restore`],
    ["relay_operations/envelope_roundtrip.rs",
      `${modulePrefix}.scenario.envelope-roundtrip`],
    ["relay_operations/identity_replay_safety.rs",
      `${modulePrefix}.scenario.identity-replay-safety`],
    ["relay_operations/local_result_authorization.rs",
      `${modulePrefix}.scenario.local-result-authorization`],
  ]);
  for (const [leaf, moduleId] of scenarioSelections) {
    const path = `crates/licoup-native/src/domain/mobile_relay/tests/${leaf}`;
    assert.deepEqual(ids(selectModulesForChangedPaths([path])), [
      "regression.rust-format",
      "regression.rust-clippy",
      "architecture.client-boundaries",
      moduleId,
    ]);
    assert.equal(modules.filter((module) => module.inputs.includes(path)).length, 1,
      `relay operation scenario must have exactly one catalog owner: ${path}`);
  }

  const sourceBundle = CLIENT_MODULE_CATALOG.find((candidate) =>
    candidate.id === sourceBundleId);
  assert.deepEqual(sourceBundle.command.args, [
    "--test",
    "tests/contract/client/relay-operations-source-bundle.test.mjs",
  ]);
});

test("device trust modules retain leaf-owned inputs and exact command filters", () => {
  const filters = new Map([
    ["rust.core.secure-mesh.trust.cross-signature",
      "core::secure_mesh_trust::tests::signature::secure_mesh_device_cross_signature_verifies_and_rejects_tamper"],
    ["rust.core.secure-mesh.trust.record",
      "core::secure_mesh_trust::tests::signature::secure_mesh_device_trust_record_signature_binds_peer_and_expiry"],
    ["rust.core.secure-mesh.trust.verification",
      "core::secure_mesh_trust::tests::verification::"],
    ["rust.core.secure-mesh.trust.policy",
      "core::secure_mesh_trust::tests::policy::"],
    ["rust.core.secure-mesh.trust.authorization",
      "core::secure_mesh_trust::tests::authorization::"],
  ]);
  for (const [id, filter] of filters) {
    const module = CLIENT_MODULE_CATALOG.find((candidate) => candidate.id === id);
    assert.equal(module.command.args.at(-1), filter);
    assert.equal(module.inputs.includes(
      "crates/licoup-native/src/core/secure_mesh_trust.rs"), false);
  }
});

test("transparency modules retain leaf-owned inputs and exact command filters", () => {
  const filters = new Map([
    ["rust.core.secure-mesh.transparency.model",
      "core::secure_mesh_transparency::tests::proofs::directory_leaf_serialization_exposes_only_an_opaque_scope_commitment"],
    ["rust.core.secure-mesh.transparency.signature-freshness",
      "core::secure_mesh_transparency::tests::signature_freshness"],
    ["rust.core.secure-mesh.transparency.json-codec",
      "core::secure_mesh_transparency::tests::gossip::gossip_json_codec_round_trips_without_leaf_lists"],
    ["rust.core.secure-mesh.transparency.inclusion-proof",
      "core::secure_mesh_transparency::tests::proofs::rfc9162_inclusion_paths_are_logarithmic_and_exact"],
    ["rust.core.secure-mesh.transparency.consistency-proof",
      "core::secure_mesh_transparency::tests::proofs::rfc9162_consistency_paths_are_logarithmic_and_exact"],
    ["rust.core.secure-mesh.transparency.sparse-map",
      "core::secure_mesh_transparency::tests::proofs::sparse_map"],
    ["rust.core.secure-mesh.transparency.diagnostics",
      "core::secure_mesh_transparency::tests::diagnostics"],
    ["rust.core.secure-mesh.transparency.client-observation",
      "core::secure_mesh_transparency::tests::gossip::gossip_same_size_split_view_is_persisted"],
    ["rust.core.secure-mesh.transparency.client-authorization.current",
      "core::secure_mesh_directory::tests::purpose_receipt_requires_target_label_at_current_sth_and_freshness_after_restart"],
    ["rust.core.secure-mesh.transparency.client-authorization.absence",
      "core::secure_mesh_directory::tests::authenticated_absence_is_typed_and_bound_to_requested_label"],
    ["rust.core.secure-mesh.transparency.client-authorization.gossip",
      "core::secure_mesh_directory::tests::directory_authorization_requires_persisted_fresh_peer_gossip"],
    ["rust.core.secure-mesh.transparency.client-authorization.absence-rollback",
      "core::secure_mesh_directory::tests::previously_present_label_cannot_become_absent_across_restart"],
    ["rust.core.secure-mesh.transparency.persistence.schema",
      "core::secure_mesh_transparency::tests::persistence::unsupported_schema_requires_explicit_state_reset"],
    ["rust.core.secure-mesh.transparency.persistence.checkpoint-consistency",
      "core::secure_mesh_transparency::tests::persistence::sqlite_checkpoint_requires_consistency_and_persists_rollback_across_restart"],
    ["rust.core.secure-mesh.transparency.persistence.checkpoint-retention",
      "core::secure_mesh_transparency::tests::persistence::checkpoint_retention_is_bounded_without_weakening_latest_rollback_guard"],
    ["rust.core.secure-mesh.transparency.persistence.directory-quota",
      "core::secure_mesh_transparency::tests::persistence::directory_label_and_authorization_quotas_are_bounded_with_stale_reclamation"],
    ["rust.core.secure-mesh.transparency.persistence.gossip",
      "core::secure_mesh_transparency::tests::gossip::gossip_observations_bind_distinct_issue_times_for_the_same_tree_view"],
    ["rust.core.secure-mesh.transparency.persistence.time-watermark",
      "core::secure_mesh_transparency::tests::persistence::durable_time_watermark_prevents_clock_rollback_and_expiry_revival"],
    ["rust.core.secure-mesh.transparency.persistence.authenticated-time",
      "core::secure_mesh_transparency::tests::persistence::unauthenticated_temporal_input_cannot_persist_security_block"],
  ]);
  for (const [id, filter] of filters) {
    const module = CLIENT_MODULE_CATALOG.find((candidate) => candidate.id === id);
    assert.equal(module.command.args.at(-1), filter);
    assert.equal(module.inputs.includes(
      "crates/licoup-native/src/core/secure_mesh_transparency.rs"), false);
  }
});

test("protocol bindings own and test the complete Lico Arc relay codec", async () => {
  const module = CLIENT_MODULE_CATALOG.find((candidate) =>
    candidate.id === "rust.core.protocol-bindings");
  assert.ok(module);
  assert.equal(module.command.program, "cargo");
  assert.deepEqual(module.command.args, [
    "test",
    "--no-fail-fast",
    "-p",
    "licoup-protocol-bindings",
  ]);

  const relaySources = await sourceFiles(
    "crates/licoup-protocol-bindings/src/licoarc_relay",
    ".rs",
  );
  for (const relativePath of [
    "crates/licoup-protocol-bindings/src/licoarc_relay.rs",
    ...relaySources,
  ]) {
    assert.deepEqual(
      ids(selectModulesForChangedPaths([relativePath])),
      ["regression.rust-format", "regression.rust-clippy", "architecture.client-boundaries", "rust.core.protocol-bindings"],
      `protocol bindings must own and test ${relativePath}`,
    );
  }
});

test("secure mesh capability leaves retain exact tests and complete source ownership", async () => {
  const filters = new Map([
    ["rust.core.secure-mesh.capability.composition", "core::secure_mesh_capability::tests::composition::"],
    ["rust.core.secure-mesh.capability.taxonomy", "core::secure_mesh_capability::tests::taxonomy::"],
    ["rust.core.secure-mesh.capability.catalog", "core::secure_mesh_capability::tests::catalog::"],
    ["rust.core.secure-mesh.capability.facts", "core::secure_mesh_capability::tests::facts::"],
    ["rust.core.secure-mesh.capability.custody", "core::secure_mesh_capability::tests::custody::"],
    ["rust.core.secure-mesh.capability.evaluation", "core::secure_mesh_capability::tests::evaluation::"],
    ["rust.core.secure-mesh.capability.report", "core::secure_mesh_capability::tests::report::"],
  ]);
  const modules = CLIENT_MODULE_CATALOG.filter((candidate) =>
    candidate.id.startsWith("rust.core.secure-mesh.capability."));
  assert.equal(modules.length, filters.size);
  for (const [id, filter] of filters) {
    const module = CLIENT_MODULE_CATALOG.find((candidate) => candidate.id === id);
    assert.equal(module.command.args.at(-1), filter);
    if (!id.endsWith(".composition")) {
      assert.equal(module.inputs.includes(
        "crates/licoup-native/src/core/secure_mesh_capability.rs"), false);
    }
  }
  const sourceCheck = CLIENT_MODULE_CATALOG.find((candidate) =>
    candidate.id === "regression.secure-mesh-capability-source-bundle");
  assert.deepEqual(sourceCheck.command.args,
    ["--test", "tests/contract/client/secure-mesh-capability-source-bundle.test.mjs"]);
  const ownedInputs = new Set([
    ...modules.flatMap((module) => module.inputs),
    ...sourceCheck.inputs,
  ]);
  const splitSources = await sourceFiles(
    "crates/licoup-native/src/core/secure_mesh_capability", ".rs");
  for (const relativePath of [
    "crates/licoup-native/src/core/secure_mesh_capability.rs",
    ...splitSources,
  ]) {
    assert.equal(ownedInputs.has(relativePath), true,
      `secure mesh capability source must have a precise regression owner: ${relativePath}`);
  }
});

test("secure mesh prekey leaves retain exact tests and complete source ownership", async () => {
  const filters = new Map([
    ["rust.core.secure-mesh.prekey.composition", "core::secure_mesh_prekey::tests::composition::"],
    ["rust.core.secure-mesh.prekey.pairwise", "core::secure_mesh_prekey::tests::pairwise::"],
    ["rust.core.secure-mesh.prekey.key-package", "core::secure_mesh_prekey::tests::key_package::"],
    ["rust.core.secure-mesh.prekey.inventory", "core::secure_mesh_prekey::tests::inventory::"],
    ["rust.core.secure-mesh.prekey.validation", "core::secure_mesh_prekey::tests::validation::"],
  ]);
  const modules = CLIENT_MODULE_CATALOG.filter((candidate) =>
    candidate.id.startsWith("rust.core.secure-mesh.prekey."));
  assert.equal(modules.length, filters.size);
  for (const [id, filter] of filters) {
    const module = CLIENT_MODULE_CATALOG.find((candidate) => candidate.id === id);
    assert.equal(module.command.args.at(-1), filter);
    if (!id.endsWith(".composition")) {
      assert.equal(module.inputs.includes(
        "crates/licoup-native/src/core/secure_mesh_prekey.rs"), false);
    }
  }
  const sourceCheck = CLIENT_MODULE_CATALOG.find((candidate) =>
    candidate.id === "regression.secure-mesh-prekey-source-bundle");
  assert.deepEqual(sourceCheck.command.args,
    ["--test", "tests/contract/client/secure-mesh-prekey-source-bundle.test.mjs"]);
  const ownedInputs = new Set([
    ...modules.flatMap((module) => module.inputs),
    ...sourceCheck.inputs,
  ]);
  const splitSources = await sourceFiles(
    "crates/licoup-native/src/core/secure_mesh_prekey", ".rs");
  for (const relativePath of [
    "crates/licoup-native/src/core/secure_mesh_prekey.rs",
    ...splitSources,
  ]) {
    assert.equal(ownedInputs.has(relativePath), true,
      `secure mesh prekey source must have a precise regression owner: ${relativePath}`);
  }
});

test("pairwise negotiation and ratchet boundaries retain exact leaf ownership", async () => {
  const filters = new Map([
    ["rust.core.secure-mesh.pairwise-key-ratchet.core", "core::secure_mesh_pairwise::tests::key_ratchet"],
    ["rust.core.secure-mesh.pairwise-key-ratchet.payload-adapter", "core::secure_mesh_pairwise::tests::key_ratchet_payload_adapter::"],
    ["rust.core.secure-mesh.pairwise-key-ratchet.relay-codec", "core::secure_mesh_pairwise::tests::key_ratchet_relay_codec::"],
    ["rust.core.secure-mesh.pairwise-session-negotiation.handshake-machine", "core::secure_mesh_pairwise::tests::session_negotiation"],
    ["rust.core.secure-mesh.pairwise-session-negotiation.capability-binding", "core::secure_mesh_pairwise::tests::session_negotiation_capability_binding::"],
    ["rust.core.secure-mesh.pairwise-session-negotiation.key-schedule", "core::secure_mesh_pairwise::tests::session_negotiation_key_schedule::"],
    ["rust.core.secure-mesh.pairwise-session-negotiation.transcript-codec", "core::secure_mesh_pairwise::tests::session_negotiation_transcript_codec::"],
    ["rust.core.secure-mesh.pairwise-session-negotiation.input-validation", "core::secure_mesh_pairwise::tests::session_negotiation_input_validation::"],
  ]);
  const modules = CLIENT_MODULE_CATALOG.filter((candidate) =>
    candidate.id.startsWith("rust.core.secure-mesh.pairwise-key-ratchet.") ||
    candidate.id.startsWith("rust.core.secure-mesh.pairwise-session-negotiation."));
  assert.equal(modules.length, filters.size);
  for (const [id, filter] of filters) {
    const module = CLIENT_MODULE_CATALOG.find((candidate) => candidate.id === id);
    assert.equal(module.command.args.at(-1), filter);
    if (!id.endsWith(".core") && !id.endsWith(".handshake-machine")) {
      assert.equal(module.inputs.includes(
        "crates/licoup-native/src/core/secure_mesh_pairwise.rs"), false);
      assert.equal(module.inputs.includes(
        "crates/licoup-native/src/core/secure_mesh_pairwise/tests.rs"), false);
    }
  }
  assert.equal(CLIENT_MODULE_CATALOG.some((candidate) =>
    candidate.id === "rust.core.secure-mesh.pairwise-key-ratchet" ||
    candidate.id === "rust.core.secure-mesh.pairwise-session-negotiation"), false);

  const sourceCheck = CLIENT_MODULE_CATALOG.find((candidate) =>
    candidate.id === "regression.secure-mesh-pairwise-boundary-source-bundle");
  assert.deepEqual(sourceCheck.command.args, [
    "--test", "tests/contract/client/secure-mesh-pairwise-boundary-source-bundle.test.mjs",
  ]);
  const ownedInputs = new Set([
    ...modules.flatMap((module) => module.inputs),
    ...sourceCheck.inputs,
  ]);
  const splitSources = [
    ...await sourceFiles(
      "crates/licoup-native/src/core/secure_mesh_pairwise/session_negotiation", ".rs"),
    ...await sourceFiles(
      "crates/licoup-native/src/core/secure_mesh_pairwise/key_ratchet", ".rs"),
  ];
  for (const relativePath of [
    "crates/licoup-native/src/core/secure_mesh_pairwise/session_negotiation.rs",
    "crates/licoup-native/src/core/secure_mesh_pairwise/key_ratchet.rs",
    ...splitSources,
    "crates/licoup-native/src/core/secure_mesh_pairwise/tests/session_negotiation.rs",
    "crates/licoup-native/src/core/secure_mesh_pairwise/tests/session_negotiation_capability_binding.rs",
    "crates/licoup-native/src/core/secure_mesh_pairwise/tests/session_negotiation_input_validation.rs",
    "crates/licoup-native/src/core/secure_mesh_pairwise/tests/session_negotiation_key_schedule.rs",
    "crates/licoup-native/src/core/secure_mesh_pairwise/tests/session_negotiation_transcript_codec.rs",
    "crates/licoup-native/src/core/secure_mesh_pairwise/tests/key_ratchet.rs",
    "crates/licoup-native/src/core/secure_mesh_pairwise/tests/key_ratchet_payload_adapter.rs",
    "crates/licoup-native/src/core/secure_mesh_pairwise/tests/key_ratchet_relay_codec.rs",
  ]) {
    assert.equal(ownedInputs.has(relativePath), true,
      `pairwise boundary source must have a precise regression owner: ${relativePath}`);
  }
});

test("core state-machine physical test leaves retain precise regression ownership", () => {
  const sourceBundleId = "regression.core-state-machine-test-layout";
  const selections = new Map([
    [
      "crates/licoup-native/src/core/secure_mesh_acp/tests/mod.rs",
      "rust.core.secure-mesh.acp",
    ],
    [
      "crates/licoup-native/src/core/secure_mesh_capability_proof/tests/mod.rs",
      "rust.core.secure-mesh.capability-proof",
    ],
    [
      "crates/licoup-native/src/core/secure_mesh_session_negotiation/tests/mod.rs",
      "rust.core.secure-mesh.session-negotiation",
    ],
    [
      "crates/licoup-native/src/core/secure_mesh_sparse_pq_ratchet/tests/mod.rs",
      "rust.core.secure-mesh.sparse-pq-ratchet",
    ],
    [
      "crates/licoup-native/src/core/secure_mesh_mls_product/security_ledger/test_support.rs",
      "rust.core.secure-mesh.mls-product.security-ledger",
    ],
  ]);
  for (const [relativePath, rustModuleId] of selections) {
    assert.deepEqual(ids(selectModulesForChangedPaths([relativePath])), [
      "regression.rust-format",
      "regression.rust-clippy",
      sourceBundleId,
      "architecture.client-boundaries",
      rustModuleId,
    ]);
  }
  assert.deepEqual(ids(selectModulesForChangedPaths([
    "tests/contract/client/core-state-machine-test-layout.test.mjs",
  ])), [sourceBundleId]);

  const sourceBundle = CLIENT_MODULE_CATALOG.find((module) =>
    module.id === sourceBundleId);
  assert.deepEqual(sourceBundle.command.args, [
    "--test",
    "tests/contract/client/core-state-machine-test-layout.test.mjs",
  ]);
});

test("endpoint trust modules retain leaf-owned inputs and exact command filters", () => {
  const facade = CLIENT_MODULE_CATALOG.find((candidate) =>
    candidate.id === "rust.domain.mobile-relay.endpoint-trust");
  assert.equal(facade.command.args.at(-1),
    "domain::mobile_relay::endpoint_trust::tests::");
  const filters = new Map([
    ["rust.domain.mobile-relay.endpoint-trust.directory-transparency",
      "domain::mobile_relay::endpoint_trust::directory_transparency::tests::"],
    ["rust.domain.mobile-relay.endpoint-trust.local-material",
      "domain::mobile_relay::endpoint_trust::local_material::tests::"],
    ["rust.domain.mobile-relay.endpoint-trust.pairing-presentation",
      "domain::mobile_relay::endpoint_trust::pairing_presentation::tests::"],
    ["rust.domain.mobile-relay.endpoint-trust.pairwise-codec",
      "domain::mobile_relay::endpoint_trust::pairwise_codec::tests::"],
    ["rust.domain.mobile-relay.endpoint-trust.peer-trust",
      "domain::mobile_relay::endpoint_trust::peer_trust::tests::"],
    ["rust.domain.mobile-relay.endpoint-trust.persistence",
      "domain::mobile_relay::endpoint_trust::persistence::tests::"],
    ["rust.domain.mobile-relay.endpoint-trust.primitives",
      "domain::mobile_relay::endpoint_trust::primitives::tests::"],
    ["rust.domain.mobile-relay.endpoint-trust.scenarios",
      "domain::mobile_relay::tests::endpoint_trust::"],
  ]);
  for (const [id, filter] of filters) {
    const module = CLIENT_MODULE_CATALOG.find((candidate) => candidate.id === id);
    assert.equal(module.command.args.at(-1), filter);
    assert.equal(module.inputs.includes(
      "crates/licoup-native/src/domain/mobile_relay/endpoint_trust.rs"), false);
  }
});

test("secret custody modules retain leaf-owned inputs and exact command filters", () => {
  const facade = CLIENT_MODULE_CATALOG.find((candidate) =>
    candidate.id === "rust.domain.mobile-relay.secret-custody");
  assert.equal(facade.command.args.at(-1),
    "domain::mobile_relay::secret_custody::tests::");
  const filters = new Map([
    ["rust.domain.mobile-relay.secret-custody.cleanup",
      "domain::mobile_relay::secret_custody::cleanup::tests::"],
    ["rust.domain.mobile-relay.secret-custody.config-store",
      "domain::mobile_relay::secret_custody::config_store::tests::"],
    ["rust.domain.mobile-relay.secret-custody.persistence",
      "domain::mobile_relay::secret_custody::persistence::tests::"],
    ["rust.domain.mobile-relay.secret-custody.presentation",
      "domain::mobile_relay::secret_custody::presentation::tests::"],
    ["rust.domain.mobile-relay.secret-custody.reset-guard",
      "domain::mobile_relay::secret_custody::reset_guard::tests::"],
    ["rust.domain.mobile-relay.secret-custody.runtime",
      "domain::mobile_relay::secret_custody::runtime::tests::"],
    ["rust.domain.mobile-relay.secret-custody.secret-material",
      "domain::mobile_relay::secret_custody::secret_material::tests::"],
    ["rust.domain.mobile-relay.secret-custody.self-test",
      "domain::mobile_relay::secret_custody::self_test::tests::"],
    ["rust.domain.mobile-relay.secret-custody.scenario.config-integrity",
      "domain::mobile_relay::tests::secret_custody::config_integrity::"],
    ["rust.domain.mobile-relay.secret-custody.scenario.native-store-boundary",
      "domain::mobile_relay::tests::secret_custody::native_store_boundary::"],
    ["rust.domain.mobile-relay.secret-custody.scenario.ffi-dispatcher",
      "domain::mobile_relay::tests::secret_custody::ffi_dispatcher::"],
    ["rust.domain.mobile-relay.secret-custody.scenario.authorization-batches",
      "domain::mobile_relay::tests::secret_custody::authorization_batches::"],
    ["rust.domain.mobile-relay.secret-custody.cleanup-authority",
      "domain::mobile_relay::secret_custody::cleanup_authority::tests::"],
    ["rust.domain.mobile-relay.secret-custody.scenario.cleanup-authority",
      "domain::mobile_relay::tests::secret_custody::cleanup_authority::"],
    ["rust.domain.mobile-relay.secret-custody.scenario.public-config-restore",
      "domain::mobile_relay::tests::secret_custody::public_config_restore::"],
    ["rust.domain.mobile-relay.secret-custody.scenario.e2ee-status-authorization",
      "domain::mobile_relay::tests::secret_custody::e2ee_status_authorization::"],
    ["rust.domain.mobile-relay.secret-custody.scenario.secure-command-store",
      "domain::mobile_relay::tests::secret_custody::secure_command_store::"],
  ]);
  for (const [id, filter] of filters) {
    const module = CLIENT_MODULE_CATALOG.find((candidate) => candidate.id === id);
    assert.equal(module.command.args.at(-1), filter);
    assert.equal(module.inputs.includes(
      "crates/licoup-native/src/domain/mobile_relay/secret_custody.rs"), false);
  }

  const prefix = "rust.domain.mobile-relay.secret-custody";
  const scenarioModules = CLIENT_MODULE_CATALOG.filter((candidate) =>
    candidate.id.startsWith(`${prefix}.scenario.`));
  const scenarioSelections = new Map([
    ["secret_custody.rs", `${prefix}.scenario.config-integrity`],
    ["secret_custody/config_integrity.rs", `${prefix}.scenario.config-integrity`],
    ["secret_custody/native_store_boundary.rs", `${prefix}.scenario.native-store-boundary`],
    ["secret_custody/ffi_dispatcher.rs", `${prefix}.scenario.ffi-dispatcher`],
    ["secret_custody/authorization_batches.rs", `${prefix}.scenario.authorization-batches`],
    ["secret_custody/cleanup_authority.rs", `${prefix}.scenario.cleanup-authority`],
    ["secret_custody/public_config_restore.rs", `${prefix}.scenario.public-config-restore`],
    ["secret_custody/e2ee_status_authorization.rs",
      `${prefix}.scenario.e2ee-status-authorization`],
    ["secret_custody/secure_command_store.rs", `${prefix}.scenario.secure-command-store`],
  ]);
  for (const [leaf, moduleId] of scenarioSelections) {
    const path = `crates/licoup-native/src/domain/mobile_relay/tests/${leaf}`;
    assert.deepEqual(ids(selectModulesForChangedPaths([path])), [
      "regression.rust-format",
      "regression.rust-clippy",
      "architecture.client-boundaries",
      moduleId,
    ]);
    assert.equal(scenarioModules.filter((module) => module.inputs.includes(path)).length, 1,
      `secret custody scenario must have exactly one catalog owner: ${path}`);
  }
});
