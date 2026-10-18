import {
  assert,
  test,
  CLIENT_MODULE_CATALOG,
  sourceFiles,
} from "./support.mjs";

test("archive-job modules retain complete leaf ownership and exact command filters", async () => {
  const filters = new Map([
    ["rust.domain.agent-conversations.archive-jobs.composition",
      "domain::conversation_archive_jobs::tests::create"],
    ["rust.domain.agent-conversations.archive-jobs.request",
      "domain::conversation_archive_jobs::tests::request"],
    ["rust.domain.agent-conversations.archive-jobs.plan",
      "domain::conversation_archive_jobs::tests::create::create_requires_the_exact_preview_binding"],
    ["rust.domain.agent-conversations.archive-jobs.activity",
      "domain::conversation_archive_jobs::tests::activity"],
    ["rust.domain.agent-conversations.archive-jobs.creation",
      "domain::conversation_archive_jobs::tests::create"],
    ["rust.domain.agent-conversations.archive-jobs.store-queries",
      "domain::conversation_archive_jobs::tests::reopen"],
    ["rust.domain.agent-conversations.archive-jobs.drain",
      "domain::conversation_archive_jobs::tests::drain"],
    ["rust.domain.agent-conversations.archive-jobs.execution",
      "domain::conversation_archive_jobs::tests::execution"],
    ["rust.domain.agent-conversations.archive-jobs.retry",
      "domain::conversation_archive_jobs::tests::retry"],
    ["rust.domain.agent-conversations.archive-jobs.cancel",
      "domain::conversation_archive_jobs::tests::cancel"],
    ["rust.domain.agent-conversations.archive-jobs.validation",
      "domain::conversation_archive_jobs::tests::validation"],
    ["rust.domain.agent-conversations.archive-jobs.support",
      "domain::conversation_archive_jobs::tests"],
  ]);
  const archiveModules = CLIENT_MODULE_CATALOG.filter((candidate) =>
    candidate.id.startsWith("rust.domain.agent-conversations.archive-jobs."));
  assert.equal(archiveModules.length, filters.size);
  for (const [id, filter] of filters) {
    const module = CLIENT_MODULE_CATALOG.find((candidate) => candidate.id === id);
    assert.equal(module.command.args.at(-1), filter);
    if (!id.endsWith(".composition")) {
      assert.equal(module.inputs.includes(
        "crates/licoup-native/src/domain/conversation_archive_jobs.rs"), false);
    }
  }

  const ownedInputs = new Set(archiveModules.flatMap((module) => module.inputs));
  const splitSources = await sourceFiles(
    "crates/licoup-native/src/domain/conversation_archive_jobs",
    ".rs",
  );
  for (const relativePath of [
    "crates/licoup-native/src/domain/conversation_archive_jobs.rs",
    ...splitSources,
  ]) {
    assert.equal(ownedInputs.has(relativePath), true,
      `archive-job source must have a precise regression owner: ${relativePath}`);
  }
});

test("message projection leaves retain exact tests and complete source ownership", async () => {
  const filters = new Map([
    ["rust.domain.agent-conversations.message-projection.composition",
      "domain::conversation::history::tests::message_projection"],
    ["rust.domain.agent-conversations.message-projection.projection",
      "domain::conversation::history::message_projection::tests::projection::"],
    ["rust.domain.agent-conversations.message-projection.structured-privacy",
      "domain::conversation::history::message_projection::tests::structured_privacy::"],
    ["rust.domain.agent-conversations.message-projection.antigravity",
      "domain::conversation::history::message_projection::tests::antigravity::"],
    ["rust.domain.agent-conversations.message-projection.generated-context",
      "domain::conversation::history::message_projection::tests::generated_context::"],
    ["rust.domain.agent-conversations.message-projection.json-extract",
      "domain::conversation::history::message_projection::tests::json_extract::"],
    ["rust.domain.agent-conversations.message-projection.semantic",
      "domain::conversation::history::message_projection::tests::semantic::"],
  ]);
  const modules = CLIENT_MODULE_CATALOG.filter((candidate) =>
    candidate.id.startsWith("rust.domain.agent-conversations.message-projection."));
  assert.equal(modules.length, filters.size);
  for (const [id, filter] of filters) {
    const module = CLIENT_MODULE_CATALOG.find((candidate) => candidate.id === id);
    assert.equal(module.command.args.at(-1), filter);
    assert.equal(module.inputs.includes(
      "crates/licoup-native/src/domain/conversation/history/message_projection_legacy.rs"),
    false);
  }
  const sourceCheck = CLIENT_MODULE_CATALOG.find((candidate) =>
    candidate.id === "regression.message-projection-source-bundle");
  const ownedInputs = new Set([
    ...modules.flatMap((module) => module.inputs),
    ...sourceCheck.inputs,
  ]);
  const splitSources = await sourceFiles(
    "crates/licoup-native/src/domain/conversation/history/message_projection",
    ".rs",
  );
  for (const relativePath of [
    "crates/licoup-native/src/domain/conversation/history/message_projection.rs",
    ...splitSources,
  ]) {
    assert.equal(ownedInputs.has(relativePath), true,
      `message projection source must have a focused regression owner: ${relativePath}`);
  }
});

