#!/usr/bin/env node
import { renameSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { allowedSources, parseJson, privacyFindings, redactionSecrets, transcriptHash } from "./shared.mjs";

const args = process.argv.slice(2);
const input = args.find((entry) => !entry.startsWith("--"));
const required = [
  "--approve",
  "--synthetic-task-confirmed",
  "--no-user-conversation",
  "--paths-and-identity-checked",
  "--frames-match-capture",
  "--projections-match-parser",
];
if (!input || required.some((flag) => !args.includes(flag))) {
  throw new Error(`usage: review.mjs <candidate.json> ${required.join(" ")}`);
}
const path = resolve(input);
const document = parseJson(path);
if (!allowedSources.includes(document.provenance?.source)) throw new Error("transcript_provenance_invalid");
if (document.redaction?.contentSha256 !== transcriptHash(document)) throw new Error("redacted_content_hash_mismatch");
const findings = privacyFindings(document, redactionSecrets());
if (findings.length > 0) throw new Error("redaction_scan_failed");
document.provenance.humanReviewed = true;
document.review = {
  status: "approved",
  reviewerClass: "human",
  checklist: {
    syntheticTaskConfirmed: true,
    noUserConversation: true,
    pathsAndIdentityChecked: true,
    framesMatchProtocolCapture: true,
    projectionsMatchParserOutput: true,
  },
};
const temporary = `${path}.reviewed`;
writeFileSync(temporary, `${JSON.stringify(document, null, 2)}\n`, { flag: "wx" });
renameSync(temporary, path);
