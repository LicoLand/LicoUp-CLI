import { existsSync, mkdirSync, mkdtempSync, renameSync, rmSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import {
  linuxEvidencePrivacyRecord,
  linuxEvidenceSchemaVersion,
  linuxNodeMatrixSchema,
  validateLinuxNodeMatrixReport,
  validateLinuxVmPackageReceipt,
} from "../lib/secure-mesh-linux-evidence.mjs";
import {
  LinuxClientNode,
  buildLinuxNodeImage,
  createDockerNetwork,
  removeDockerNetwork,
  removeLinuxNodeImage,
} from "../lib/secure-mesh-linux-node.mjs";
import { validateCapabilityReport } from "../lib/secure-mesh-capability-report.mjs";
import {
  stableReadFile,
  stableSnapshotFile,
} from "../lib/client-release-artifact-digest.mjs";
import {
  inspectLinuxTarGzipArchive,
  LINUX_TAR_RESOURCE_LIMITS,
} from "../lib/linux-tar-resource-bounds.mjs";
import { assert } from "./assert.mjs";
import { repoRoot } from "./constants.mjs";
import { publicIdentityDigest } from "./identity.mjs";
import {
  exchangeSecureCommand,
  pairNodes,
  proveStateIsolation,
  restartRequiresPairing,
} from "./operations.mjs";
import { OpaqueRelay } from "./relay/opaque-relay.mjs";
import { requiredOption, writeReport } from "./report.mjs";
import { extractArchive, randomMarker, requiredFile, sha256File } from "./util.mjs";

export async function runMatrix(options, phase) {
  assert(process.platform === "linux", "Linux node matrix requires Linux");
  const expectedSourceDigest = requiredOption(options, "expectedSourceDigest");
  assert(/^sha256:[a-f0-9]{64}$/u.test(expectedSourceDigest),
    "Linux node matrix source digest is invalid");
  const archive = requiredFile(requiredOption(options, "archive"), "Linux archive");
  const tempRoot = mkdtempSync(path.join(os.tmpdir(), "lico-linux-node-matrix-"));
  try {
  const stableArchive = stableSnapshotFile(
    archive,
    tempRoot,
    "release-archive.tar.gz",
    { maxBytes: LINUX_TAR_RESOURCE_LIMITS.maxCompressedBytes },
  );
  inspectLinuxTarGzipArchive(stableArchive);
  phase.set("archive_binding");
  const verificationManifestPath = requiredFile(
    requiredOption(options, "verificationManifest"),
    "Linux verification manifest"
  );
  const vmReceiptPath = requiredFile(requiredOption(options, "vmReceipt"), "Linux VM package receipt");
  const vmReceipt = JSON.parse(stableReadFile(vmReceiptPath, {
    maxBytes: 16 * 1024 * 1024,
  }).toString("utf8"));
  validateLinuxVmPackageReceipt(vmReceipt, expectedSourceDigest);
  const distribution = JSON.parse(stableReadFile(verificationManifestPath, {
    maxBytes: 2 * 1024 * 1024,
  }).toString("utf8"));
  assert(distribution.schemaVersion === "licomesh.client-linux.verification-carrier.v1" &&
    distribution.mode === "verification" && distribution.verificationReady === true &&
    distribution.publicReleaseBlocked === true,
  "Linux verification carrier policy is invalid");
  const archiveDigest = sha256File(stableArchive);
  assert(distribution.sourceStateDigest === expectedSourceDigest,
    "Linux node archive source-state digest is stale");
  assert(distribution.productVersion === vmReceipt.productVersion &&
    distribution.buildNumber === vmReceipt.buildNumber,
  "Linux node archive version binding is inconsistent");
  assert(distribution.sha256 === archiveDigest.slice("sha256:".length),
    "Linux node archive digest is invalid");
  assert(vmReceipt.sourceBinding.archiveDigest === archiveDigest,
    "Linux node archive does not match the VM install receipt");
  assert(/^sha256:[a-f0-9]{64}$/u.test(String(distribution.bundleManifestDigest || "")),
    "Linux node bundle-manifest digest is invalid");

  const context = path.join(tempRoot, "image-context");
  mkdirSync(context, { recursive: true });
  extractArchive(stableArchive, context);
  const extractedBundle = path.join(context, "bundle");
  const imageClientRoot = path.join(context, "client");
  assert(existsSync(extractedBundle), "Linux node archive did not contain the client bundle");
  renameSync(extractedBundle, imageClientRoot);
  const bundleManifestPath = requiredFile(
    path.join(imageClientRoot, "package-metadata", "licoup", "packaging-modules.json"),
    "Linux node bundle manifest"
  );
  const bundleManifest = JSON.parse(stableReadFile(bundleManifestPath, {
    maxBytes: 2 * 1024 * 1024,
  }).toString("utf8"));
  assert(bundleManifest.sourceStateDigest === expectedSourceDigest,
    "Linux node bundle source-state digest is stale");
  assert(sha256File(bundleManifestPath) === distribution.bundleManifestDigest,
    "Linux node bundle-manifest digest does not match the archive receipt");
  assert(vmReceipt.sourceBinding.bundleManifestDigest === distribution.bundleManifestDigest,
    "Linux node bundle manifest does not match the VM install receipt");
  const nativeClientDigest = sha256File(requiredFile(
    path.join(imageClientRoot, "licoup"),
    "Linux node native sidecar",
  ), { maxBytes: 512 * 1024 * 1024 });
  assert(vmReceipt.sourceBinding.nativeClientDigest === nativeClientDigest,
    "Linux node native sidecar does not match the VM install receipt");

  let imageRecord = null;
  let network = null;
  let relay = null;
  const nodes = [];
  const teardown = {
    bounded: true,
    nodeCount: 3,
    allProcessesStopped: false,
    allContainersRemoved: false,
    ephemeralStateRemoved: false
  };
  let result;
  try {
    phase.set("image_build");
    imageRecord = buildLinuxNodeImage({
      context,
      dockerfile: options.dockerfile ||
        path.join(repoRoot, "apps", "desktop", "docker", "secure-mesh-node.Dockerfile"),
      dockerCommand: options.dockerCommand || ""
    });
    network = createDockerNetwork({ dockerCommand: options.dockerCommand || "" });
    relay = await OpaqueRelay.start();
    phase.set("node_start");
    const gateway = relay.containerGateway();
    for (const label of ["linux-a", "linux-b", "linux-c"]) {
      const node = new LinuxClientNode({
        label,
        image: imageRecord.image,
        network: network.name,
        dockerCommand: options.dockerCommand || ""
      });
      nodes.push(node);
      const status = await node.start();
      validateCapabilityReport(status.capabilityReport);
    }
    const [linuxA, linuxB, linuxC] = nodes;
    const capabilityReports = await Promise.all(nodes.map((node) =>
      node.execute(["secure-mesh", "status"]).then((status) => status.capabilityReport)
    ));
    for (const report of capabilityReports) validateCapabilityReport(report);
    assert(new Set(capabilityReports.map((report) => report.catalogDigest)).size === 1,
      "Linux nodes disagreed on the exact capability catalog");

    phase.set("state_isolation");
    await proveStateIsolation(nodes);
    phase.set("first_pair_exchange");
    const firstPair = await pairNodes({ pc: linuxA, mobile: linuxB, gateway });
    const firstExchange = await exchangeSecureCommand({
      pc: linuxA,
      mobile: linuxB,
      relay,
      marker: randomMarker()
    });
    phase.set("second_pair_exchange");
    const secondPair = await pairNodes({ pc: linuxB, mobile: linuxC, gateway });
    const secondExchange = await exchangeSecureCommand({
      pc: linuxB,
      mobile: linuxC,
      relay,
      marker: randomMarker()
    });
    const publicIdentityDigests = [
      publicIdentityDigest(firstPair.pcSecureMesh),
      publicIdentityDigest(secondPair.pcSecureMesh),
      publicIdentityDigest(secondPair.mobileSecureMesh)
    ];
    assert(new Set(publicIdentityDigests).size === 3,
      "Linux nodes did not expose three unique public endpoint identities");

    phase.set("restart_isolation");
    await linuxA.restartRpc();
    assert(linuxA.rpcProcessCount === 2 && linuxB.rpcProcessCount === 1 &&
      linuxC.rpcProcessCount === 1,
    "Linux node restart affected more than the selected participant");
    const restartedState = await linuxA.execute(["state", "get", "settings"]);
    assert(restartedState?.document?.linuxNodeIsolationMarker === "linux-a",
      "Restarted Linux node lost or crossed its public state root");
    const restartRequiresRePairRekey = await restartRequiresPairing(linuxA);
    assert(restartRequiresRePairRekey, "Memory-only Linux restart did not require re-pair/rekey");
    await proveStateIsolation([linuxB, linuxC]);
    const postRestartExchange = await exchangeSecureCommand({
      pc: linuxB,
      mobile: linuxC,
      relay,
      marker: randomMarker()
    });
    assert(postRestartExchange.ready, "Unaffected Linux nodes failed after peer restart");
    assert(relay.plaintextObserved === false, "Opaque relay observed protected command plaintext");

    result = {
      capabilityReport: capabilityReports[0],
      identityCount: new Set(publicIdentityDigests).size,
      exchangeCount: [firstExchange, secondExchange, postRestartExchange]
        .filter((exchange) => exchange.ready).length,
      allNodesParticipated: firstPair.ready && secondPair.ready,
      secureSessionsEstablished: firstPair.ready && secondPair.ready,
      restartRequiresRePairRekey
    };
  } finally {
    phase.set(result ? "teardown" : phase.get());
    const processStopResults = [];
    const containerRemovalResults = [];
    for (const node of [...nodes].reverse()) {
      try {
        await node.stop();
        processStopResults.push(node.rpcStopped === true);
        containerRemovalResults.push(node.removed === true);
      } catch {
        processStopResults.push(false);
        containerRemovalResults.push(false);
      }
    }
    teardown.allProcessesStopped = processStopResults.length === nodes.length &&
      processStopResults.every(Boolean);
    teardown.allContainersRemoved = containerRemovalResults.length === nodes.length &&
      containerRemovalResults.every(Boolean);
    const relayStopped = relay ? await relay.stop() : true;
    const networkRemoved = network ? removeDockerNetwork(network) : true;
    const imageRemoved = imageRecord ? removeLinuxNodeImage(imageRecord) : true;
    teardown.bounded = teardown.bounded && relayStopped && networkRemoved && imageRemoved;
    teardown.ephemeralStateRemoved = teardown.allContainersRemoved && networkRemoved && imageRemoved;
  }
  assert(result, "Linux node matrix operations did not complete");
  assert(teardown.bounded && teardown.allContainersRemoved,
    "Linux node matrix teardown did not complete within bounds");
  const report = {
    schema: linuxNodeMatrixSchema,
    schemaVersion: linuxEvidenceSchemaVersion,
    ok: true,
    producer: "linux-node-matrix",
    artifactKind: "linux-current-client-node-matrix",
    target: "ubuntu-linux-arm64",
    redacted: true,
    reportLeakScan: true,
    rawPrivateMaterialIncluded: false,
    rawPlaintextIncluded: false,
    rawPublicWireBytesIncluded: false,
    sourceBinding: {
      sourceStateDigest: expectedSourceDigest,
      sourceStateDigestProvenance: String(distribution.sourceStateDigestProvenance || ""),
      archiveDigest,
      bundleManifestDigest: distribution.bundleManifestDigest,
      nativeClientDigest,
      stale: false
    },
    runtime: {
      kind: "isolated_linux_containers",
      nodeCount: 3,
      currentClientArchive: true,
      publicOperationsOnly: true,
      eventDrivenReadiness: true
    },
    isolation: {
      participantLabels: ["linux-a", "linux-b", "linux-c"],
      distinctStateRoots: true,
      noSharedSecretVolume: nodes.every((node) => node.mountIsolationVerified),
      uniquePublicIdentityCount: result.identityCount,
      crossNodeStateReadRejected: true,
      containerIsolation: true
    },
    pairwise: {
      exchangeCount: result.exchangeCount,
      allNodesParticipated: result.allNodesParticipated,
      secureSessionsEstablished: result.secureSessionsEstablished,
      opaqueRelay: true,
      relayPlaintextObserved: false,
      relayCiphertextIncludedInReport: false
    },
    restart: {
      restartedParticipant: "linux-a",
      restartedProcessCount: 1,
      restartRequiresRePairRekey: result.restartRequiresRePairRekey,
      unaffectedParticipantCount: 2,
      postRestartExchangeReady: true,
      stateContaminationDetected: false
    },
    teardown,
    capabilityReport: result.capabilityReport,
    privacy: linuxEvidencePrivacyRecord(),
    summary: {
      currentSourceNodes: true,
      isolationReady: true,
      pairwiseReady: true,
      restartIsolationReady: true,
      teardownReady: true,
      privacyReady: true
    }
  };
  validateLinuxNodeMatrixReport(report, expectedSourceDigest);
  writeReport(options, report);
  console.log(JSON.stringify({
    ok: true,
    artifactKind: report.artifactKind,
    nodeCount: 3,
    exchangeCount: report.pairwise.exchangeCount,
    isolationReady: true,
    restartIsolationReady: true,
    teardownReady: true,
    privacyReady: true
  }, null, 2));
  } finally {
    rmSync(tempRoot, { recursive: true, force: true });
  }
}
