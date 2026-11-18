// Only the installed persistence/LLM libraries are loaded. No profile, provider,
// environment file, runtime or credential resolver is initialized.
import { createRequire } from 'node:module';
import { realpathSync } from 'node:fs';
import { basename, dirname, join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { createInterface } from 'node:readline';

export function usageSamples(events, inheritedEventCount, lastUsage) {
  const samples = [];
  let header;
  let last;
  for (const event of events) {
    const data = event.data;
    if (event.type === 'request/header') header = data.header;
    if (event.seq < inheritedEventCount) continue;
    if (event.type === 'llm/retry-started') {
      if (last?.turn === data.turn && last.step === data.step) last = undefined;
      continue;
    }
    if (event.type !== 'assistant/message' && event.type !== 'assistant/attempt') continue;
    const config = header?.config;
    const source = data.message?.source ?? config;
    const sameRoute = source?.model === config?.model && source?.provider === config?.provider;
    const sample = {
      seq: event.seq,
      messageId: data.message?.id,
      time: event.time,
      turn: data.turn,
      step: data.step,
      model: source?.model,
      provider: source?.provider,
      effort: sameRoute ? config?.reasoningEffort ?? header?.adapterDefaults?.reasoningEffort : undefined,
      usage: event.type === 'assistant/message' && data.usage !== undefined
        ? data.usage : lastUsage(data.stream, 'usage')?.usage,
    };
    // Match token-meter's replacement slot: repeated samples of one attempt
    // replace; a recorded retry starts a separately billed attempt.
    if (last?.turn === data.turn && last.step === data.step) {
      // A later tokenless settlement does not erase an earlier reported sample.
      if (sample.usage !== undefined || samples[last.index].usage === undefined) samples[last.index] = sample;
    }
    else {
      samples.push(sample);
      last = { turn: data.turn, step: data.step, index: samples.length - 1 };
    }
  }
  return samples;
}

export async function createUsageReader(entry) {
  const resolved = realpathSync(entry);
  const require = createRequire(pathToFileURL(resolved));
  const load = name => import(pathToFileURL(require.resolve(name, {
    paths: [dirname(resolved), join(dirname(resolved), 'node_modules/@deepseek-ai/dsh')],
  })));
  const [{ Context }, { default: Persistence }, { lastAssistantStreamChunk }] = await Promise.all([
    load('@deepseek-ai/cordis'), load('@deepseek-ai/dsh-session-persistence-jsonl'), load('@deepseek-ai/dsh-llm'),
  ]);
  const stores = new Map();
  return async path => {
  const directory = dirname(path);
  // The canonical store escapes individual UTF-16 code units as ~XXXX.
  const id = basename(directory).replace(/~([0-9A-F]{4})/g, (_, code) => String.fromCharCode(parseInt(code, 16)));
  const root = dirname(dirname(directory));
  const compression = path.endsWith('.zstd') ? 'zstd' : 'none';
  const key = JSON.stringify([root, compression]);
  let storage = stores.get(key);
  if (!storage) {
    storage = new Persistence(new Context(), { root, compression });
    stores.set(key, storage);
  }
  const handle = await storage.open(id, 'read');
  try {
    const { events } = await handle.read();
    return { samples: usageSamples(events, handle.inheritedEventCount, lastAssistantStreamChunk) };
  } finally {
    await handle.close();
  }
  };
}

if (process.argv[1] === '--licoup-deepseek-usage') {
  try {
    const readUsage = await createUsageReader(process.argv[2]);
    for await (const line of createInterface({ input: process.stdin, crlfDelay: Infinity })) {
      try {
        const { path } = JSON.parse(line);
        process.stdout.write(JSON.stringify({ ok: await readUsage(path) }) + '\n');
      } catch {
        process.stdout.write('{"error":"deepseek_usage_decode_failed"}\n');
      }
    }
  } catch {
    // Backend errors may contain source paths or event bodies.
    process.stderr.write('deepseek_usage_read_failed\n');
    process.exitCode = 1;
  }
}
