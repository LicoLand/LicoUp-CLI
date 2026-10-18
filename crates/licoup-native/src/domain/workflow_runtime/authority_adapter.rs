//! V7-R2: the native side of admission — the existing authorization owner
//! behind the boundary, and the durable scope barrier a pause writes.
//!
//! Two adapters, and no new source of truth:
//!
//! * [`StoreAuthorityAdapter`] adapts **the existing authorization owner**
//!   (`StrategyStore::{authorization_preview, grant_authorization,
//!   revoke_authorization, authorize_effect}`) into the read/recheck/permit
//!   shape C01's admission boundary needs. It resolves the grant in force,
//!   rechecks an expectation against it, and issues the one-shot effect permit
//!   through the store's own write-locked path. It adds no policy of its own:
//!   the same three conditions the store applies to a current authorization
//!   (active, the definition digest it was issued for, the semantics it was
//!   issued against) are the ones used here.
//! * [`StoreScopeBarrier`] writes the pause/stop barrier and freezes the
//!   recipients it addresses **in one transaction**, using the durable tables
//!   the controlled store already owns (`workflow_graph_state.barrier_active`,
//!   `workflow_pause_requests`, `workflow_stop_requests`). C03 requires the two
//!   facts to be one decision: a barrier without the frozen set lets a later
//!   instruction reach recipients the instruction never addressed, and a freeze
//!   without the barrier lets a new node visit start under a paused scope.
//!
//! ## Why this file does not name the runtime's port types
//!
//! The ports are declared in `licoup-workflow-runtime`
//! (`ports::{AuthorityPort, …}`, and `admission::{…}` for the boundary itself),
//! and the store side of that seam implements them. `licoup-native` carries no
//! dependency on that crate yet, so this adapter repeats the *shape* — a
//! digest-only grant reference, a recheck that answers with what moved, and a
//! one-shot permit — rather than importing it. The composition edge is V7-I1's
//! step; adding a dependency here to avoid a few method signatures would edit
//! the crate graph, which is not this leaf's to do. The meaning is deliberately
//! identical, so the first thing that can be retired is the repetition, not a
//! second interpretation of the same facts.
//!
//! ## The ordering a revocation takes effect in (C01)
//!
//! [`StoreAuthorityAdapter::permit_effect`] goes through the store's
//! `authorize_effect`, whose own contract is the linearization rule this task
//! carries: it revalidates the command and the authorization under the SQLite
//! write lock, and **a later revoke does not retroactively invalidate an
//! already-issued one-shot permit**. So a revocation stops the *next* effect —
//! [`StoreAuthorityAdapter::recheck`] then answers `NoneInForce` or `Replaced`,
//! and the next admission is refused — while the effect already admitted keeps
//! its binding and settles on its own authenticated outcome. The permit half is
//! the store's own contract, proved by
//! `effect_authorization_revalidates_digest_owner_and_live_lease` in
//! `workflow_store/store.rs` (a live lease under the current digest permits,
//! a lost lease or a stale digest does not). This adapter delegates to that
//! path and proves its own wiring of it in the tests below; it does not
//! re-implement the rule.
//!
//! ## What this file deliberately does not do
//!
//! It does not change the legacy control path: `InterventionProxy::admit` with
//! `ControlOperation::Pause` still goes through `record_admission`, which writes
//! the pause-negotiating row and *not* the graph barrier. That one statement
//! belongs to the controlled store's owner
//! (`licoup-workflow-store/src/control.rs`), which is outside this task's write
//! scope; the barrier writer here is what the admission seam calls, and routing
//! the legacy control path through it is V7-I1's wiring step. Until then, a
//! graph pause admitted through the legacy proxy does not by itself stop a new
//! visit — a gap this module records rather than papers over.

use std::collections::BTreeSet;

use anyhow::{Result, ensure};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::domain::workflow_store::{StrategyAuthorization, StrategyDefinition, StrategyStore};

