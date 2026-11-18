import assert from 'node:assert/strict';
import test from 'node:test';
import { usageSamples, createUsageReader } from './deepseek_reader.mjs';
import { createRequire } from 'node:module';
import { mkdtemp, mkdir, writeFile, appendFile, readFile, rm, readdir } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { pathToFileURL } from 'node:url';
import { zstdCompressSync, constants } from 'node:zlib';
import { spawnSync } from 'node:child_process';

const lastUsage = stream => stream?.findLast(record => record.chunk?.type === 'usage')?.chunk;
const event = (seq, type, data) => ({ seq, time: 1784109600000 + seq, type, data });
const sample = (seq, usage, message = {}) => ({ ...event(seq, 'assistant/message', { turn: 1, step: 0, usage, stream: [], message }), surfaceOp: 'append' });
const config = { model: 'deepseek-native', provider: 'deepseek-official', reasoningEffort: 'high' };

test('settlements replace one attempt, retries count once each, inherited prefixes do not count', () => {
  const samples = usageSamples([
    event(0, 'request/header', { header: { config } }),
    sample(1, { inputTokens: 900, outputTokens: 1 }),
    sample(2, { inputTokens: 10, outputTokens: 1 }),
    sample(3, { inputTokens: 12, outputTokens: 2 }),
    event(4, 'llm/retry-started', { turn: 1, step: 0 }),
    event(5, 'assistant/attempt', { turn: 1, step: 0, stream: [
      { type: 'chunk', chunk: { type: 'usage', usage: { inputTokens: 3, outputTokens: 1 } } },
      { type: 'chunk', chunk: { type: 'usage', usage: { inputTokens: 4, outputTokens: 2 } } },
    ] }),
  ], 2, lastUsage);
  assert.equal(samples.length, 2);
  assert.deepEqual(samples.map(sample => sample.usage.inputTokens), [12, 4]);
  assert.deepEqual(samples.map(sample => sample.effort), ['high', 'high']);
  assert.deepEqual(samples.map(sample => sample.seq), [3, 5]);
});

test('actual response route wins and missing options never inherit across a header reset', () => {
  const samples = usageSamples([
    event(0, 'request/header', { header: { config, adapterDefaults: { reasoningEffort: 'low' } } }),
    sample(1, { inputTokens: 3, outputTokens: 1 }, { source: { model: 'actual-other', provider: 'other' } }),
    event(2, 'llm/retry-started', { turn: 1, step: 0 }),
    event(3, 'request/header', { header: { config: { model: 'deepseek-native', provider: 'deepseek-official' } } }),
    sample(4, undefined),
  ], 0, lastUsage);
  assert.equal(samples[0].model, 'actual-other');
  assert.equal(samples[0].effort, undefined);
  assert.equal(samples[1].effort, undefined);
  assert.equal(samples[1].usage, undefined);
});

// Optional compatibility exercise imports installed program code only and
// creates all session bytes below a new temporary fixture root.
test('official read-only backend decodes plaintext and zstd and never publishes a generation', {
  skip: !process.env.LICOUP_DSH_TEST_ENTRY,
}, async () => {
  const entry = process.env.LICOUP_DSH_TEST_ENTRY;
  const require = createRequire(pathToFileURL(entry));
  const { sessionFormatCatalog: catalog } = await import(pathToFileURL(require.resolve('@deepseek-ai/dsh-session-format-catalog')));
  const root = await mkdtemp(join(tmpdir(), 'licoup-harness-fixture-'));
  try {
    for (const compressed of [false, true]) {
      const directory = join(root, compressed ? 'compressed' : 'plain', '_no-cwd', 'fixture');
      await mkdir(directory, { recursive: true });
      const header = catalog.encodeCurrentHeader({ version: catalog.currentVersion, id: 'fixture', createdAt: 1784109600000, isSeeded: false, delegationDepth: 0 }, 0);
      const events = [
        event(0, 'request/header', { reason: 'initial', header: { config } }),
        sample(1, { inputTokens: 70, cacheReadTokens: 30, outputTokens: 20, reasoningTokens: 15 }, { id: 'synthetic-message', role: 'assistant', content: [], source: { kind: 'model', provider: config.provider, model: config.model } }),
      ];
      const text = [header, ...events.map(event => catalog.encodeCurrentEvent(event))].map(row => JSON.stringify(row) + '\n');
      const bytes = compressed ? Buffer.concat(text.map(row => zstdCompressSync(Buffer.from(row), { params: { [constants.ZSTD_c_checksumFlag]: 1 } }))) : Buffer.from(text.join(''));
      const file = join(directory, `session.v${catalog.currentVersion}.jsonl${compressed ? '.zstd' : ''}`);
      await writeFile(file, bytes);
      const before = await readdir(directory);
      const read = await createUsageReader(entry);
      const first = await read(file);
      assert.equal(first.samples.length, 1);
      assert.equal(first.samples[0].usage.totalTokens, undefined);
      assert.equal(first.samples[0].usage.cacheWriteTokens, undefined);
      assert.deepEqual(await read(file), first);
      await appendFile(file, compressed ? Buffer.from([0x28, 0xb5]) : Buffer.from('{"type":'));
      assert.deepEqual(await read(file), first);
      assert.deepEqual(await readdir(directory), before);
      const { verify } = await import('../../../../../../tools/local/verify-deepseek-usage.mjs');
      const report = await verify(entry, join(root, compressed ? 'compressed' : 'plain'), '2026-07-15', '2026-07-15', 480);
      assert.equal(report.totalTokens, 120);
      assert.equal(report.requestCount, 1);
      assert.equal(report.reasoningTokens, 15);
      assert.equal(report.deduplication.uniqueKeys, 1);
      assert.equal(report.failedSources, 0);
      assert.equal(JSON.stringify(report).includes(directory), false);
      if (compressed) {
        const source = await readFile(new URL('./deepseek_reader.mjs', import.meta.url), 'utf8');
        const worker = spawnSync(process.execPath, ['--input-type=module', '--eval', source, '--', '--licoup-deepseek-usage', entry], {
          input: [file, file, join(root, 'missing', 'session.v3.jsonl.zstd')].map(path => JSON.stringify({ path }) + '\n').join(''), encoding: 'utf8',
        });
        assert.equal(worker.status, 0);
        const replies = worker.stdout.trim().split('\n').map(line => JSON.parse(line));
        assert.equal(replies.length, 3);
        assert.deepEqual(replies[0].ok, first);
        assert.deepEqual(replies[1].ok, first);
        assert.deepEqual(replies[2], { error: 'deepseek_usage_decode_failed' });
        assert.equal(worker.stderr, '');
      }
    }
  } finally { await rm(root, { recursive: true, force: true }); }
});
