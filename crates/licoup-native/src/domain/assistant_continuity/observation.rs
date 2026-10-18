//! Durable, low-cost observation of already admitted continuity facts.
//!
//! Observation is deliberately narrower than execution.  This module records
//! what an admitted run reported, keeps superseded and expired facts for
//! traceability, and builds read-only indexes over those facts.  It never
//! admits a turn, grants a capability, or decides that a goal is complete.
//!
//! The existing `continuity_source_cursors` seam is used as the durable
//! storage boundary.  A fact row is keyed by its caller-provided fact id and a
//! second, small row is keyed by the three routing dimensions.  This keeps
//! duplicate writes idempotent and lets scoped reads use the existing SQL key
//! index without introducing another continuity table.

use std::collections::{BTreeMap, BTreeSet};

use licoup_conversation::ConversationStore;
use licoup_conversation::continuity::{
    ContinuityDecisionLayer, ContinuityEffectClass, ContinuityFailure, ContinuityFailureCode,
    ContinuityFailureStage, ContinuityRecoveryClass,
};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use serde_json::json;

const OBSERVATION_FACT_KEY: &str = "observation:v1:fact";
const OBSERVATION_INDEX_PREFIX: &str = "observation:v1:index:";

/// Default and maximum page size for durable observation reads.
pub const OBSERVATION_PAGE_SIZE: usize = 50;
pub const MAX_OBSERVATION_PAGE_SIZE: usize = 1_000;

/// The dimensions used for narrow observation reads.
///
/// `configuration_id` is an opaque identity supplied by the existing runtime
/// or model owner.  It is recorded, not interpreted, here.
#[derive(Clone, Debug, Default, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservationScope {
    pub responsibility_id: String,
    pub task_type: String,
    pub configuration_id: String,
}

impl ObservationScope {
    pub fn new(
        responsibility_id: impl Into<String>,
        task_type: impl Into<String>,
        configuration_id: impl Into<String>,
    ) -> Self {
        Self {
            responsibility_id: responsibility_id.into(),
            task_type: task_type.into(),
            configuration_id: configuration_id.into(),
        }
    }

    fn validate(&self) -> Result<(), ContinuityFailure> {
        if self.responsibility_id.trim().is_empty()
            || self.task_type.trim().is_empty()
            || self.configuration_id.trim().is_empty()
        {
            return Err(invalid_request());
        }
        Ok(())
    }
}

/// What kind of fact was observed.  A repeat is an actual later run fact;
/// storage-level duplicate delivery is reported as [`ObservationRecordOutcome`]
/// and is never materialized as another fact.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ObservationFactKind {
    Run,
    Wait,
    Result,
    Repeat,
    Correction,
}

/// An immutable fact from one admitted run or its follow-up.
///
/// The fields are identities and observations only.  In particular, there is
/// no execution-permission or cross-domain score field in this type.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservationFact {
    pub fact_id: String,
    pub conversation_id: String,
    pub run_id: String,
    pub logical_key: String,
    pub scope: ObservationScope,
    pub membership_id: String,
    pub runtime_version: String,
    pub designation_epoch: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graph_visit: Option<String>,
    pub revision: i64,
    pub observed_at: i64,
    pub kind: ObservationFactKind,
    pub outcome: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
}

impl ObservationFact {
    pub fn validate(&self) -> Result<(), ContinuityFailure> {
        if self.fact_id.trim().is_empty()
            || self.conversation_id.trim().is_empty()
            || self.run_id.trim().is_empty()
            || self.logical_key.trim().is_empty()
            || self.membership_id.trim().is_empty()
            || self.runtime_version.trim().is_empty()
            || self.outcome.trim().is_empty()
            || self.designation_epoch < 0
            || self.revision < 0
            || self.observed_at < 0
            || self
                .graph_visit
                .as_deref()
                .is_some_and(|visit| visit.trim().is_empty())
            || self
                .supersedes
                .as_deref()
                .is_some_and(|fact_id| fact_id.trim().is_empty())
            || self
                .expires_at
                .is_some_and(|expires_at| expires_at < self.observed_at)
        {
            return Err(invalid_request());
        }
        self.scope.validate()
    }

