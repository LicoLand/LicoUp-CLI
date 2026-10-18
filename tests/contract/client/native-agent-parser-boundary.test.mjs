import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

// Each Agent owns its parser, declaration and replay arm. The host composes
// all thirteen package registrations through the shared adapter SDK.
const parserRoot = 'crates/licoup-native/src/platform/native_agent_parser';
const compositionRoot = `${parserRoot}/adapters`;
const sdkRoot = 'crates/licoup-agent-adapter-sdk/src';

// Every adapter this composition carries, in `RuntimeAdapter` order.
const adapters = [
  'antigravity',
  'claude_code',
  'codex',
  'copilot',
  'cursor',
  'hermes',
  'kilo_code',
  'kimi_code',
  'openclaw',
  'opencode',
  'pi',
  'lico_agent',
  'deepseek_harness',
];
// The Agents whose protocol is a package's own: their parser module, their
// declaration and their replay arm live in the package's crate. Every fact the
// boundary reads for one of them is named here once, as a literal — the crate,
// the module inside that crate which exposes the parser, the source publishing
// the declaration, the contract id that declaration carries, and whether this
// composition reads that parser. Nothing below is derived by rewriting a name:
// `kimi_code` is `licoup_agent_kimi` because this table says so, and the crate
// whose parser module sits deeper than `parser` writes that path here.
const packaged = {
  antigravity: {
    crate: 'licoup_agent_antigravity',
    module: 'parser',
    source: 'crates/licoup-agent-antigravity/src/parser.rs',
    contractId: 'antigravity',
    readsParser: false,
  },
  claude_code: {
    crate: 'licoup_agent_claude_code',
    module: 'protocol::parser',
    source: 'crates/licoup-agent-claude-code/src/protocol/parser.rs',
    contractId: 'claude-code',
    readsParser: false,
  },
  codex: {
    crate: 'licoup_agent_codex',
    module: 'parser',
    source: 'crates/licoup-agent-codex/src/parser.rs',
    contractId: 'codex',
    readsParser: false,
  },
  copilot: {
    crate: 'licoup_agent_copilot',
    module: 'parser',
    source: 'crates/licoup-agent-copilot/src/parser.rs',
    contractId: 'copilot',
    readsParser: false,
  },
  cursor: {
    crate: 'licoup_agent_cursor',
    module: 'parser',
    source: 'crates/licoup-agent-cursor/src/parser.rs',
    contractId: 'cursor',
    readsParser: true,
  },
  deepseek_harness: {
    crate: 'licoup_agent_deepseek',
    module: 'parser',
    source: 'crates/licoup-agent-deepseek/src/parser.rs',
    contractId: 'deepseek-harness',
    readsParser: false,
  },
  kimi_code: {
    crate: 'licoup_agent_kimi',
    module: 'parser',
    source: 'crates/licoup-agent-kimi/src/parser.rs',
    contractId: 'kimi-code',
    readsParser: false,
  },
  hermes: {
    crate: 'licoup_agent_hermes',
    module: 'parser',
    source: 'crates/licoup-agent-hermes/src/parser.rs',
    contractId: 'hermes',
    readsParser: false,
  },
  kilo_code: {
    crate: 'licoup_agent_kilo',
    module: 'parser',
    source: 'crates/licoup-agent-kilo/src/parser/mod.rs',
    contractId: 'kilo-code',
    readsParser: false,
  },
  lico_agent: {
    crate: 'licoup_agent_lico_agent',
    module: 'parser',
    source: 'crates/licoup-agent-lico-agent/src/parser.rs',
    contractId: 'lico-agent',
    readsParser: false,
  },
  openclaw: {
    crate: 'licoup_agent_openclaw',
    module: 'parser',
    source: 'crates/licoup-agent-openclaw/src/parser.rs',
    contractId: 'openclaw',
    readsParser: false,
  },
  opencode: {
    crate: 'licoup_agent_opencode',
    module: 'parser',
    source: 'crates/licoup-agent-opencode/src/parser.rs',
    contractId: 'opencode',
    readsParser: true,
  },
  pi: {
    crate: 'licoup_agent_pi',
    module: 'parser',
    source: 'crates/licoup-agent-pi/src/parser.rs',
    contractId: 'pi',
    readsParser: false,
  },
};

// The packaged parsers this composition reads, derived from the table so the two
// cannot drift: a parser alias belongs exactly where a production reader reads
// the parser, and an alias kept for an Agent nothing reads is the unused import
// the compiler reports.
const readParserAliases = new Set(
  Object.entries(packaged)
    .filter(([, moved]) => moved.readsParser)
    .map(([adapter]) => adapter),
);

