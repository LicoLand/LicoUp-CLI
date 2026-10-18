// Recorded vendor frames for the replay corpus.
//
// A frame is what the adapter's own parser consumes: the vendor's wire bytes
// for that adapter's framing. Payloads are written as objects (serialized in
// insertion order) or as raw strings when the recorded wire is deliberately
// malformed, which is how the interruption scenarios reach the parser's own
// framing failure. Nothing here describes parser output: projections are
// recorded from the real parsers by `adapter_replay_record_projections`.
import { acpFrames } from "./protocol-frames/acp.mjs";
import { ptyJsonlFrames } from "./protocol-frames/pty-jsonl.mjs";
import { serveFrames } from "./protocol-frames/serve.mjs";
import { streamJsonFrames } from "./protocol-frames/stream-json.mjs";

const tables = [streamJsonFrames, acpFrames, serveFrames, ptyJsonlFrames];

export function recordedAdapterIds() {
  return tables.flatMap((table) => Object.keys(table)).sort();
}

export function protocolFrames(adapterId, scenario) {
  for (const table of tables) {
    const adapter = table[adapterId];
    if (!adapter) continue;
    const payloads = adapter.scenarios?.[scenario];
    if (!payloads || payloads.length === 0) {
      throw new Error(`protocol_frames_missing:${adapterId}:${scenario}`);
    }
    return payloads.map((entry, index) => {
      const recorded = typeof entry === "string" ? { payload: entry } : entry;
      return {
        index,
        direction: recorded.direction || "agent-to-client",
        channel: adapter.channel,
        payload: typeof entry === "string" ? entry : JSON.stringify(entry),
        projection: null,
      };
    });
  }
  throw new Error(`protocol_frames_adapter_unknown:${adapterId}`);
}