    pub fn is_expired_at(&self, now_ms: i64) -> bool {
        self.expires_at
            .is_some_and(|expires_at| now_ms >= expires_at)
    }

    fn scope_key(&self) -> ScopeKey {
        (
            self.scope.responsibility_id.clone(),
            self.scope.task_type.clone(),
            self.scope.configuration_id.clone(),
        )
    }

    fn lineage_key(&self) -> LineageKey {
        (
            self.scope.responsibility_id.clone(),
            self.scope.task_type.clone(),
            self.logical_key.clone(),
        )
    }
}

/// Read-time state derived from immutable facts.  State is never written back
/// over the original observation, so a later correction cannot erase history.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ObservationState {
    Current,
    Stale,
    Superseded,
    Expired,
}

/// Parameters for a bounded durable page read.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservationQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<ObservationScope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_fact_id: Option<String>,
    #[serde(default = "default_observation_page_size")]
    pub limit: usize,
}

impl Default for ObservationQuery {
    fn default() -> Self {
        Self {
            scope: None,
            after_fact_id: None,
            limit: OBSERVATION_PAGE_SIZE,
        }
    }
}

fn default_observation_page_size() -> usize {
    OBSERVATION_PAGE_SIZE
}

/// A durable observation page.  The continuation token is an opaque fact id
/// suitable for passing back as [`ObservationQuery::after_fact_id`].
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservationPage {
    pub observations: Vec<ObservationFact>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_fact_id: Option<String>,
}

/// Result of attempting to persist one fact.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObservationRecordOutcome {
    Inserted,
    Duplicate,
}

type ScopeKey = (String, String, String);
type LineageKey = (String, String, String);

/// In-memory read index rebuilt from durable facts.
///
/// The index contains no aggregate quality score.  It only groups exact
/// observations by their declared scope and logical lineage, allowing callers
/// to inspect repeats, corrections, stale revisions, and expiry independently.
#[derive(Clone, Debug, Default)]
pub struct ObservationIndex {
    facts: BTreeMap<String, ObservationFact>,
    by_scope: BTreeMap<ScopeKey, BTreeSet<String>>,
    by_lineage: BTreeMap<LineageKey, BTreeSet<String>>,
    superseded_by: BTreeMap<String, BTreeSet<String>>,
}