/// The grant in force for one definition revision, as the owner resolves it.
///
/// The owner's own [`StrategyAuthorization`], not a copy: the digest, the
/// revision and the active flag are its facts, and a second type here would be
/// a second place for them to disagree.
pub type ResolvedGrant = StrategyAuthorization;

/// One recheck of an effect against the grant that admitted it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GrantRecheck {
    pub run_id: String,
    /// The grant the caller believes admitted this effect.
    pub expected_authorization_digest: String,
    /// The semantics the caller believes it is acting under.
    pub expected_semantics_digest: String,
}

/// What the recheck found, with the fact that decided it.
///
/// A bare `false` would be indistinguishable between "the grant was revoked",
/// "a different grant is in force" and "the semantics moved under you", and
/// those three need different answers from the caller.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GrantVerdict {
    /// The grant in force is the one the effect was admitted under.
    Covered { authorization_digest: String },
    /// No grant is in force for the run's revision.
    NoneInForce { revision_digest: String },
    /// A different grant is in force: the caller's expectation is stale.
    Replaced { expected: String, observed: String },
    /// The grant's semantics are not the ones the caller expected.
    SemanticsMoved { expected: String, observed: String },
}

/// One request for the one-shot effect permit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EffectPermit {
    pub run_id: String,
    /// The effect identity (C01's `effectId`).
    pub effect_id: String,
    /// The attempt the permit is issued for. Stable effect identity, per-attempt
    /// permit: a replayed attempt cannot reuse the previous one.
    pub attempt_token: String,
    pub expected_authorization_digest: String,
    /// The fenced claimant that holds the command's lease.
    pub claimant: String,
    pub lease_until_unix_ms: i64,
}

/// Reads the grant in force, rechecks it at the boundary, and issues the
/// one-shot permit the effect must hold before it runs.
#[derive(Clone, Debug)]
pub struct StoreAuthorityAdapter {
    store: StrategyStore,
}

impl StoreAuthorityAdapter {
    pub fn new(store: StrategyStore) -> Self {
        Self { store }
    }

    pub fn store(&self) -> &StrategyStore {
        &self.store
    }

    /// The grant in force for a definition revision, if any.
    pub fn active_grant(&self, revision_digest: &str) -> Result<Option<ResolvedGrant>> {
        let definition = self.store.definition_by_revision(revision_digest)?;
        Ok(active_authorization(&definition).cloned())
    }

    /// Recheck an effect's expectation against the grant in force now.
    ///
    /// Read-only: this is the answer the admission boundary records. Issuing the
    /// permit that serializes with revocation is
    /// [`Self::permit_effect`]'s job, because that one has to share the store's
    /// write lock with the grant writes.
    pub fn recheck(&self, request: &GrantRecheck) -> Result<GrantVerdict> {
        let snapshot = self.store.run(&request.run_id)?;
        let definition = self
            .store
            .definition_by_revision(&snapshot.definition_digest)?;
        let Some(active) = active_authorization(&definition) else {
            return Ok(GrantVerdict::NoneInForce {
                revision_digest: snapshot.definition_digest,
            });
        };
        if active.authorization_digest != request.expected_authorization_digest {
            return Ok(GrantVerdict::Replaced {
                expected: request.expected_authorization_digest.clone(),
                observed: active.authorization_digest.clone(),
            });
        }
        if active.semantics_digest != request.expected_semantics_digest {
            return Ok(GrantVerdict::SemanticsMoved {
                expected: request.expected_semantics_digest.clone(),
                observed: active.semantics_digest.clone(),
            });
        }
        Ok(GrantVerdict::Covered {
            authorization_digest: active.authorization_digest.clone(),
        })
    }

    /// Issue the one-shot permit for one attempt of one effect.
    ///
    /// Delegates to the store's `authorize_effect`: the lease, the attempt
    /// token, the claimant fence, the command status and the authorization are
    /// revalidated in one write transaction, so the permit cannot be issued
    /// against a lease the caller has already lost. Its doc also states the
    /// ordering rule: a revocation after this call does not invalidate this
    /// permit, it prevents the next one.
    pub fn permit_effect(&self, permit: &EffectPermit) -> Result<()> {
        self.store.authorize_effect(
            &permit.run_id,
            &permit.effect_id,
            &permit.attempt_token,
            &permit.expected_authorization_digest,
            &permit.claimant,
            permit.lease_until_unix_ms,
        )
    }

