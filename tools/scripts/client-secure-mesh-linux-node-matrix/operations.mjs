import { assert } from "./assert.mjs";

export async function proveStateIsolation(nodes) {
  for (const node of nodes) {
    await node.execute([
      "state",
      "set",
      "settings",
      JSON.stringify({ linuxNodeIsolationMarker: node.label })
    ]);
  }
  for (const node of nodes) {
    const state = await node.execute(["state", "get", "settings"]);
    assert(state?.document?.linuxNodeIsolationMarker === node.label,
      "Linux node public state crossed an endpoint boundary");
  }
}

export async function pairNodes({ pc, mobile, gateway }) {
  await configureForRelay(pc, gateway, true);
  await configureForRelay(mobile, gateway, true);
  const pairing = await pc.execute(["mobile", "relay", "pairing", "create"]);
  const invite = pairing?.mobileRelayPairingInvite;
  assert(invite && typeof invite === "object", "Linux pairing create omitted its invite");
  const claimed = await mobile.execute([
    "mobile",
    "relay",
    "pairing",
    "claim",
    "--invite-json",
    JSON.stringify(invite),
    "--mobile-device-name",
    mobile.label,
    "--platform",
    "linux"
  ]);
  await pc.execute(["mobile", "relay", "pairing", "status"]);
  await mobile.execute(["mobile", "relay", "pairing", "status"]);
  const pcStatus = await pc.execute(["mobile", "relay", "e2ee", "status"]);
  const mobileStatus = await mobile.execute(["mobile", "relay", "e2ee", "status"]);
  assert(pcStatus?.secureSessionEstablished === true &&
    mobileStatus?.secureSessionEstablished === true,
  "Linux pairwise secure session was not established");
  return {
    ready: claimed?.ok === true,
    pcSecureMesh: pairing?.pairing?.pc?.secureMesh || invite.pcSecureMesh,
    mobileSecureMesh: claimed?.pairing?.mobile?.secureMesh
  };
}

export async function configureForRelay(node, gateway, reset) {
  await node.execute([
    "mobile",
    "relay",
    "config",
    "set",
    "--station-base-url",
    gateway,
    "--reset-pairing",
    reset ? "true" : "false",
    "--relay-enabled",
    "true",
    "--pc-client-id",
    node.label,
    "--pc-client-name",
    node.label
  ]);
}

export async function exchangeSecureCommand({ pc, mobile, relay, marker }) {
  relay.observeMarker(marker);
  const created = await mobile.execute([
    "mobile",
    "relay",
    "commands",
    "create-secure",
    "--command-kind",
    "client.activity.sync",
    "--workspace-id",
    "default",
    "--body",
    JSON.stringify({ limit: 1, nodeMatrixMarker: marker })
  ]);
  const commandId = String(created?.command?.commandId || "");
  assert(commandId, "Linux pairwise exchange did not create a command");
  const synced = await pc.execute(["mobile", "relay", "commands", "sync"]);
  const opened = await mobile.execute([
    "mobile",
    "relay",
    "commands",
    "result-secure",
    "--command-id",
    commandId
  ]);
  assert(opened?.ok === true && opened?.openedResult,
    "Linux pairwise exchange did not open the protected result");
  assert(Array.isArray(synced?.completed) && synced.completed.some((entry) => entry?.ok === true),
    "Linux pairwise exchange did not complete through the public operation");
  assert(relay.plaintextObserved === false, "Opaque relay observed the Linux exchange plaintext");
  return { ready: true };
}

export async function restartRequiresPairing(node) {
  try {
    const status = await node.execute(["mobile", "relay", "e2ee", "status"]);
    return status?.secureSessionEstablished === false &&
      status?.secretStore?.capabilityReport?.custody?.restartSemantics ===
        "re_pair_rekey_after_restart" &&
      Array.isArray(status?.blockers) &&
      status.blockers.includes("safe_secret_custody_not_operational");
  } catch {
    return false;
  }
}