impl ObservationIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_facts(
        facts: impl IntoIterator<Item = ObservationFact>,
    ) -> Result<Self, ContinuityFailure> {
        let mut index = Self::new();
        for fact in facts {
            let _ = index.insert(fact)?;
        }
        Ok(index)
    }

    pub fn insert(
        &mut self,
        fact: ObservationFact,
    ) -> Result<ObservationRecordOutcome, ContinuityFailure> {
        fact.validate()?;
        if let Some(previous) = self.facts.get(&fact.fact_id) {
            if previous == &fact {
                return Ok(ObservationRecordOutcome::Duplicate);
            }
            return Err(idempotency_conflict());
        }
        let fact_id = fact.fact_id.clone();
        self.by_scope
            .entry(fact.scope_key())
            .or_default()
            .insert(fact_id.clone());
        self.by_lineage
            .entry(fact.lineage_key())
            .or_default()
            .insert(fact_id.clone());
        if let Some(superseded) = &fact.supersedes {
            self.superseded_by
                .entry(superseded.clone())
                .or_default()
                .insert(fact_id.clone());
        }
        self.facts.insert(fact_id, fact);
        Ok(ObservationRecordOutcome::Inserted)
    }

    pub fn len(&self) -> usize {
        self.facts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.facts.is_empty()
    }

    pub fn get(&self, fact_id: &str) -> Option<&ObservationFact> {
        self.facts.get(fact_id)
    }

    pub fn all(&self) -> Vec<&ObservationFact> {
        let mut facts: Vec<_> = self.facts.values().collect();
        sort_facts(&mut facts);
        facts
    }

    pub fn facts_for(&self, scope: &ObservationScope) -> Vec<&ObservationFact> {
        let Some(ids) = self.by_scope.get(&scope_key(scope)) else {
            return Vec::new();
        };
        let mut facts: Vec<_> = ids.iter().filter_map(|id| self.facts.get(id)).collect();
        sort_facts(&mut facts);
        facts
    }

    pub fn current_for(&self, scope: &ObservationScope, now_ms: i64) -> Vec<&ObservationFact> {
        self.facts_for(scope)
            .into_iter()
            .filter(|fact| self.state_at(&fact.fact_id, now_ms) == Some(ObservationState::Current))
            .collect()
    }

    pub fn latest_for(
        &self,
        responsibility_id: &str,
        task_type: &str,
        logical_key: &str,
    ) -> Option<&ObservationFact> {
        let ids = self.by_lineage.get(&(
            responsibility_id.to_owned(),
            task_type.to_owned(),
            logical_key.to_owned(),
        ))?;
        ids.iter()
            .filter_map(|id| self.facts.get(id))
            .max_by_key(|fact| observation_order(fact))
    }

    /// A fact is stale when its lineage holds a fact from a strictly newer
    /// designation epoch or revision.  A repeat of the same epoch and
    /// revision is another observation of the same decision, not a newer
    /// one, so it never makes earlier facts stale.
    pub fn is_stale(&self, fact_id: &str) -> bool {
        let Some(fact) = self.facts.get(fact_id) else {
            return false;
        };
        self.by_lineage
            .get(&fact.lineage_key())
            .into_iter()
            .flat_map(|ids| ids.iter())
            .filter_map(|id| self.facts.get(id))
            .any(|other| {
                other.fact_id != fact.fact_id && decision_order(other) > decision_order(fact)
            })
    }

    pub fn state_at(&self, fact_id: &str, now_ms: i64) -> Option<ObservationState> {
        let fact = self.facts.get(fact_id)?;
        if let Some(superseders) = self.superseded_by.get(fact_id)
            && superseders.iter().any(|superseder_id| {
                self.facts.get(superseder_id).is_some_and(|superseder| {
                    !self.is_stale(superseder_id) && supersedes_temporally(superseder, fact)
                })
            })
        {
            return Some(ObservationState::Superseded);
        }
        if self.is_stale(fact_id) {
            return Some(ObservationState::Stale);
        }
        if fact.is_expired_at(now_ms) {
            return Some(ObservationState::Expired);
        }
        Some(ObservationState::Current)
    }

    pub fn corrections_for(&self, scope: &ObservationScope) -> Vec<&ObservationFact> {
        self.facts_for(scope)
            .into_iter()
            .filter(|fact| {
                fact.kind == ObservationFactKind::Correction || fact.supersedes.is_some()
            })
            .collect()
    }

    pub fn expired_at(&self, scope: &ObservationScope, now_ms: i64) -> Vec<&ObservationFact> {
        self.facts_for(scope)
            .into_iter()
            .filter(|fact| fact.is_expired_at(now_ms))
            .collect()
    }
}

/// Durable observation access over the existing Conversation store.
#[derive(Clone, Debug)]
pub struct ObservationStore {
    store: ConversationStore,
}

impl ObservationStore {
    pub fn new(store: ConversationStore) -> Self {
        Self { store }
    }

    pub fn store(&self) -> &ConversationStore {
        &self.store
    }

    pub fn record(
        &self,
        fact: &ObservationFact,
    ) -> Result<ObservationRecordOutcome, ContinuityFailure> {
        record_observation(&self.store, fact)
    }