    /// The digest the next grant for this revision would carry.
    ///
    /// Delegated unchanged: `authorization_preview` is the store's own answer,
    /// including its refusal when the definition's bindings are incomplete.
    pub fn preview(&self, revision_digest: &str) -> Result<ResolvedGrant> {
        self.store.authorization_preview(revision_digest)
    }

    /// Record a grant, against the digest the caller previewed.
    ///
    /// Delegated unchanged: the store refuses a stale expected digest rather
    /// than granting over a binding change the caller did not see.
    pub fn grant(
        &self,
        revision_digest: &str,
        expected_authorization_digest: &str,
    ) -> Result<ResolvedGrant> {
        self.store
            .grant_authorization(revision_digest, expected_authorization_digest)
    }

    /// Revoke the grant in force for a revision.
    ///
    /// Delegated unchanged. After this call the next admission of an effect for
    /// this revision is refused; permits already issued stand.
    pub fn revoke(&self, revision_digest: &str) -> Result<()> {
        self.store.revoke_authorization(revision_digest)
    }
}

/// The grant the owner considers current for a definition.
///
/// The same three conditions the store's private `current_authorization`
/// applies — active, issued for this definition digest, issued against the
/// semantics the definition still carries — so a grant that became inactive, or
/// one whose semantics moved, resolves to nothing instead of to a stale digest.
fn active_authorization(definition: &StrategyDefinition) -> Option<&StrategyAuthorization> {
    definition.authorization.as_ref().filter(|authorization| {
        authorization.active
            && authorization.definition_digest == definition.summary.revision_digest
            && authorization.semantics_digest == definition.summary.semantics_digest
    })
}

/// Which scope a barrier fences.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BarrierScope {
    /// The whole graph. This is the scope C03 gives the new-start fence to.
    Graph,
    /// One node or invocation target inside the graph.
    Node(String),
}

impl BarrierScope {
    /// Whether a barrier on this scope stops a new node visit from starting.
    pub fn blocks_new_visits(&self) -> bool {
        matches!(self, Self::Graph)
    }

    /// The target key the durable tables use for this scope.
    pub fn target(&self) -> &str {
        match self {
            Self::Graph => "graph",
            Self::Node(target) => target.as_str(),
        }
    }
}

/// What was asked of a scope.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BarrierKind {
    /// Stop starting new work; in-flight work drains or suspends according to
    /// what its adapter supports.
    Pause,
    /// Stop the scope. Recorded in the monotonic stop table, which a later
    /// pause or resume does not clear.
    Stop,
}

/// One barrier publication, as the control path asks for it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BarrierRequest {
    pub graph_id: String,
    pub scope: BarrierScope,
    pub kind: BarrierKind,
    pub reason: String,
    /// The recipients frozen when the instruction was handled, as target keys.
    /// The caller took them from its own freeze; this adapter does not resolve
    /// them, because only the handling owner knows what was in flight then.
    pub recipients: Vec<String>,
}

/// What the publication wrote, read back from the same transaction.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublishedBarrier {
    pub graph_id: String,
    pub scope: BarrierScope,
    pub kind: BarrierKind,
    pub reason: String,
    /// The recipients frozen with this barrier.
    pub recipients: Vec<String>,
    /// Whether the graph-scope new-start fence is active after this write.
    pub barrier_active: bool,
    pub graph_revision: Option<u64>,
    /// The pause targets active for this graph, read after the write.
    pub pause_targets: BTreeSet<String>,
    /// The stop targets recorded for this graph, read after the write.
    pub stop_targets: BTreeSet<String>,
}

