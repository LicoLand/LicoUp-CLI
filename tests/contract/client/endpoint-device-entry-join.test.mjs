// Source assertions for the shipped device entry join and its projections.
//
// The joined native owners are not implemented in this tree, so these checks
// read the shipped Dart sources and the one native owner that does exist, and
// assert the rules the two Nodes claim: a resolved package answer is a cut, no
// intent is offered without the authority that covers it, an unknown remote
// effect is never re-driven, a file-stage receipt is never read as erased, and
// an ordinary sign-in never stands in for native authentication.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const read = (path) => readFileSync(path, "utf8");

const PROJECTION =
  "apps/desktop/lib/src/presentation/mobile_relay/device_replacement_projection.dart";
const POLICY =
  "apps/desktop/lib/src/application/features/mobile_relay/policy/device_replacement_policy.dart";
const SOURCE =
  "apps/desktop/lib/src/projections/mobile_relay/device_replacement_projection_source.dart";
const JOIN =
  "apps/desktop/lib/src/composition/features/mobile_relay/device_entry_join.dart";
const FEATURE_COMPOSITION =
  "apps/desktop/lib/src/composition/features/mobile_relay/mobile_relay_feature_composition.dart";
const FIXTURES = "apps/desktop/test/fixtures/device_replacement/scenarios.dart";
const PROJECTION_TEST = "apps/desktop/test/device_replacement_projection_test.dart";
const JOIN_TEST = "apps/desktop/test/device_entry_join_test.dart";
const NATIVE_OWNER =
  "crates/licoup-native/src/platform/extension_packages/endpoint_collaboration.rs";

const projection = read(PROJECTION);
const policy = read(POLICY);
const source = read(SOURCE);
const join = read(JOIN);
const featureComposition = read(FEATURE_COMPOSITION);
const fixtures = read(FIXTURES);
const nativeOwner = read(NATIVE_OWNER);

/** The member names one Dart enum declares, read from its own body. */
const enumMembers = (sourceText, name) => {
  const body = sourceText.match(
    new RegExp(`enum ${name} \\{([\\s\\S]*?)\\n\\}`, "u"),
  );
  assert.ok(body, `enum ${name} is declared`);
  return [...body[1].matchAll(/^ {2}([a-z][A-Za-z]*),$/gmu)].map(([, member]) => member);
};

test("the reduced action vocabulary carries no replay and no key export", () => {
  // The vocabulary is asserted exactly, so adding a re-driving or key-exporting
  // action to the shipped surface fails here instead of passing silently.
  assert.deepEqual(enumMembers(policy, "DeviceReplacementAction"), [
    "reviewErase",
    "submitEraseConfirmation",
    "requestRemoteStop",
    "refreshRemoteWorkState",
    "refreshTransferVerification",
    "reviewPendingCleanup",
    "reconnectEndpoint",
  ]);
  for (const member of enumMembers(policy, "DeviceReplacementAction")) {
    assert.doesNotMatch(member, /replay|resend|retry|export|key|secret|phrase/u);
  }
  // The join's own refusal vocabulary grants no re-drive and no renewed
  // admission.
  for (const member of enumMembers(join, "DeviceReplacementControlRefusal")) {
    assert.doesNotMatch(member, /replay|resend|retry|readmit|renew|restore/u);
  }
});