    pub fn load(
        &self,
        conversation_id: &str,
        fact_id: &str,
    ) -> Result<Option<ObservationFact>, ContinuityFailure> {
        load_observation(&self.store, conversation_id, fact_id)
    }

    pub fn page(
        &self,
        conversation_id: &str,
        query: &ObservationQuery,
    ) -> Result<ObservationPage, ContinuityFailure> {
        list_observations(&self.store, conversation_id, query)
    }

    pub fn index(&self, conversation_id: &str) -> Result<ObservationIndex, ContinuityFailure> {
        load_observation_index(&self.store, conversation_id)
    }

    pub fn current_at(
        &self,
        conversation_id: &str,
        scope: &ObservationScope,
        now_ms: i64,
    ) -> Result<Vec<ObservationFact>, ContinuityFailure> {
        Ok(self
            .index(conversation_id)?
            .current_for(scope, now_ms)
            .into_iter()
            .cloned()
            .collect())
    }
}

/// Persist one immutable run fact using the existing continuity transaction.
pub fn record_observation(
    store: &ConversationStore,
    fact: &ObservationFact,
) -> Result<ObservationRecordOutcome, ContinuityFailure> {
    fact.validate()?;
    store
        .ensure_continuity_migrated()
        .map_err(|_| source_unavailable())?;
    let payload = serde_json::to_string(fact).map_err(|_| invalid_request())?;
    let index_key = observation_index_key(&fact.scope);
    let fact_id = fact.fact_id.clone();
    let conversation_id = fact.conversation_id.clone();
    let persist = store
        .with_continuity_unit_of_work(|unit| {
            let previous: Option<String> = unit
                .query_row(
                    "SELECT payload FROM continuity_source_cursors
                 WHERE conversation_id=?1 AND source_event_id=?2 AND interpretation_key=?3",
                    rusqlite::params![conversation_id, fact_id, OBSERVATION_FACT_KEY],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            if let Some(previous) = previous {
                let previous_fact: ObservationFact = serde_json::from_str(&previous)?;
                return Ok(if previous_fact == *fact {
                    PersistOutcome::Duplicate
                } else {
                    PersistOutcome::Conflict
                });
            }
            let conversation_exists: i64 = unit.query_row(
                "SELECT EXISTS(SELECT 1 FROM conversations WHERE id=?1)",
                [&conversation_id],
                |row| row.get::<_, i64>(0),
            )?;
            if conversation_exists == 0 {
                return Ok(PersistOutcome::ScopeDenied);
            }
            unit.execute(
                "INSERT INTO continuity_source_cursors(
               conversation_id, source_event_id, interpretation_key, payload
             ) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![conversation_id, fact_id, OBSERVATION_FACT_KEY, payload],
            )?;
            unit.execute(
                "INSERT INTO continuity_source_cursors(
               conversation_id, source_event_id, interpretation_key, payload
             ) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![
                    conversation_id,
                    fact_id,
                    index_key,
                    json!({"factId": fact.fact_id}).to_string()
                ],
            )?;
            unit.request_commit();
            Ok(PersistOutcome::Inserted)
        })
        .map_err(|_| source_unavailable())?;
    match persist {
        PersistOutcome::Inserted => Ok(ObservationRecordOutcome::Inserted),
        PersistOutcome::Duplicate => Ok(ObservationRecordOutcome::Duplicate),
        PersistOutcome::Conflict => Err(idempotency_conflict()),
        PersistOutcome::ScopeDenied => Err(scope_denied()),
    }
}

