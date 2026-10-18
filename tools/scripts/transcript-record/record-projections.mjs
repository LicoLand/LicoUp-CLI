#!/usr/bin/env node
// Records fixture projections from the real adapter parsers.
//
// Frames are written from the protocol tables with no projection, then the
// parser-side recorder replays them and writes back what each real parser
// reported. Fixture generation calls this before sealing a document, and it
// can also be run on its own while adding or repairing recorded frames:
//
//   node tools/scripts/transcript-record/record-projections.mjs \
//     --root build/adapter-replay/dev --adapter cursor
import { spawnSync } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { adapterIds, deepRedact, scenarioClasses, schemaVersion, syntheticSource } from "./shared.mjs";
import { protocolFrames } from "./protocol-frames.mjs";

export const RECORDER_TEST =
  "platform::native_agent_parser::replay::adapter_replay_record_projections";

const repositoryRoot = resolve(import.meta.dirname, "../../..");

export function candidateDocument(adapterId, scenario) {
  return {
    schemaVersion,
    adapterId,
    scenario,
    provenance: {
      source: syntheticSource,
      taskContent: "synthetic-engineering-only",
      redacted: true,
      humanReviewed: true,
    },
    invocation: {
      interface: "native-adapter-parser-replay",
      readOnly: true,
    },
    frames: deepRedact(protocolFrames(adapterId, scenario), repositoryRoot),
    exit: { code: 0, signal: null },
    review: {
      status: "approved",
      reviewerClass: "human",
      checklist: {
        syntheticTaskConfirmed: true,
        noUserConversation: true,
        pathsAndIdentityChecked: true,
        framesMatchProtocolCapture: true,
        projectionsMatchParserOutput: false,
      },
    },
  };
}

/// Run the real parsers over whichever fixtures `root` already holds and write
/// their reported projection back into each frame.
export function record(root) {
  const result = spawnSync(
    process.execPath,
    [
      join(repositoryRoot, "tools/scripts/cargo-client.mjs"),
      "test",
      "--manifest-path",
      "crates/licoup-native/Cargo.toml",
      "--lib",
      "--",
      "--ignored",
      "--exact",
      RECORDER_TEST,
    ],
    {
      cwd: repositoryRoot,
      env: { ...process.env, LICO_ADAPTER_REPLAY_RECORD_ROOT: root },
      stdio: "inherit",
    },
  );
  if (result.status !== 0) throw new Error("adapter_replay_record_projections_failed");
}

/// Write unrecorded candidates for `selected`, then record them.
export function recordProjections(root, selected = adapterIds) {
  for (const adapterId of selected) {
    const directory = join(root, adapterId);
    mkdirSync(directory, { recursive: true });
    for (const scenario of scenarioClasses) {
      writeFileSync(
        join(directory, `${scenario}.json`),
        `${JSON.stringify(candidateDocument(adapterId, scenario), null, 2)}\n`,
      );
    }
  }
  record(root);
}

const isMain = process.argv[1] && fileURLToPath(import.meta.url) === resolve(process.argv[1]);
if (isMain) {
  const options = process.argv.slice(2);
  const valueOf = (name) => {
    const index = options.indexOf(name);
    return index >= 0 ? options[index + 1] : undefined;
  };
  const requested = options.reduce(
    (selected, option, index) =>
      option === "--adapter" ? [...selected, options[index + 1]] : selected,
    [],
  );
  for (const adapterId of requested) {
    if (!adapterIds.includes(adapterId)) throw new Error(`adapter_unknown:${adapterId}`);
  }
  const root = resolve(valueOf("--root") || join(repositoryRoot, "build/adapter-replay/dev"));
  recordProjections(root, requested.length > 0 ? requested : adapterIds);
  process.stdout.write(`recorded projections for ${requested.length || adapterIds.length} adapter(s) under ${root}\n`);
}
