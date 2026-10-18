#!/usr/bin/env node
import { existsSync, readdirSync, statSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import {
  adapterIds,
  parseJson,
  reviewApproved,
  scenarioClasses,
  transcriptHash,
} from "./shared.mjs";

const repositoryRoot = resolve(import.meta.dirname, "../../..");
const directoryExists = (path) => existsSync(path) && statSync(path).isDirectory();
const developerCorpusRoot = join(repositoryRoot, "tests/replay-corpus");
const syntheticFixtureRoot = join(repositoryRoot, "apps/desktop/test/fixtures/adapter-replay");
const usingDeveloperCorpus = directoryExists(developerCorpusRoot);
const corpusRoot = usingDeveloperCorpus ? developerCorpusRoot : syntheticFixtureRoot;
const replaySource = usingDeveloperCorpus ? "tests/replay-corpus" : "apps/desktop/test/fixtures/adapter-replay";
const e2ePath = join(repositoryRoot, "crates/licoup-native/resources/agent-conversation-evidence.json");
const outputPath = join(repositoryRoot, "crates/licoup-native/resources/agent-adapter-tiers.json");
const e2e = new Map((parseJson(e2ePath).adapters || []).map((entry) => [entry.agentId, entry]));

const adapters = adapterIds.map((agentId) => {
  const adapterDir = join(corpusRoot, agentId);
  const files = directoryExists(adapterDir)
    ? new Set(readdirSync(adapterDir).filter((name) => name.endsWith(".json")))
    : new Set();
  const validDocuments = new Map();
  const replayScenarios = scenarioClasses.filter((scenario) => {
    const name = `${scenario}.json`;
    if (!files.has(name)) return false;
    const document = parseJson(join(corpusRoot, agentId, name));
    const valid = document.adapterId === agentId
      && document.scenario === scenario
      && document.redaction?.contentSha256 === transcriptHash(document);
    if (valid) validDocuments.set(scenario, document);
    return valid;
  });
  const reviewedReplayScenarios = replayScenarios.filter((scenario) =>
    reviewApproved(validDocuments.get(scenario)));
  const replayCovered = reviewedReplayScenarios.length === scenarioClasses.length;
  const evidence = e2e.get(agentId);
  const e2eCovered = evidence?.officialNativeLane === true
    && evidence?.conversationGatePassed === true
    && evidence?.cleanupPassed === true
    && evidence?.privacyPassed === true;
  return {
    agentId,
    tier: replayCovered && e2eCovered ? "first-class" : "best-effort",
    derivedFrom: {
      replayCovered,
      replayScenarios,
      reviewedReplayScenarios,
      e2eCovered,
      e2eEvidencePresent: Boolean(evidence),
    },
  };
});

const document = {
  schemaVersion: "lico.agent-adapter-tier-projection.v1",
  derivation: "first-class requires complete human-reviewed replay scenario coverage and passing native e2e evidence; otherwise best-effort",
  sources: [
    replaySource,
    "agent-conversation-evidence.json",
  ],
  adapters,
};
writeFileSync(outputPath, `${JSON.stringify(document, null, 2)}\n`);
process.stdout.write(`derived tiers for ${adapters.length} adapters\n`);