/// The durable barrier state of one graph.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GraphBarrierState {
    pub barrier_active: bool,
    pub graph_revision: Option<u64>,
    pub pause_targets: BTreeSet<String>,
    pub stop_targets: BTreeSet<String>,
}

impl GraphBarrierState {
    /// Whether a new node visit may start in this graph.
    pub fn admits_new_visits(&self) -> bool {
        !self.barrier_active
    }
}

/// Writes the pause/stop barrier and freezes its recipients in one transaction.
#[derive(Clone, Debug)]
pub struct StoreScopeBarrier {
    store: StrategyStore,
}

impl StoreScopeBarrier {
    pub fn new(store: StrategyStore) -> Self {
        Self { store }
    }

    pub fn store(&self) -> &StrategyStore {
        &self.store
    }

    /// Write the barrier and the frozen recipients, or write neither.
    ///
    /// One transaction, because the two facts are one decision. What is *not*
    /// written here matters too: this method never clears a barrier and never
    /// removes a pause or stop target, so a later pause cannot quietly re-open a
    /// scope a stop already closed.
    pub fn publish(&self, request: &BarrierRequest) -> Result<PublishedBarrier> {
        validate_id(&request.graph_id, "workflow_barrier_graph_invalid")?;
        for target in &request.recipients {
            validate_id(target, "workflow_barrier_target_invalid")?;
        }
        let fences_new_visits = request.scope.blocks_new_visits();
        // The target set is the instruction's own scope key plus its frozen
        // recipients — the same keys the controlled store's control path uses
        // (`pause_target`/`control_target`), so `is_pause_negotiating` and
        // `is_stop_requested` keep answering for them.
        let mut targets = request.recipients.clone();
        if !targets
            .iter()
            .any(|target| target == request.scope.target())
        {
            targets.push(request.scope.target().to_owned());
        }
        self.store.with_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if fences_new_visits {
                transaction.execute(
                    "INSERT INTO workflow_graph_state(graph_id, graph_revision, barrier_active)
                     VALUES (?1, 1, 1)
                     ON CONFLICT(graph_id) DO UPDATE SET barrier_active=1",
                    params![request.graph_id],
                )?;
            }
            for target in &targets {
                match request.kind {
                    BarrierKind::Pause => {
                        transaction.execute(
                            "INSERT INTO workflow_pause_requests(graph_id, target, active)
                             VALUES (?1, ?2, 1)
                             ON CONFLICT(graph_id, target) DO UPDATE SET active=1",
                            params![request.graph_id, target],
                        )?;
                    }
                    // A stop is recorded where a stop is read, and it is never
                    // cleared: `workflow_stop_requests` has no active flag to
                    // unset, which is what makes the stop monotonic.
                    BarrierKind::Stop => {
                        transaction.execute(
                            "INSERT OR IGNORE INTO workflow_stop_requests(graph_id, target)
                             VALUES (?1, ?2)",
                            params![request.graph_id, target],
                        )?;
                    }
                }
            }
            // Read back inside the same transaction: the receipt is what this
            // write produced, not a later read of a scope someone else moved.
            let state = read_state(&transaction, &request.graph_id)?;
            transaction.commit()?;
            Ok(PublishedBarrier {
                graph_id: request.graph_id.clone(),
                scope: request.scope.clone(),
                kind: request.kind,
                reason: request.reason.clone(),
                recipients: request.recipients.clone(),
                barrier_active: state.barrier_active,
                graph_revision: state.graph_revision,
                pause_targets: state.pause_targets,
                stop_targets: state.stop_targets,
            })
        })
    }

    /// The barrier state of one graph, as admission reads it.
    pub fn state(&self, graph_id: &str) -> Result<GraphBarrierState> {
        validate_id(graph_id, "workflow_barrier_graph_invalid")?;
        self.store
            .with_connection(|connection| read_state(connection, graph_id))
    }
}