test("session merge leaves retain exact tests and complete source ownership", async () => {
  const filters = new Map([
    ["rust.domain.agent-conversations.session-merge.composition",
      "domain::conversation::history::session_merge::tests::composition::"],
    ["rust.domain.agent-conversations.session-merge.codex-lineage",
      "domain::conversation::history::session_merge::tests::codex_lineage::"],
    ["rust.domain.agent-conversations.session-merge.delegated",
      "domain::conversation::history::session_merge::tests::delegated_merge::"],
    ["rust.domain.agent-conversations.session-merge.dedupe-paging",
      "domain::conversation::history::session_merge::tests::dedupe_paging::"],
    ["rust.domain.agent-conversations.session-merge.model-names",
      "domain::conversation::history::session_merge::tests::model_names::"],
    ["rust.domain.agent-conversations.session-merge.session-index",
      "domain::conversation::history::session_merge::tests::session_index::"],
    ["rust.domain.agent-conversations.session-merge.stable-order",
      "domain::conversation::history::session_merge::tests::stable_order::"],
    ["rust.domain.agent-conversations.session-merge.integration",
      "domain::conversation::history::tests::session_merge"],
  ]);
  const modules = CLIENT_MODULE_CATALOG.filter((candidate) =>
    candidate.id.startsWith("rust.domain.agent-conversations.session-merge."));
  assert.equal(modules.length, filters.size);
  for (const [id, filter] of filters) {
    const module = CLIENT_MODULE_CATALOG.find((candidate) => candidate.id === id);
    assert.equal(module.command.args.at(-1), filter);
  }
  assert.equal(CLIENT_MODULE_CATALOG.some((candidate) =>
    candidate.id === "rust.domain.agent-conversations.session-merge"), false);

  const sourceCheck = CLIENT_MODULE_CATALOG.find((candidate) =>
    candidate.id === "regression.session-merge-source-bundle");
  const ownedInputs = new Set([
    ...modules.flatMap((module) => module.inputs),
    ...sourceCheck.inputs,
  ]);
  const splitSources = await sourceFiles(
    "crates/licoup-native/src/domain/conversation/history/session_merge",
    ".rs",
  );
  for (const relativePath of [
    "crates/licoup-native/src/domain/conversation/history/session_merge.rs",
    ...splitSources,
  ]) {
    assert.equal(ownedInputs.has(relativePath), true,
      `session merge source must have a focused regression owner: ${relativePath}`);
  }
});

test("agent conversation pane leaves retain exact widget tests and bounded catalog ownership", () => {
  const filters = new Map([
    ["flutter.feature.agent-conversations.pane-composition",
      "test/agent_conversation_pane/composition_test.dart"],
    ["flutter.feature.agent-conversations.pane-actions",
      "test/agent_conversation_pane/actions_test.dart"],
    ["flutter.feature.agent-conversations.pane-resize",
      "test/agent_conversation_pane/resize_test.dart"],
    ["flutter.feature.agent-conversations.pane-header",
      "test/agent_conversation_pane/header_test.dart"],
  ]);
  const modules = CLIENT_MODULE_CATALOG.filter((candidate) =>
    candidate.id.startsWith("flutter.feature.agent-conversations.pane-"));
  assert.equal(modules.length, filters.size);
  for (const [id, testPath] of filters) {
    const module = CLIENT_MODULE_CATALOG.find((candidate) => candidate.id === id);
    assert.equal(module.command.args.at(-1), testPath);
  }

  const sourceCheck = CLIENT_MODULE_CATALOG.find((candidate) =>
    candidate.id === "regression.agent-conversation-pane-source-bundle");
  const ownedInputs = new Set([
    ...modules.flatMap((module) => module.inputs),
    ...sourceCheck.inputs,
  ]);
  for (const relativePath of [
    "apps/desktop/lib/src/frontend/features/agents/ui/agent_conversation_pane.dart",
    "apps/desktop/lib/src/frontend/features/agents/ui/agent_conversation_pane/actions.dart",
    "apps/desktop/lib/src/frontend/features/agents/ui/agent_conversation_pane/composition.dart",
    "apps/desktop/lib/src/frontend/features/agents/ui/agent_conversation_pane/header.dart",
    "apps/desktop/lib/src/frontend/features/agents/ui/agent_conversation_pane/resize.dart",
  ]) {
    assert.equal(ownedInputs.has(relativePath), true,
      `agent conversation pane source must have a focused regression owner: ${relativePath}`);
  }

  const conversationFoundation = CLIENT_MODULE_CATALOG.find((candidate) =>
    candidate.id === "flutter.feature.agent-conversations");
  assert.equal(conversationFoundation.inputs.includes(
    "apps/desktop/lib/src/frontend/features/agents/ui/agent_conversation_pane.dart"), false);
});