/// Load one exact fact without changing its state or interpreting it as a
/// permission to execute.
pub fn load_observation(
    store: &ConversationStore,
    conversation_id: &str,
    fact_id: &str,
) -> Result<Option<ObservationFact>, ContinuityFailure> {
    if conversation_id.trim().is_empty() || fact_id.trim().is_empty() {
        return Err(invalid_request());
    }
    store
        .ensure_continuity_migrated()
        .map_err(|_| source_unavailable())?;
    let conversation_id = conversation_id.to_owned();
    let fact_id = fact_id.to_owned();
    let raw = store
        .with_continuity_unit_of_work(|unit| {
            Ok(unit
                .query_row(
                    "SELECT payload FROM continuity_source_cursors
                     WHERE conversation_id=?1 AND source_event_id=?2 AND interpretation_key=?3",
                    rusqlite::params![conversation_id, fact_id, OBSERVATION_FACT_KEY],
                    |row| row.get::<_, String>(0),
                )
                .optional()?)
        })
        .map_err(|_| source_unavailable())?;
    raw.map(|value| serde_json::from_str(&value).map_err(|_| source_unavailable()))
        .transpose()
}

/// Read a bounded page through the persisted scope index.
pub fn list_observations(
    store: &ConversationStore,
    conversation_id: &str,
    query: &ObservationQuery,
) -> Result<ObservationPage, ContinuityFailure> {
    if conversation_id.trim().is_empty()
        || query.limit == 0
        || query.limit > MAX_OBSERVATION_PAGE_SIZE
        || query
            .after_fact_id
            .as_deref()
            .is_some_and(|fact_id| fact_id.trim().is_empty())
    {
        return Err(invalid_request());
    }
    if let Some(scope) = &query.scope {
        scope.validate()?;
    }
    store
        .ensure_continuity_migrated()
        .map_err(|_| source_unavailable())?;
    let conversation_id = conversation_id.to_owned();
    let after = query.after_fact_id.clone();
    let limit = query.limit as i64;
    let fetch_limit = limit.saturating_add(1);
    let raw_rows = store
        .with_continuity_unit_of_work(|unit| {
            if let Some(scope) = &query.scope {
                let index_key = observation_index_key(scope);
                Ok(unit.query_vec(
                    "SELECT fact.payload
                         FROM continuity_source_cursors AS idx
                         JOIN continuity_source_cursors AS fact
                           ON fact.conversation_id=idx.conversation_id
                          AND fact.source_event_id=idx.source_event_id
                          AND fact.interpretation_key=?4
                         WHERE idx.conversation_id=?1
                           AND idx.interpretation_key=?3
                           AND (?2 IS NULL OR idx.source_event_id > ?2)
                         ORDER BY idx.source_event_id ASC
                         LIMIT ?5",
                    rusqlite::params![
                        conversation_id,
                        after,
                        index_key,
                        OBSERVATION_FACT_KEY,
                        fetch_limit
                    ],
                    |row| row.get::<_, String>(0),
                )?)
            } else {
                Ok(unit.query_vec(
                    "SELECT payload FROM continuity_source_cursors
                         WHERE conversation_id=?1
                           AND interpretation_key=?2
                           AND (?3 IS NULL OR source_event_id > ?3)
                         ORDER BY source_event_id ASC
                         LIMIT ?4",
                    rusqlite::params![conversation_id, OBSERVATION_FACT_KEY, after, fetch_limit],
                    |row| row.get::<_, String>(0),
                )?)
            }
        })
        .map_err(|_| source_unavailable())?;
    let mut observations: Vec<ObservationFact> = Vec::with_capacity(raw_rows.len());
    for raw in raw_rows {
        observations.push(serde_json::from_str(&raw).map_err(|_| source_unavailable())?);
    }
    let has_more = observations.len() > query.limit;
    if has_more {
        observations.truncate(query.limit);
    }
    let next_fact_id = has_more
        .then(|| observations.last().map(|fact| fact.fact_id.clone()))
        .flatten();
    Ok(ObservationPage {
        observations,
        next_fact_id,
    })
}