fn read_state(connection: &Connection, graph_id: &str) -> Result<GraphBarrierState> {
    let row: Option<(i64, i64)> = connection
        .query_row(
            "SELECT graph_revision, barrier_active FROM workflow_graph_state WHERE graph_id=?1",
            params![graph_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let (graph_revision, barrier_active) = match row {
        Some((revision, active)) => (Some(revision.max(0) as u64), active == 1),
        None => (None, false),
    };
    Ok(GraphBarrierState {
        barrier_active,
        graph_revision,
        pause_targets: targets(connection, "workflow_pause_requests", graph_id)?,
        stop_targets: targets(connection, "workflow_stop_requests", graph_id)?,
    })
}

fn targets(connection: &Connection, table: &str, graph_id: &str) -> Result<BTreeSet<String>> {
    let query = match table {
        "workflow_pause_requests" => {
            "SELECT target FROM workflow_pause_requests WHERE graph_id=?1 AND active=1"
        }
        _ => "SELECT target FROM workflow_stop_requests WHERE graph_id=?1",
    };
    let mut statement = connection.prepare(query)?;
    let rows = statement.query_map(params![graph_id], |row| row.get::<_, String>(0))?;
    let mut targets = BTreeSet::new();
    for target in rows {
        targets.insert(target?);
    }
    Ok(targets)
}

/// The identifier rule the control tables already apply: a non-empty, trimmed,
/// bounded key with no control characters.
fn validate_id(value: &str, code: &'static str) -> Result<()> {
    ensure!(
        !value.trim().is_empty()
            && value == value.trim()
            && value.len() <= 160
            && !value.chars().any(char::is_control),
        "{code}"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use licoup_workflow::{
        GraphState, GraphStateKind, ReducerEvent, RetryPolicy, Transition, TransitionEvent,
        TransitionMode, WORKFLOW_SCHEMA_VERSION, WorkflowDefinition, WorkflowLimits,
        WorkflowMetadata,
    };
    use serde_json::json;
    use uuid::Uuid;

    const REVISION: &str = "revision-1";
    const SEMANTICS: &str = "semantics-1";

    fn store() -> (StrategyStore, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!("lico-authority-adapter-{}", Uuid::new_v4()));
        (StrategyStore::open(&root).expect("the store opens"), root)
    }

    fn state(id: &str, kind: GraphStateKind) -> GraphState {
        GraphState {
            id: id.to_owned(),
            kind,
            label: id.to_owned(),
            instruction: String::new(),
            binding: None,
            runtime: None,
            entry: None,
            workset: None,
            retry: RetryPolicy::default(),
        }
    }

    /// A definition with no binding slots, so its bindings are complete and a
    /// grant can be issued for it without inventing an actor candidate. It is
    /// the smallest graph the compiler accepts: one pass-through state routing
    /// on completion to a terminal state.
    fn definition() -> WorkflowDefinition {
        WorkflowDefinition {
            schema: WORKFLOW_SCHEMA_VERSION.into(),
            metadata: WorkflowMetadata {
                id: "adapter.workflow".into(),
                name: "Adapter".into(),
                version: "1".into(),
                description: String::new(),
            },
            limits: WorkflowLimits::default(),
            actor_slots: vec![],
            runtimes: vec![],
            worksets: vec![],
            initial: "start".into(),
            states: vec![
                state("start", GraphStateKind::Pass),
                state("done", GraphStateKind::Succeed),
            ],
            transitions: vec![Transition {
                id: "ready".into(),
                from: "start".into(),
                to: "done".into(),
                event: TransitionEvent::Complete,
                mode: TransitionMode::Flow,
                guard: None,
            }],
        }
    }

    /// A definition whose entry state dispatches an actor effect, so a permit
    /// can be asked for. It is the smallest graph that actually produces a
    /// command: one bound actor state routing on success to a terminal state.
    fn dispatched_definition() -> WorkflowDefinition {
        let mut work = state("work", GraphStateKind::Actor);
        work.binding = Some("worker".to_owned());
        WorkflowDefinition {
            schema: WORKFLOW_SCHEMA_VERSION.into(),
            metadata: WorkflowMetadata {
                id: "adapter.dispatch".into(),
                name: "Adapter dispatch".into(),
                version: "1".into(),
                description: String::new(),
            },
            limits: WorkflowLimits::default(),
            actor_slots: vec![licoup_workflow::ActorSlot::required_actor(
                "worker", "Worker",
            )],
            runtimes: vec![],
            worksets: vec![],
            initial: "work".into(),
            states: vec![
                work,
                state("done", GraphStateKind::Succeed),
                state("fail", GraphStateKind::Fail),
            ],
            transitions: vec![
                Transition {
                    id: "done".into(),
                    from: "work".into(),
                    to: "done".into(),
                    event: TransitionEvent::Success,
                    mode: TransitionMode::Flow,
                    guard: None,
                },
                Transition {
                    id: "failed".into(),
                    from: "work".into(),
                    to: "fail".into(),
                    event: TransitionEvent::Failure,
                    mode: TransitionMode::Flow,
                    guard: None,
                },
            ],
        }
    }

    fn granted_store() -> (StrategyStore, String) {
        let (store, _root) = store();
        store
            .register_definition(REVISION, SEMANTICS, &definition(), 0, 0)
            .expect("the definition registers");
        let preview = store
            .authorization_preview(REVISION)
            .expect("the preview is the owner's answer");
        let granted = store
            .grant_authorization(REVISION, &preview.authorization_digest)
            .expect("the grant is recorded");
        (store, granted.authorization_digest)
    }

    /// A store with a dispatchable actor graph, a bound slot, and a grant.
    fn dispatched_granted_store() -> (StrategyStore, String) {
        let (store, _root) = store();
        store
            .register_definition(REVISION, SEMANTICS, &dispatched_definition(), 0, 0)
            .expect("the definition registers");
        store
            .update_binding(REVISION, "worker", "agent:test", "", "", None)
            .expect("the binding is recorded");
        let preview = store
            .authorization_preview(REVISION)
            .expect("the preview is the owner's answer");
        let granted = store
            .grant_authorization(REVISION, &preview.authorization_digest)
            .expect("the grant is recorded");
        (store, granted.authorization_digest)
    }

    #[test]
    fn a_grant_resolves_only_while_it_is_in_force() {
        let (store, digest) = granted_store();
        let adapter = StoreAuthorityAdapter::new(store.clone());
        let resolved = adapter
            .active_grant(REVISION)
            .expect("resolution reads the owner")
            .expect("a granted revision resolves");
        assert_eq!(resolved.authorization_digest, digest);
        assert_eq!(resolved.semantics_digest, SEMANTICS);
        assert!(resolved.active);

        adapter.revoke(REVISION).expect("the revocation records");
        assert!(
            adapter
                .active_grant(REVISION)
                .expect("resolution reads the owner")
                .is_none(),
            "a revoked grant must not resolve to an authority"
        );
        // The preview still answers, and a new grant is a different digest:
        // authority is per grant, not per revision.
        let next = adapter
            .preview(REVISION)
            .expect("the preview is the owner's answer");
        let regranted = adapter
            .grant(REVISION, &next.authorization_digest)
            .expect("re-granting records a new grant");
        assert_ne!(regranted.authorization_digest, digest);
    }

    #[test]
    fn a_recheck_names_which_fact_moved() {
        let (store, first) = granted_store();
        let adapter = StoreAuthorityAdapter::new(store.clone());
        let run = store
            .start_run(REVISION, json!({}), "idempotency-1", None, None)
            .expect("the run starts under its grant");
        let request = GrantRecheck {
            run_id: run.run_id.clone(),
            expected_authorization_digest: first.clone(),
            expected_semantics_digest: SEMANTICS.to_owned(),
        };
        assert_eq!(
            adapter.recheck(&request).expect("the recheck answers"),
            GrantVerdict::Covered {
                authorization_digest: first.clone()
            }
        );

        // A different grant for the same revision: the caller's expectation is
        // stale, and the answer names both digests.
        adapter.revoke(REVISION).expect("the revocation records");
        let preview = adapter.preview(REVISION).expect("the preview answers");
        let second = adapter
            .grant(REVISION, &preview.authorization_digest)
            .expect("re-granting records a new grant");
        assert_eq!(
            adapter.recheck(&request).expect("the recheck answers"),
            GrantVerdict::Replaced {
                expected: first,
                observed: second.authorization_digest,
            }
        );

        // No grant in force: the next effect is refused rather than admitted
        // under the previous one.
        adapter.revoke(REVISION).expect("the revocation records");
        assert_eq!(
            adapter.recheck(&request).expect("the recheck answers"),
            GrantVerdict::NoneInForce {
                revision_digest: REVISION.to_owned()
            }
        );
    }

    #[test]
    fn an_effect_permit_goes_through_the_store_admission_and_its_lease() {
        let (store, digest) = granted_store();
        let adapter = StoreAuthorityAdapter::new(store.clone());
        let run = store
            .start_run(REVISION, json!({}), "idempotency-2", None, None)
            .expect("the run starts under its grant");
        // A permit for a command this owner does not hold has to be refused by
        // the store's own admission — the lease check — not by a second rule
        // invented here.
        let error = adapter
            .permit_effect(&EffectPermit {
                run_id: run.run_id.clone(),
                effect_id: "command-1".to_owned(),
                attempt_token: "attempt-1".to_owned(),
                expected_authorization_digest: digest,
                claimant: "host-1#1".to_owned(),
                lease_until_unix_ms: 4_000_000_000_000,
            })
            .expect_err("a permit without the command's lease must be refused");
        assert_eq!(error.to_string(), "strategy_lease_lost");
    }

    #[test]
    fn a_permit_is_issued_under_the_grant_in_force_and_a_later_revoke_stops_only_the_next_one() {
        let (store, digest) = dispatched_granted_store();
        let adapter = StoreAuthorityAdapter::new(store.clone());
        let run = store
            .start_run(REVISION, json!({}), "idempotency-permit", None, None)
            .expect("the run starts under its grant");
        let command = store
            .claim_next_command(&run.run_id, "host-1#1", 4_000_000_000_000)
            .expect("the claim answers")
            .expect("an effect is dispatchable");
        store
            .apply_event(
                &run.run_id,
                ReducerEvent::CommandStarted {
                    command_id: command.id.clone(),
                    attempt_token: command.attempt_token.clone(),
                },
            )
            .expect("the started marker commits before the effect");

        let recheck = GrantRecheck {
            run_id: run.run_id.clone(),
            expected_authorization_digest: digest.clone(),
            expected_semantics_digest: SEMANTICS.to_owned(),
        };
        assert_eq!(
            adapter.recheck(&recheck).expect("the recheck answers"),
            GrantVerdict::Covered {
                authorization_digest: digest.clone()
            }
        );

        let permit = EffectPermit {
            run_id: run.run_id.clone(),
            effect_id: command.id.clone(),
            attempt_token: command.attempt_token.clone(),
            expected_authorization_digest: digest,
            claimant: "host-1#1".to_owned(),
            lease_until_unix_ms: 4_000_000_000_000,
        };
        adapter
            .permit_effect(&permit)
            .expect("a claimed command under a live grant is permitted");

        // The revocation happens after the permit was issued. It does not
        // unmake that permit — the effect it admitted keeps its binding — and
        // it stops the next one.
        adapter.revoke(REVISION).expect("the revocation records");
        assert_eq!(
            adapter.recheck(&recheck).expect("the recheck answers"),
            GrantVerdict::NoneInForce {
                revision_digest: REVISION.to_owned()
            }
        );
        assert_eq!(
            adapter
                .permit_effect(&permit)
                .expect_err("the next permit must be refused")
                .to_string(),
            "strategy_authorization_required"
        );
    }

    #[test]
    fn a_graph_pause_writes_the_barrier_and_its_recipients_in_one_write() {
        let (store, root) = store();
        let barrier = StoreScopeBarrier::new(store.clone());
        assert!(
            barrier
                .state("graph-1")
                .expect("the state reads")
                .admits_new_visits()
        );

        let published = barrier
            .publish(&BarrierRequest {
                graph_id: "graph-1".to_owned(),
                scope: BarrierScope::Graph,
                kind: BarrierKind::Pause,
                reason: "operator paused the graph".to_owned(),
                recipients: vec!["node-a".to_owned(), "node-b".to_owned()],
            })
            .expect("the barrier is written");
        assert!(published.barrier_active);
        // The scope key is recorded alongside its recipients, exactly as the
        // controlled store's own pause path records a graph pause.
        assert_eq!(
            published.pause_targets,
            ["graph".to_owned(), "node-a".to_owned(), "node-b".to_owned()]
                .into_iter()
                .collect()
        );
        // The barrier and the freeze came from one write, so the read-back
        // inside that write already saw both.
        assert!(published.pause_targets.contains("node-b"));
        assert!(
            !barrier
                .state("graph-1")
                .expect("the state reads")
                .admits_new_visits()
        );

        // Both facts are durable: a second handle on the same root sees them.
        let reopened = StoreScopeBarrier::new(StrategyStore::open(&root).expect("reopen"));
        let state = reopened.state("graph-1").expect("the state reads");
        assert!(state.barrier_active);
        assert_eq!(state.pause_targets.len(), 3);
        assert!(state.graph_revision.is_some());
    }

    #[test]
    fn a_node_pause_freezes_its_targets_without_fencing_new_visits() {
        let (store, _root) = store();
        let barrier = StoreScopeBarrier::new(store);
        let published = barrier
            .publish(&BarrierRequest {
                graph_id: "graph-2".to_owned(),
                scope: BarrierScope::Node("node-a".to_owned()),
                kind: BarrierKind::Pause,
                reason: "drain this node".to_owned(),
                recipients: vec!["node-a".to_owned()],
            })
            .expect("the barrier is written");
        // C03 gives the new-start fence to the graph scope: a node instruction
        // acts on its recipients and fences nothing new.
        assert!(!published.barrier_active);
        assert_eq!(
            published.pause_targets,
            ["node-a".to_owned()].into_iter().collect()
        );
        assert!(
            barrier
                .state("graph-2")
                .expect("the state reads")
                .admits_new_visits()
        );
    }

    #[test]
    fn a_stop_is_monotonic_and_a_later_pause_does_not_reopen_the_scope() {
        let (store, _root) = store();
        let barrier = StoreScopeBarrier::new(store);
        let stopped = barrier
            .publish(&BarrierRequest {
                graph_id: "graph-3".to_owned(),
                scope: BarrierScope::Graph,
                kind: BarrierKind::Stop,
                reason: "operator stopped the graph".to_owned(),
                recipients: vec!["node-a".to_owned()],
            })
            .expect("the stop is written");
        // A stop is recorded where a stop is read, for the scope key and for
        // each recipient; it writes no pause request.
        assert!(stopped.barrier_active);
        assert_eq!(
            stopped.stop_targets,
            ["graph".to_owned(), "node-a".to_owned()]
                .into_iter()
                .collect()
        );
        assert!(stopped.pause_targets.is_empty());

        let later = barrier
            .publish(&BarrierRequest {
                graph_id: "graph-3".to_owned(),
                scope: BarrierScope::Graph,
                kind: BarrierKind::Pause,
                reason: "a pause arrived afterwards".to_owned(),
                recipients: vec![],
            })
            .expect("the pause is written");

        assert!(later.barrier_active);
        let state = barrier.state("graph-3").expect("the state reads");
        assert!(state.barrier_active);
        // The pause did not clear the stop, and the stop did not block the
        // pause from being recorded: neither write unsets the other.
        assert!(state.stop_targets.contains("graph"));
        assert_eq!(
            state.pause_targets,
            ["graph".to_owned()].into_iter().collect()
        );
    }
}