test("message block leaves retain exact widget tests and bounded catalog ownership", () => {
  const filters = new Map([
    ["flutter.feature.agent-conversations.message-blocks-dispatcher",
      "test/agent_conversation_message_blocks/dispatcher_test.dart"],
    ["flutter.feature.agent-conversations.message-blocks-disclosures",
      "test/agent_conversation_message_blocks/disclosures_test.dart"],
    ["flutter.feature.agent-conversations.message-blocks-roles",
      "test/agent_conversation_message_blocks/role_blocks_test.dart"],
    ["flutter.feature.agent-conversations.message-blocks-subagent",
      "test/agent_conversation_message_blocks/subagent_test.dart"],
  ]);
  const modules = CLIENT_MODULE_CATALOG.filter((candidate) =>
    candidate.id.startsWith("flutter.feature.agent-conversations.message-blocks-"));
  assert.equal(modules.length, filters.size);
  for (const [id, testPath] of filters) {
    const module = CLIENT_MODULE_CATALOG.find((candidate) => candidate.id === id);
    assert.equal(module.command.args.at(-1), testPath);
  }

  const sourceCheck = CLIENT_MODULE_CATALOG.find((candidate) =>
    candidate.id === "regression.agent-conversation-message-blocks-source-bundle");
  const ownedInputs = new Set([
    ...modules.flatMap((module) => module.inputs),
    ...sourceCheck.inputs,
  ]);
  for (const relativePath of [
    "apps/desktop/lib/src/frontend/features/agents/ui/agent_conversation_message_blocks.dart",
    "apps/desktop/lib/src/frontend/features/agents/ui/agent_conversation_message_blocks/disclosures.dart",
    "apps/desktop/lib/src/frontend/features/agents/ui/agent_conversation_message_blocks/dispatcher.dart",
    "apps/desktop/lib/src/frontend/features/agents/ui/agent_conversation_message_blocks/role_blocks.dart",
    "apps/desktop/lib/src/frontend/features/agents/ui/agent_conversation_message_blocks/subagent.dart",
  ]) {
    assert.equal(ownedInputs.has(relativePath), true,
      `message block source must have a focused regression owner: ${relativePath}`);
  }

  const conversationFoundation = CLIENT_MODULE_CATALOG.find((candidate) =>
    candidate.id === "flutter.feature.agent-conversations");
  assert.equal(conversationFoundation.inputs.includes(
    "apps/desktop/lib/src/frontend/features/agents/ui/agent_conversation_message_blocks.dart"), false);
});

test("conversation semantic leaves retain exact tests and complete source ownership", async () => {
  const filters = new Map([
    ["rust.domain.agent-conversations.semantic.composition",
      "domain::conversation_semantic::tests::composition::"],
    ["rust.domain.agent-conversations.semantic.model",
      "domain::conversation_semantic::tests::model::"],
    ["rust.domain.agent-conversations.semantic.builder",
      "domain::conversation_semantic::tests::builder::"],
    ["rust.domain.agent-conversations.semantic.thread-projection",
      "domain::conversation_semantic::tests::thread_projection::"],
    ["rust.domain.agent-conversations.semantic.execution-projection",
      "domain::conversation_semantic::tests::execution_projection::"],
    ["rust.domain.agent-conversations.semantic.artifact-projection",
      "domain::conversation_semantic::tests::artifact_projection::"],
    ["rust.domain.agent-conversations.semantic.validation",
      "domain::conversation_semantic::tests::validation::"],
    ["rust.domain.agent-conversations.semantic.privacy",
      "domain::conversation_semantic::tests::privacy::"],
    ["rust.domain.agent-conversations.semantic.markdown",
      "domain::conversation_semantic::tests::markdown::"],
    ["rust.domain.agent-conversations.semantic.io",
      "domain::conversation_semantic::tests::io::"],
  ]);
  const modules = CLIENT_MODULE_CATALOG.filter((candidate) =>
    candidate.id.startsWith("rust.domain.agent-conversations.semantic."));
  assert.equal(modules.length, filters.size);
  for (const [id, filter] of filters) {
    const module = CLIENT_MODULE_CATALOG.find((candidate) => candidate.id === id);
    assert.equal(module.command.args.at(-1), filter);
  }
  assert.equal(CLIENT_MODULE_CATALOG.some((candidate) =>
    candidate.id === "rust.domain.agent-conversations.semantic"), false);

  const sourceCheck = CLIENT_MODULE_CATALOG.find((candidate) =>
    candidate.id === "regression.conversation-semantic-source-bundle");
  const ownedInputs = new Set([
    ...modules.flatMap((module) => module.inputs),
    ...sourceCheck.inputs,
  ]);
  const splitSources = await sourceFiles(
    "crates/licoup-native/src/domain/conversation_semantic",
    ".rs",
  );
  for (const relativePath of [
    "crates/licoup-native/src/domain/conversation_semantic.rs",
    ...splitSources,
  ]) {
    assert.equal(ownedInputs.has(relativePath), true,
      `conversation semantic source must have a focused regression owner: ${relativePath}`);
  }
});