test("an unknown remote effect offers a re-read and never an action", () => {
  assert.match(policy, /static const String remoteEffectUnknown/u);
  assert.match(
    policy,
    /final unknown = effect\.state == DeviceRemoteEffectState\.unknown;/u,
  );
  assert.match(policy, /reason = unknown\s*\?\s*remoteEffectUnknown/u);
  assert.match(policy, /actionable: false/u);
  assert.match(
    policy,
    /offeredActions: immutablePresentationList\(const \[\s*DeviceReplacementAction\.refreshRemoteWorkState,/u,
  );
  // A late result keeps no ordinary authority either.
  assert.match(
    policy,
    /static bool carriesOrdinaryAuthority\(DeviceRemoteEffectFacts effect\) =>\s*effect\.state != DeviceRemoteEffectState\.unknown && !effect\.lateResult;/u,
  );
});

test("an offline or partial cleanup never becomes an erased or complete badge", () => {
  for (const [outcome, status] of [
    ["offline", "offline"],
    ["platformDenied", "platformDenied"],
    ["partial", "partial"],
  ]) {
    assert.match(
      policy,
      new RegExp(
        `DeviceCleanupOutcome\\.${outcome}\\s*=>\\s*DeviceCleanupStatus\\.${status},`,
        "u",
      ),
    );
    assert.doesNotMatch(
      policy,
      new RegExp(
        `DeviceCleanupOutcome\\.${outcome}\\s*=>\\s*DeviceCleanupStatus\\.confirmedComplete`,
        "u",
      ),
    );
  }
  // A completion claim needs every stage settled and a final cleanup receipt.
  assert.match(
    policy,
    /settledAllStages =\s*facts\.stage == DeviceCleanupStage\.complete &&\s*facts\.pendingEntryCount == 0/u,
  );
  assert.match(
    policy,
    /finalReceipt =\s*facts\.receiptKind == DeviceCleanupReceiptKind\.finalCleanup/u,
  );
  assert.match(policy, /localSettlementComplete = settledAllStages && finalReceipt/u);
  // The badge needs local settlement, delivery and the replacement's own answer.
  assert.match(
    policy,
    /erasedBadgeVisible =\s*status == DeviceCleanupStatus\.confirmedComplete &&\s*facts\.receiptIssued &&\s*facts\.receiptDelivered &&\s*facts\.replacementEndpointConfirmed;/u,
  );
  // Production reads the badge through the policy, never around it.
  assert.match(join, /DeviceReplacementPolicy\.cleanupPresentation\(/u);
});

test("no intent is offered without the authority that covers it", () => {
  assert.match(
    policy,
    /final currentAuthority =\s*authority\.activation == DeviceEndpointActivation\.active &&\s*authority\.outboundAuthority == DeviceOutboundAuthoritySource\.package &&\s*authority\.identityRotationEpoch == authority\.authorizedRotationEpoch &&\s*!revocation\.revoked;/u,
  );
  // The pre-package in-kernel path is named as an implementation, not a grant.
  assert.match(
    projection,
    /The pre-package path is a statement about\n\/\/\/ which implementation is running, never a grant\./u,
  );
  // A stop needs this host to own the stop and to not have asked already.
  assert.match(
    policy,
    /if \(currentAuthority &&\s*work\.selected &&\s*work\.stopOwnership == DeviceStopOwnership\.localOwner &&\s*!work\.stopAlreadyRequested\)/u,
  );
  // A submit needs a confirmation the gates accepted.
  assert.match(
    policy,
    /eraseConfirmationRefusal\(projection\) == null\) \{\s*actions\.add\(DeviceReplacementAction\.submitEraseConfirmation\);/u,
  );
});

test("ordinary sign-in never stands in for native authentication", () => {
  assert.match(policy, /if \(!review\.nativeAuthenticationPresent\) \{/u);
  assert.match(policy, /eraseRefusalNativeAuthenticationMissing/u);
  // The identifier exists to be ignored: the acceptance path never reads it.
  assert.doesNotMatch(policy, /ordinarySignInPresent/u);
  assert.match(projection, /ordinarySignInPresent/u);
  // The review has to name its own consequence and its reconnect behaviour.
  assert.match(
    policy,
    /review\.consequenceStatement != deviceAppOnlyEraseConsequenceStatement/u,
  );
  assert.match(
    policy,
    /review\.postReconnectStatement !=\s*deviceAppOnlyErasePostReconnectStatement/u,
  );
  assert.match(
    projection,
    /A confirmed app-only wipe executes after the old endpoint reconnects\./u,
  );
});

test("the package answer is a cut that refuses before the native entry is asked", () => {
  for (const state of [
    "active",
    "disabled",
    "capabilityUndeclared",
    "missing",
    "unreadable",
  ]) {
    assert.match(join, new RegExp(`^ {2}${state},$`, "mu"));
  }
  // Every refusal publishes the reason the native owner publishes verbatim.
  for (const reason of [
    "endpoint_collaboration_package_absent",
    "endpoint_collaboration_package_disabled",
    "endpoint_collaboration_capability_undeclared",
    "endpoint_collaboration_store_unreadable",
  ]) {
    assert.match(join, new RegExp(reason, "u"));
    assert.match(nativeOwner, new RegExp(reason, "u"));
  }
  // A refused package offers nothing at all.
  assert.match(
    join,
    /if \(!capabilityAvailable\) return immutablePresentationList\(const \[\]\);/u,
  );
  // And the control entry is never reached behind the gate.
  assert.match(
    join,
    /if \(!capabilityAvailable\) \{\s*return const DeviceReplacementControlAdmission\.refused\(\s*DeviceReplacementControlRefusal\.packageUnavailable,/u,
  );
  // The local client stays usable without the optional package.
  assert.match(join, /localClientUsable: true/u);
});

test("the shipped control entry refuses what the native owners cannot drive", () => {
  assert.match(
    join,
    /final class AbsentDeviceReplacementControl\s*implements DeviceReplacementControlPort \{/u,
  );
  assert.match(
    join,
    /refused\(\s*DeviceReplacementControlRefusal\.nativeEntryAbsent,/u,
  );
  // The honesty boundary is stated where the port is defined.
  assert.match(join, /NOT IMPLEMENTED IN THIS TREE/u);
  // A client that never resolved the package reports it absent, not installed.
  assert.match(
    join,
    /class UnboundEndpointCollaborationAvailability[\s\S]{0,200}EndpointCollaborationAvailability\.missing\(\)/u,
  );
});

test("the join is production-reachable from the relay feature composition", () => {
  assert.match(join, /factory DeviceEntryJoin\.production\(\{/u);
  assert.match(featureComposition, /late final DeviceEntryJoin deviceEntry;/u);
  assert.match(featureComposition, /deviceEntry = DeviceEntryJoin\.production\(/u);
  assert.match(featureComposition, /await deviceEntry\.dispose\(\);/u);
  // The relay half is the mounted feature's own binding, published unchanged.
  assert.match(join, /required this\.relay,/u);
  assert.match(featureComposition, /relay: binding,/u);
});

test("the new sources invent no native command and reach no credential", () => {
  for (const [path, sourceText] of [
    [PROJECTION, projection],
    [POLICY, policy],
    [SOURCE, source],
    [JOIN, join],
  ]) {
    assert.doesNotMatch(sourceText, /dart:io|package:http|Process\./u, path);
    assert.doesNotMatch(
      sourceText,
      /keychain|SecretStore|SecretBytes|Platform\.is/u,
      path,
    );
    assert.doesNotMatch(sourceText, /secure_mesh\.[a-z]/u, path);
    // No caller-supplied authority flag is read as authority.
    assert.doesNotMatch(sourceText, /authorizedByCaller|callerSupplied/u, path);
  }
});

test("the synthetic evidence is labelled as synthetic and drives both rules", () => {
  assert.match(fixtures, /synthetic/iu);
  assert.match(fixtures, /are not implemented in this tree/u);
  assert.match(fixtures, /SyntheticDeviceReplacementControl/u);
  assert.match(read(PROJECTION_TEST), /reviewErase|submitEraseConfirmation/u);
  assert.match(read(JOIN_TEST), /nativeEntryAbsent/u);
});
