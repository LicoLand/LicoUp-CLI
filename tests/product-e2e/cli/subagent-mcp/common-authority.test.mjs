import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const claims = readFileSync(
  "crates/licoup-conversation/src/store/dispatches.rs",
  "utf8",
);
const store = readFileSync(
  "crates/licoup-conversation/src/store/mod.rs",
  "utf8",
);
const schema = readFileSync(
  "crates/licoup-conversation/src/store/schema.rs",
  "utf8",
);
const application = readFileSync(
  "crates/licoup-native/src/domain/subagents/mod.rs",
  "utf8",
);
const production = readFileSync(
  "crates/licoup-native/src/domain/subagents/production.rs",
  "utf8",
);
const conversationHost = readFileSync(
  "crates/licoup-native/src/bin/licoup/conversation_host.rs",
  "utf8",
);

test("authority admits only exact active same-conversation agent memberships", () => {
  assert.match(claims, /m\.status='active'/u);
  assert.match(claims, /p\.kind='agent'/u);
  assert.match(claims, /subagent_self_call_rejected/u);
  assert.match(claims, /subagent_cross_conversation_rejected/u);
  assert.match(claims, /subagent_duplicate_active_edge/u);
  assert.match(store, /mod schema;/u);
  assert.match(schema, /CREATE UNIQUE INDEX IF NOT EXISTS subagent_dispatch_claims_active_edge/u);
});

test("claim precedes PersistentTurn effect and uncertain cancel reconciles", () => {
  const claim = application.indexOf("claim_dispatch(");
  const send = application.indexOf("runtime.send", claim);
  assert.ok(claim >= 0 && send > claim);
  assert.match(application, /ReconciliationRequired/u);
  assert.match(application, /reconcile_before_retry/u);
  assert.match(application, /active_claim/u);
});

test("read-only target tools use non-persisting inspection rather than a mutable scan", () => {
  assert.match(production, /inspect_target_read_only/u);
  assert.doesNotMatch(production, /scan_targets\(\)/u);
});

test("production MCP delegates every Conversation operation to the persistent host", () => {
  const implementation = production.split("#[cfg(test)]", 1)[0];
  assert.match(implementation, /execute_existing\(\s*"client\.conversation\.execute"/u);
  assert.doesNotMatch(implementation, /ConversationService|ConversationStore|portable_data_dir|\.store\(\)/u);
  assert.match(conversationHost, /serve_stdio_rpc_with_persistent_conversation/u);
  assert.match(conversationHost, /conversation_service/u);
});
