-- Retained unused relations from an independent historical producer.
-- Current initialization preserves these tables without interpreting their rows.

CREATE TABLE peer_bindings(
  author BLOB NOT NULL, device BLOB NOT NULL,
  conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
  membership_id TEXT NOT NULL, provider_id TEXT NOT NULL,
  active INTEGER NOT NULL CHECK(active IN (0,1)), PRIMARY KEY(author,device));
CREATE TABLE peer_inbox(
  author BLOB NOT NULL, device BLOB NOT NULL,
  conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
  logical_id BLOB NOT NULL, event_id TEXT NOT NULL REFERENCES events(id) ON DELETE CASCADE,
  PRIMARY KEY(author,device,conversation_id,logical_id));
CREATE TABLE peer_effect_intents(
  id TEXT PRIMARY KEY, event_id TEXT NOT NULL REFERENCES events(id) ON DELETE CASCADE,
  ordinal INTEGER NOT NULL,
  conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
  membership_id TEXT NOT NULL, provider_id TEXT NOT NULL, request_json TEXT NOT NULL,
  accepted INTEGER NOT NULL DEFAULT 0 CHECK(accepted IN (0,1)), UNIQUE(event_id,ordinal));
CREATE INDEX peer_effect_intents_pending_idx ON peer_effect_intents(accepted,event_id,ordinal);