test('packaged adapter registry is bijective with the thirteen-entry inventory', () => {
  const composition = readFileSync(`${compositionRoot}/mod.rs`, 'utf8');
  const registrations = composition.slice(
    composition.indexOf('pub(in crate::platform) static REGISTRATIONS'),
    composition.indexOf('/// The parser registrations this host injects'),
  );
  // Every entry names its Agent's declaration exactly once, and none inherits
  // another Agent's answer. The entries a package owns are that package's own
  // registration constant, counted from the package map rather than restated.
  const hostedEntries =
    (registrations.match(/ParserRegistration::(?:unanswered|new)\(/g) ?? []).length;
  const packageEntries =
    (registrations.match(/licoup_agent_\w+::registration::REGISTRATION/g) ?? []).length;
  assert.equal(hostedEntries + packageEntries, 13);
  assert.equal(packageEntries, Object.keys(packaged).length);
  assert.equal(hostedEntries, 0);
  for (const adapter of adapters) {
    const moved = packaged[adapter];
    assert.ok(moved, `${adapter} must have a package owner`);
    const source = readFileSync(moved.source, 'utf8');
    assert.match(source, /AdapterContract::new/);
    // The package owns the parser, the declaration and the replay arm. The
    // composition reaches that Agent through the package's own crate, names
    // the package's own REGISTRATION constant, may not declare the module, and
    // may not restate the declaration the package publishes.
    assert.ok(composition.includes(`${moved.crate}::`),
      `${adapter} must be reached through ${moved.crate}`);
    assert.match(registrations,
      new RegExp(`${moved.crate}::registration::REGISTRATION`));
    const registration = readFileSync(
      `${moved.source.split('/src/')[0]}/src/registration.rs`, 'utf8');
    const answersQueries = ['antigravity', 'claude_code', 'codex', 'cursor', 'hermes', 'kilo_code']
      .includes(adapter);
    if (answersQueries) {
      const identity = adapter === 'hermes' ? 'no_identity' : 'valid_identity';
      assert.match(registration, new RegExp(
        `ParserRegistration::new\\(CONTRACT, execution_transitions, ${identity}\\)`));
    } else {
      assert.match(registration, /ParserRegistration::unanswered\(CONTRACT\)/);
    }
    assert.doesNotMatch(composition, new RegExp(`mod ${adapter};`));
    // The contract id is the package's, so it is read from the package: the
    // table records it and the composition may not carry a second copy of it.
    assert.match(source, new RegExp(`AdapterContract::new\\("${moved.contractId}"`),
      `${adapter}'s parser publishes the contract id this table records`);
    assert.doesNotMatch(composition,
      new RegExp(`AdapterContract::new\\("${moved.contractId}"`),
      `${adapter}'s declaration is the package's, not a second one here`);
    // A package alias is the composition naming the package's parser in order
    // to read it. The assertion is bidirectional: the alias is there exactly
    // when the table says a production reader reads that parser, so removing a
    // needed alias and re-adding an unread one both fail here.
    const alias = `use ${moved.crate}::${moved.module} as ${adapter};`;
    assert.equal(composition.includes(alias), readParserAliases.has(adapter),
      `${adapter} composes \`${alias}\` exactly where production reads its parser`);
  }
  // The other direction of the alias rule: every package parser the composition
  // composes is a row of this table that says production reads it. A second name
  // for a package parser — a new alias, or the alias an unread parser would leave
  // behind — fails here even under a name the loop above never asks about.
  const composed = [...composition.matchAll(
    /use\s+(licoup_agent_[a-z_]+)::([A-Za-z_][A-Za-z0-9_:]*?)\s+as\s+([a-z_][A-Za-z0-9_]*)\s*;/g,
  )].map(([, crate, module, alias]) => ({ crate, module, alias }));
  for (const { crate, module, alias } of composed) {
    const moved = packaged[alias];
    assert.ok(moved,
      `the composition composes ${crate}::${module} as ${alias}, which no package row records`);
    assert.deepEqual({ crate: moved.crate, module: moved.module }, { crate, module },
      `${alias} composes the module its package row names`);
    assert.ok(moved.readsParser,
      `${alias} is composed as a package parser but no production reader reads it`);
  }
});


test('the shared adapter contract names no Agent', () => {
  const contract = [
    readFileSync(`${sdkRoot}/adapters/mod.rs`, 'utf8'),
    readFileSync(`${sdkRoot}/registry.rs`, 'utf8'),
    readFileSync(`${sdkRoot}/port.rs`, 'utf8'),
    readFileSync(`${sdkRoot}/lifecycle.rs`, 'utf8'),
    readFileSync(`${sdkRoot}/reconciliation.rs`, 'utf8'),
    readFileSync(`${sdkRoot}/replay/mod.rs`, 'utf8'),
  ].join('\n');
  for (const adapter of adapters) {
    assert.doesNotMatch(contract, new RegExp(`\\b${adapter}\\b`));
  }
});

test('normalized runtime responses cross the typed final parser boundary', () => {
  const normalization = readFileSync(
    'crates/licoup-agent-drivers/src/runtime_adapters/normalization.rs',
    'utf8',
  );
  assert.match(normalization, /execution\s*\.transitions/);
  assert.doesNotMatch(normalization, /native_agent_parser::parse_execution/);
  assert.match(normalization, /"events": transitions/);
  assert.doesNotMatch(normalization, /"events": execution\.events/);
  const parserCore = [
    readFileSync(`${sdkRoot}/adapters/mod.rs`, 'utf8'),
    readFileSync(`${sdkRoot}/registry.rs`, 'utf8'),
  ].join('\n');
  assert.doesNotMatch(parserCore, /ReturnedFrames|DecodePolicy|decode_execution/);

  const service = readFileSync(
    'crates/licoup-native/src/domain/client_conversation/service.rs',
    'utf8',
  );
  assert.match(service, /compose_generated_instruction_delivery/);
  const persistentServer = readFileSync(
    'crates/licoup-native/src/bin/licoup/stdio_rpc/server/conversation.rs',
    'utf8',
  );
  assert.match(persistentServer, /context\.private_instructions\(\)/);
  assert.match(persistentServer, /compose_generated_instruction_delivery/);
});

test('serve HTTP and SSE frames decode only in target parser components', () => {
  const neutralServe = readFileSync(
    'crates/licoup-agent-drivers/src/local_service/serve.rs',
    'utf8',
  );
  assert.doesNotMatch(neutralServe, /message\.updated|message\.part\.updated|serde_json::from_str/);

  for (const adapter of ['opencode', 'kilo_code']) {
    const parser = [packaged[adapter].source, ...(adapter === 'kilo_code'
      ? ['crates/licoup-agent-kilo/src/parser/serve.rs'] : [])]
      .map((source) => readFileSync(source, 'utf8')).join('\n');
    assert.match(parser, /struct ServeEventParser/);
    assert.match(parser, /fn session_id/);
    assert.match(parser, /fn message/);
    assert.match(parser, /message\.part\.updated/);
  }
  const openCodeTransport = readFileSync(
    'crates/licoup-agent-opencode/src/driver/serve_transport.rs',
    'utf8',
  );
  const kiloTransport = readFileSync(
    'crates/licoup-agent-kilo/src/driver/turn.rs',
    'utf8',
  );
  assert.match(openCodeTransport, /crate::parser as serve_parser/);
  assert.match(kiloTransport, /use crate::parser;/);
  assert.match(kiloTransport, /parser::ServeEventParser::new/);
});

test('Cursor PTY isolation precedes its strict NDJSON parser', () => {
  const transport = readFileSync(
    'crates/licoup-agent-cursor/src/driver/io.rs',
    'utf8',
  );
  // The PTY isolation stays in the package's process half; the parser it hands a
  // clean line to is the package's own, and it reads no PTY control at all.
  const parser = readFileSync(
    'crates/licoup-agent-cursor/src/parser.rs',
    'utf8',
  );
  assert.match(transport, /isolate_pty_protocol_line/);
  assert.doesNotMatch(parser, /strip_pty_controls|isolate_pty_protocol_line/);
  assert.match(parser, /serde_json::from_slice/);
});

test('interaction and lifecycle authorities are unbounded and write-once', () => {
  const interaction = readFileSync(
    'crates/licoup-foundation/src/platform/native_agent_interaction/mod.rs',
    'utf8',
  );
  assert.match(interaction, /in-process-one-shot/);
  const productionInteraction = interaction.slice(
    0,
    interaction.indexOf("pub(in crate::platform) fn pending_token"),
  );
  assert.doesNotMatch(productionInteraction, /expires_at|deadline:/i);
  const approvalRoute = readFileSync(
    'crates/licoup-agent-drivers/src/acp_session_transport/approval_store.rs',
    'utf8',
  );
  assert.doesNotMatch(approvalRoute, /PARKED_PERMISSIONS|ParkedPermission/);
  assert.match(approvalRoute, /native_agent_interaction::resolve/);

  // The lifecycle reducer moved to the adapter SDK, which owns it now. The
  // first failure stays write-once, and the stages stay prefix closed: the
  // reducer walks the declared machine from its initial state to the reported
  // one rather than emitting a stage list of its own.
  const lifecycle = readFileSync(`${sdkRoot}/lifecycle.rs`, 'utf8');
  assert.match(lifecycle, /if self\.failure\.is_some\(\)/);
  assert.match(lifecycle, /parser_lifecycle::INITIAL/);
  assert.match(lifecycle, /parser_lifecycle::transition\(current, Event::Advance\)/);
  const machine = readFileSync(
    'crates/licoup-agent-adapter-sdk/resources/state-machines/parser-lifecycle.json',
    'utf8',
  );
  const declared = JSON.parse(machine).machines;
  assert.equal(declared.length, 1);
  assert.deepEqual(declared[0].terminal, ['completed']);
  assert.equal(declared[0].initial, 'submitted');
});