/// Rebuild the small in-memory index from durable fact rows.
pub fn load_observation_index(
    store: &ConversationStore,
    conversation_id: &str,
) -> Result<ObservationIndex, ContinuityFailure> {
    if conversation_id.trim().is_empty() {
        return Err(invalid_request());
    }
    store
        .ensure_continuity_migrated()
        .map_err(|_| source_unavailable())?;
    let conversation_id = conversation_id.to_owned();
    let raw_rows = store
        .with_continuity_unit_of_work(|unit| {
            Ok(unit.query_vec(
                "SELECT payload FROM continuity_source_cursors
                     WHERE conversation_id=?1 AND interpretation_key=?2
                     ORDER BY source_event_id ASC",
                rusqlite::params![conversation_id, OBSERVATION_FACT_KEY],
                |row| row.get::<_, String>(0),
            )?)
        })
        .map_err(|_| source_unavailable())?;
    let mut facts = Vec::with_capacity(raw_rows.len());
    for raw in raw_rows {
        facts.push(serde_json::from_str(&raw).map_err(|_| source_unavailable())?);
    }
    ObservationIndex::from_facts(facts)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PersistOutcome {
    Inserted,
    Duplicate,
    Conflict,
    ScopeDenied,
}

fn observation_index_key(scope: &ObservationScope) -> String {
    format!(
        "{OBSERVATION_INDEX_PREFIX}{}:{}:{}",
        encode_component(&scope.responsibility_id),
        encode_component(&scope.task_type),
        encode_component(&scope.configuration_id),
    )
}

fn encode_component(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len() * 2);
    for byte in value.bytes() {
        encoded.push_str(&format!("{byte:02x}"));
    }
    encoded
}

fn scope_key(scope: &ObservationScope) -> ScopeKey {
    (
        scope.responsibility_id.clone(),
        scope.task_type.clone(),
        scope.configuration_id.clone(),
    )
}

/// Decision ordering: only a newer designation epoch or revision is a newer
/// decision.  Observation time alone does not outrank an earlier fact.
fn decision_order(fact: &ObservationFact) -> (i64, i64) {
    (fact.designation_epoch, fact.revision)
}

fn observation_order(fact: &ObservationFact) -> (i64, i64, i64, String) {
    (
        fact.designation_epoch,
        fact.revision,
        fact.observed_at,
        fact.fact_id.clone(),
    )
}

/// Guard for explicit corrections: a `supersedes` reference must not point
/// backwards in observation order.
fn supersedes_temporally(newer: &ObservationFact, older: &ObservationFact) -> bool {
    observation_order(newer) > observation_order(older)
}

fn sort_facts(facts: &mut Vec<&ObservationFact>) {
    facts.sort_by(|left, right| observation_order(left).cmp(&observation_order(right)));
}

fn invalid_request() -> ContinuityFailure {
    failure(
        ContinuityFailureCode::InvalidRequest,
        ContinuityFailureStage::ContinuityAdmission,
        ContinuityRecoveryClass::CorrectRequest,
        false,
    )
}

fn idempotency_conflict() -> ContinuityFailure {
    failure(
        ContinuityFailureCode::IdempotencyConflict,
        ContinuityFailureStage::ContinuityCommit,
        ContinuityRecoveryClass::CorrectRequest,
        false,
    )
}

fn scope_denied() -> ContinuityFailure {
    failure(
        ContinuityFailureCode::ScopeDenied,
        ContinuityFailureStage::ContinuityCommit,
        ContinuityRecoveryClass::CorrectRequest,
        false,
    )
}

fn source_unavailable() -> ContinuityFailure {
    failure(
        ContinuityFailureCode::SourceUnavailable,
        ContinuityFailureStage::ContinuityCommit,
        ContinuityRecoveryClass::ReviewOrWait,
        true,
    )
}

fn failure(
    code: ContinuityFailureCode,
    stage: ContinuityFailureStage,
    recovery: ContinuityRecoveryClass,
    retryable: bool,
) -> ContinuityFailure {
    ContinuityFailure {
        code,
        stage,
        recovery,
        effect_class: ContinuityEffectClass::None,
        decision_layer: ContinuityDecisionLayer::Admission,
        retryable,
    }
}
