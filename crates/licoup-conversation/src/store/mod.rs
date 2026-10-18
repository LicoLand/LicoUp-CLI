use super::{
    Conversation, ConversationArchiveReport, ConversationClearReport, ConversationDispatch,
    ConversationEvent, ConversationSummary, DEFAULT_LOCAL_AGENT_GROUP_ID,
    DEFAULT_LOCAL_AGENT_GROUP_TITLE, DirectTurn, DispatchSessionMode, DispatchState, EventKind,
    EventPage, EventPart, EventPartKind, ImageAttachment, LICOUP_GUIDE_SKILL_ID, Membership,
    MembershipAccess, Principal, PrincipalKind, PrivateRuntimeBinding, ProfileIntent,
    ProfileIntentUpdate, ProfileResponsibility, RuntimeBinding, SourceLink, TurnState,
};
use anyhow::{Result, anyhow};
use rusqlite::{
    Connection, OptionalExtension, Row, Statement, TransactionBehavior, params, params_from_iter,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::collections::{BTreeSet, HashMap};
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};
#[cfg(any(test, feature = "test-counters"))]
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

mod admission;
mod admission_keys;
mod continuity_seam;
mod conversations;
mod dispatches;
mod events;
mod execution;
mod lifecycle;
mod native_sessions;
mod path_security;
mod recovery;
mod schema;

#[cfg(test)]
use schema::CONVERSATION_SCHEMA_TABLES;
use schema::{configure_connection, initialize_schema, preflight_schema, validate_current_schema};

pub use admission::{
    LocalWorkBlocker, LocalWorkKind, MAX_UNFINISHED_LOCAL_WORK, UnfinishedLocalWork,
    local_work_database_path, read_unfinished_local_work,
};
pub use admission_keys::EventAppendOutcome;
pub use continuity_seam::ContinuityUnitOfWork;
pub use conversations::ConversationRepository;
pub use dispatches::{DispatchRepository, MAX_SUBAGENT_INVOCATION_DEPTH};
pub use events::{EventPagePosition, EventRepository};
pub use execution::{
    ExecutionRecord, NativeExecutionReference, NativeExecutionReferenceIndex,
    RuntimeExecutionSnapshot, RuntimeFrameRecord,
};
pub use native_sessions::NativeSessionReference;
pub use recovery::{ColdRecoverableConversationStore, ColdRecoveryReport};
pub use schema::validate_migration_source;

pub const DEFAULT_EVENT_PAGE_SIZE: usize = 20;
pub const MAX_EVENT_PAGE_SIZE: usize = 100;
/// Bounded conversation SQLite pool size. Long-running processes reuse at
/// most this many configured connections; acquisition blocks on a condition
/// variable instead of opening unbounded connections.
pub const DEFAULT_CONVERSATION_POOL_SIZE: usize = 4;
pub type StoreError = anyhow::Error;
pub type StoreResult<T> = Result<T, StoreError>;

const DATABASE_FILE: &str = "conversations.sqlite3";
pub const CURRENT_SCHEMA_VERSION: &str = "19";

#[derive(Clone, Debug)]
pub struct ConversationStore {
    pool: Arc<ConversationPool>,
}

/// Owned bounded SQLite state for one long-running Conversation authority.
/// Connections are configured (WAL, foreign keys, timeout, hardening) once at
/// creation and are checked out only for local database work; a checkout can
/// never escape into agent, network, or subprocess callbacks because it is
/// scoped to the `with_connection` closure.
#[derive(Debug)]
struct ConversationPool {
    db_path: PathBuf,
    state: Mutex<PoolState>,
    available: Condvar,
    #[cfg(any(test, feature = "test-counters"))]
    counters: Arc<ConversationCounters>,
}

#[derive(Debug, Default)]
struct PoolState {
    idle: Vec<Connection>,
    in_flight: usize,
    opened: usize,
}

/// Privacy-safe counters for the focused suite: connection opens, SQL query
/// statements, pool leases, and in-flight direct turns. They never carry
/// content, paths, credentials, or runtime data, and their read accessors are
/// test-only so production never reports them.
#[cfg(any(test, feature = "test-counters"))]
#[derive(Debug, Default)]
#[doc(hidden)]
pub struct ConversationCounters {
    opens: AtomicUsize,
    queries: AtomicUsize,
    leases: AtomicUsize,
    peak_in_flight: AtomicUsize,
    in_flight_turns: AtomicUsize,
    peak_in_flight_turns: AtomicUsize,
}

#[cfg(any(test, feature = "test-counters"))]
impl ConversationCounters {
    pub fn opens(&self) -> usize {
        self.opens.load(Ordering::Relaxed)
    }

    pub fn queries(&self) -> usize {
        self.queries.load(Ordering::Relaxed)
    }

    pub fn leases(&self) -> usize {
        self.leases.load(Ordering::Relaxed)
    }

    pub fn peak_in_flight(&self) -> usize {
        self.peak_in_flight.load(Ordering::Relaxed)
    }

    pub fn in_flight_turns(&self) -> usize {
        self.in_flight_turns.load(Ordering::Relaxed)
    }

    pub fn peak_in_flight_turns(&self) -> usize {
        self.peak_in_flight_turns.load(Ordering::Relaxed)
    }

    pub fn begin_turn(&self) {
        let in_flight = self.in_flight_turns.fetch_add(1, Ordering::Relaxed) + 1;
        self.peak_in_flight_turns
            .fetch_max(in_flight, Ordering::Relaxed);
    }

    pub fn end_turn(&self) {
        self.in_flight_turns.fetch_sub(1, Ordering::Relaxed);
    }
}

/// RAII checkout of one pooled connection. Returning the connection to the
/// pool happens on drop, so a lease can never span a runtime callback.
struct PooledConnection {
    pool: Arc<ConversationPool>,
    connection: Option<Connection>,
}

impl Deref for PooledConnection {
    type Target = Connection;

    fn deref(&self) -> &Self::Target {
        self.connection.as_ref().expect("pooled connection present")
    }
}

impl DerefMut for PooledConnection {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.connection.as_mut().expect("pooled connection present")
    }
}

impl Drop for PooledConnection {
    fn drop(&mut self) {
        if let Some(connection) = self.connection.take() {
            self.pool.release(connection);
        }
    }
}

impl ConversationPool {
    fn new(db_path: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            db_path,
            state: Mutex::new(PoolState::default()),
            available: Condvar::new(),
            #[cfg(any(test, feature = "test-counters"))]
            counters: Arc::new(ConversationCounters::default()),
        })
    }

    fn acquire(self: &Arc<Self>) -> StoreResult<PooledConnection> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let connection = loop {
            if let Some(connection) = state.idle.pop() {
                break connection;
            }
            if state.in_flight < DEFAULT_CONVERSATION_POOL_SIZE {
                let connection = match open_pooled_connection(&self.db_path) {
                    Ok(connection) => connection,
                    Err(error) => {
                        self.available.notify_all();
                        return Err(error);
                    }
                };
                state.opened += 1;
                #[cfg(any(test, feature = "test-counters"))]
                self.counters.opens.fetch_add(1, Ordering::Relaxed);
                break connection;
            }
            state = self
                .available
                .wait(state)
                .unwrap_or_else(|poison| poison.into_inner());
        };
        state.in_flight += 1;
        #[cfg(any(test, feature = "test-counters"))]
        {
            self.counters.leases.fetch_add(1, Ordering::Relaxed);
            self.counters
                .peak_in_flight
                .fetch_max(state.in_flight, Ordering::Relaxed);
        }
        Ok(PooledConnection {
            pool: Arc::clone(self),
            connection: Some(connection),
        })
    }

    fn release(&self, connection: Connection) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        state.idle.push(connection);
        state.in_flight -= 1;
        self.available.notify_one();
    }
}

fn open_pooled_connection(db_path: &Path) -> StoreResult<Connection> {
    path_security::validate_existing_private_file(db_path)?;
    let connection = Connection::open(db_path)
        .map_err(|error| anyhow!("conversation_database_open_failed: {error}"))?;
    preflight_schema(&connection)?;
    configure_connection(&connection)?;
    path_security::harden_private_path(db_path)?;
    for suffix in ["-wal", "-shm"] {
        let mut sidecar = db_path.as_os_str().to_os_string();
        sidecar.push(suffix);
        path_security::harden_private_path(Path::new(&sidecar))?;
    }
    Ok(connection)
}

fn preflight_database_path(db_path: &Path, migration_admission: bool) -> StoreResult<bool> {
    let exists = path_security::validate_existing_private_file(db_path)?;
    let mut sidecars_exist = false;
    for suffix in ["-wal", "-shm"] {
        let mut sidecar = db_path.as_os_str().to_os_string();
        sidecar.push(suffix);
        sidecars_exist |= path_security::validate_existing_private_file(Path::new(&sidecar))?;
    }
    if !exists {
        if sidecars_exist {
            return Err(anyhow!("conversation_database_sidecar_without_database"));
        }
        return Ok(false);
    }

    let connection =
        Connection::open_with_flags(db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|_| anyhow!("conversation_database_preflight_failed"))?;
    let schema_version = preflight_schema(&connection)?;
    if !migration_admission
        && schema_version
            .as_deref()
            .is_some_and(|version| version != CURRENT_SCHEMA_VERSION)
    {
        return Err(anyhow!("conversation_schema_migration_required"));
    }
    Ok(true)
}

/// Query-counting view over one checked-out connection. It counts every SQL
/// statement preparation or execution on the pooled connection so the focused
/// suite can prove query counts are independent of page size.
struct CountedConnection<'a> {
    connection: &'a mut Connection,
    #[cfg(any(test, feature = "test-counters"))]
    counters: &'a ConversationCounters,
}

impl<'a> CountedConnection<'a> {
    fn new(
        connection: &'a mut Connection,
        #[cfg(any(test, feature = "test-counters"))] counters: &'a ConversationCounters,
    ) -> Self {
        Self {
            connection,
            #[cfg(any(test, feature = "test-counters"))]
            counters,
        }
    }

    fn count(&self) {
        #[cfg(any(test, feature = "test-counters"))]
        self.counters.queries.fetch_add(1, Ordering::Relaxed);
    }

    fn query_row<T, P, F>(&self, sql: &str, params: P, f: F) -> rusqlite::Result<T>
    where
        P: rusqlite::Params,
        F: FnOnce(&Row<'_>) -> rusqlite::Result<T>,
    {
        self.count();
        self.connection.query_row(sql, params, f)
    }

    fn execute<P: rusqlite::Params>(&self, sql: &str, params: P) -> rusqlite::Result<usize> {
        self.count();
        self.connection.execute(sql, params)
    }

    fn transaction_with_behavior(
        &mut self,
        behavior: TransactionBehavior,
    ) -> rusqlite::Result<CountedTransaction<'_>> {
        self.count();
        let transaction = self.connection.transaction_with_behavior(behavior)?;
        Ok(CountedTransaction {
            transaction,
            #[cfg(any(test, feature = "test-counters"))]
            counters: self.counters,
        })
    }

    fn unchecked_transaction(&mut self) -> rusqlite::Result<CountedTransaction<'_>> {
        self.count();
        let transaction = self.connection.unchecked_transaction()?;
        Ok(CountedTransaction {
            transaction,
            #[cfg(any(test, feature = "test-counters"))]
            counters: self.counters,
        })
    }
}

impl<'a> Deref for CountedConnection<'a> {
    type Target = Connection;

    fn deref(&self) -> &Self::Target {
        self.connection
    }
}

impl<'a> DerefMut for CountedConnection<'a> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.connection
    }
}

struct CountedTransaction<'a> {
    transaction: rusqlite::Transaction<'a>,
    #[cfg(any(test, feature = "test-counters"))]
    counters: &'a ConversationCounters,
}

impl<'a> CountedTransaction<'a> {
    fn count(&self) {
        #[cfg(any(test, feature = "test-counters"))]
        self.counters.queries.fetch_add(1, Ordering::Relaxed);
    }

    fn query_row<T, P, F>(&self, sql: &str, params: P, f: F) -> rusqlite::Result<T>
    where
        P: rusqlite::Params,
        F: FnOnce(&Row<'_>) -> rusqlite::Result<T>,
    {
        self.count();
        self.transaction.query_row(sql, params, f)
    }

    fn execute<P: rusqlite::Params>(&self, sql: &str, params: P) -> rusqlite::Result<usize> {
        self.count();
        self.transaction.execute(sql, params)
    }

    fn commit(self) -> rusqlite::Result<()> {
        self.count();
        self.transaction.commit()
    }
}

impl Deref for CountedTransaction<'_> {
    type Target = Connection;

    fn deref(&self) -> &Self::Target {
        &self.transaction
    }
}

/// Counting view shared by checked-out connections and their transactions so
/// inner helpers accept either without bypassing statement counting. Every
/// SQL statement prepared or executed through this view increments the
/// privacy-safe query counter once; the counter carries no content.
trait CountedSqlite {
    fn prepare(&self, sql: &str) -> rusqlite::Result<Statement<'_>>;
    fn query_row<T, P, F>(&self, sql: &str, params: P, f: F) -> rusqlite::Result<T>
    where
        P: rusqlite::Params,
        F: FnOnce(&Row<'_>) -> rusqlite::Result<T>;
    fn execute<P: rusqlite::Params>(&self, sql: &str, params: P) -> rusqlite::Result<usize>;
}

impl CountedSqlite for Connection {
    // A read-only connection has no counter: readers that must not open,
    // initialise or recover the canonical store still share the same SQL.
    fn prepare(&self, sql: &str) -> rusqlite::Result<Statement<'_>> {
        self.prepare(sql)
    }

    fn query_row<T, P, F>(&self, sql: &str, params: P, f: F) -> rusqlite::Result<T>
    where
        P: rusqlite::Params,
        F: FnOnce(&Row<'_>) -> rusqlite::Result<T>,
    {
        self.query_row(sql, params, f)
    }

    fn execute<P: rusqlite::Params>(&self, sql: &str, params: P) -> rusqlite::Result<usize> {
        self.execute(sql, params)
    }
}

impl CountedSqlite for CountedConnection<'_> {
    fn prepare(&self, sql: &str) -> rusqlite::Result<Statement<'_>> {
        self.count();
        self.connection.prepare(sql)
    }

    fn query_row<T, P, F>(&self, sql: &str, params: P, f: F) -> rusqlite::Result<T>
    where
        P: rusqlite::Params,
        F: FnOnce(&Row<'_>) -> rusqlite::Result<T>,
    {
        self.count();
        self.connection.query_row(sql, params, f)
    }

    fn execute<P: rusqlite::Params>(&self, sql: &str, params: P) -> rusqlite::Result<usize> {
        self.count();
        self.connection.execute(sql, params)
    }
}

impl CountedSqlite for CountedTransaction<'_> {
    fn prepare(&self, sql: &str) -> rusqlite::Result<Statement<'_>> {
        self.count();
        self.transaction.prepare(sql)
    }

    fn query_row<T, P, F>(&self, sql: &str, params: P, f: F) -> rusqlite::Result<T>
    where
        P: rusqlite::Params,
        F: FnOnce(&Row<'_>) -> rusqlite::Result<T>,
    {
        self.count();
        self.transaction.query_row(sql, params, f)
    }

    fn execute<P: rusqlite::Params>(&self, sql: &str, params: P) -> rusqlite::Result<usize> {
        self.count();
        self.transaction.execute(sql, params)
    }
}

/// One stored image part reference: the durable part identity plus its
/// decoded attachment metadata, carried to member dispatch so the wire id
/// traces back to the canonical Event Part.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImageAttachmentReference {
    pub part_id: String,
    pub attachment: ImageAttachment,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirectTurnExecutionContext {
    pub turn: DirectTurn,
    pub agent_id: String,
    pub source_content: String,
    pub source_attachments: Vec<ImageAttachmentReference>,
    pub is_assistant: bool,
    pub preferred_model: Option<String>,
    pub preferred_reasoning_effort: Option<String>,
    pub preferred_permission_mode: Option<String>,
    pub preferred_instructions: Option<String>,
    pub preferred_context_window: Option<u64>,
    pub runtime_session_id: Option<String>,
    pub runtime_conversation_path: Option<String>,
    pub working_directory: Option<String>,
}

impl DirectTurnExecutionContext {
    /// Private request-only guidance for the canonical Assistant membership.
    /// The value is derived at dispatch time and is never stored as Event text.
    pub fn private_instructions(&self) -> Option<&'static str> {
        self.is_assistant.then(super::direct_assistant_guidance)
    }
}

/// Private canonical ownership for one persistent desktop Agent turn. The
/// dispatch id is also the opaque transport handle; conversation and
/// membership ids are required on every later attach or control operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConversationRuntimeScope {
    pub dispatch_id: String,
    pub conversation_id: String,
    pub membership_id: String,
    pub event_id: String,
}

impl ConversationStore {
    pub fn open(portable_root: &Path) -> StoreResult<Self> {
        let root = portable_root.join("client-state").join("conversations");
        let db_path = root.join(DATABASE_FILE);
        let existed = preflight_database_path(&db_path, false)?;
        path_security::ensure_private_dir(&portable_root.join("client-state"))?;
        path_security::ensure_private_dir(&root)?;
        let store = Self {
            pool: ConversationPool::new(db_path),
        };
        store.with_connection(|connection| {
            if existed {
                validate_current_schema(connection)
            } else {
                initialize_schema(connection)
            }
        })?;
        store.cold_recover()?;
        Ok(store)
    }

    /// Upgrade a published canonical store to the current development format.
    /// Ordinary readers use [`Self::open`] and therefore cannot mutate
    /// an older schema before startup admission owns the transition.
    #[doc(hidden)]
    pub fn open_for_migration(portable_root: &Path) -> StoreResult<Self> {
        let root = portable_root.join("client-state").join("conversations");
        let db_path = root.join(DATABASE_FILE);
        preflight_database_path(&db_path, true)?;
        path_security::ensure_private_dir(&portable_root.join("client-state"))?;
        path_security::ensure_private_dir(&root)?;
        let store = Self {
            pool: ConversationPool::new(db_path),
        };
        store.with_connection(|connection| initialize_schema(connection))?;
        store.cold_recover()?;
        Ok(store)
    }

    /// Open an isolated in-memory store for deterministic domain tests.
    pub fn open_in_memory() -> StoreResult<Self> {
        let path =
            std::env::temp_dir().join(format!("lico-conversation-{}.sqlite3", Uuid::new_v4()));
        let store = Self {
            pool: ConversationPool::new(path),
        };
        store.with_connection(|connection| initialize_schema(connection))?;
        Ok(store)
    }

    /// Flush WAL into the main file and fsync before resetting the log.
    /// `synchronous=FULL` makes this checkpoint durable; TRUNCATE then drops
    /// the WAL so a later SIGKILL cannot replay a torn main file.
    pub fn checkpoint(&self) -> StoreResult<()> {
        self.with_connection(|connection| {
            connection.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
            Ok(())
        })
    }

    /// Reuse the idempotent reserved-default-group normalization after a
    /// legacy import may have adopted the historical default group, before
    /// the legacy completion marker is written. Naturally idempotent: events
    /// already removed stay absent, already-active memberships stay active,
    /// already-contiguous sequences are rewritten to the same values, and the
    /// retired default title is migrated once to the current product name.
    #[doc(hidden)]
    pub fn normalize_reserved_default_group_after_legacy_import(&self) -> StoreResult<()> {
        self.with_connection(|connection| normalize_reserved_group_after_legacy_import(connection))
    }

    /// Ensure the product-owned Local group exists in the canonical
    /// Conversation store. Legacy imports are adopted into the stable identity
    /// instead of creating a second group or reopening the retired JSON store.
    #[doc(hidden)]
    pub fn ensure_default_local_group(&self) -> StoreResult<Conversation> {
        self.with_connection(ensure_default_local_group_inner)
    }

    pub fn db_path(&self) -> &Path {
        &self.pool.db_path
    }

    fn with_connection<T>(
        &self,
        operation: impl FnOnce(&mut CountedConnection<'_>) -> StoreResult<T>,
    ) -> StoreResult<T> {
        let mut connection = self.pool.acquire()?;
        let mut counted = CountedConnection::new(
            &mut connection,
            #[cfg(any(test, feature = "test-counters"))]
            &self.pool.counters,
        );
        operation(&mut counted)
    }

    /// Test-only view of the pool counters shared by every clone of this
    /// store, so the focused suite can assert reuse and query bounds.
    #[cfg(any(test, feature = "test-counters"))]
    #[doc(hidden)]
    pub fn counters(&self) -> &Arc<ConversationCounters> {
        &self.pool.counters
    }

    pub fn ensure_principal(&self, principal: &Principal) -> StoreResult<()> {
        validate_identifier(&principal.id, "principal_id")?;
        validate_required_text(&principal.display_name, "principal_display_name")?;
        self.with_connection(|connection| upsert_principal(connection, principal))
    }

    /// Prepare one persistent runtime dispatch and commit its owning
    /// Conversation facts before native execution starts. Direct Agent sends
    /// receive (or reuse) a canonical one-Agent Conversation; group sends pass
    /// their existing Conversation and Membership and therefore do not create
    /// a competing transcript.
    pub fn prepare_runtime_dispatch(
        &self,
        agent_id: &str,
        session_id: &str,
        text: &str,
        conversation_id: Option<&str>,
        membership_id: Option<&str>,
        causation_id: Option<&str>,
        requested_dispatch_id: Option<&str>,
    ) -> StoreResult<ConversationRuntimeScope> {
        validate_identifier(agent_id, "runtime_agent_id")?;
        validate_required_text(text, "runtime_user_text")?;
        if conversation_id.is_some() != membership_id.is_some() {
            return Err(anyhow!("runtime_scope_incomplete"));
        }
        let dispatch_id = if let Some(requested) = requested_dispatch_id {
            validate_identifier(requested, "dispatch_id")?;
            requested.to_owned()
        } else {
            new_id("dispatch")
        };
        let now = now_ms();
        self.with_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let (conversation_id, membership_id, append_user) =
                if let (Some(conversation_id), Some(membership_id)) =
                    (conversation_id, membership_id)
                {
                    validate_identifier(conversation_id, "conversation_id")?;
                    validate_identifier(membership_id, "membership_id")?;
                    let eligible: Option<i64> = transaction
                        .query_row(
                            "SELECT 1 FROM memberships m JOIN principals p ON p.id=m.principal_id
                             WHERE m.id=?1 AND m.conversation_id=?2 AND m.status='active'
                               AND p.kind='agent' AND p.agent_id=?3",
                            params![membership_id, conversation_id, agent_id],
                            |row| row.get(0),
                        )
                        .optional()?;
                    if eligible.is_none() {
                        return Err(anyhow!("runtime_scope_mismatch"));
                    }
                    (
                        conversation_id.to_owned(),
                        membership_id.to_owned(),
                        causation_id.is_none(),
                    )
                } else {
                    let source_identity = runtime_source_identity(agent_id, session_id, &dispatch_id)?;
                    let existing: Option<String> = if session_id.trim().is_empty() {
                        None
                    } else {
                        transaction
                            .query_row(
                                "SELECT conversation_id FROM migration_provenance
                                 WHERE source_kind='projection' AND source_identity=?1
                                 UNION ALL
                                 SELECT conversation_id FROM source_links
                                 WHERE source_kind='agent-runtime' AND native_identity=?1
                                 LIMIT 1",
                                params![source_identity],
                                |row| row.get(0),
                            )
                            .optional()?
                    };
                    let conversation_id = existing.unwrap_or_else(|| new_id("conversation"));
                    let owner_principal = Principal {
                        id: "human:local".to_owned(),
                        kind: PrincipalKind::Human,
                        display_name: "human:local".to_owned(),
                        agent_id: None,
                        created_at_unix_ms: now,
                    };
                    let agent_principal = Principal {
                        id: format!("agent:{agent_id}"),
                        kind: PrincipalKind::Agent,
                        display_name: agent_id.to_owned(),
                        agent_id: Some(agent_id.to_owned()),
                        created_at_unix_ms: now,
                    };
                    upsert_principal(&transaction, &owner_principal)?;
                    upsert_principal(&transaction, &agent_principal)?;
                    let exists: Option<i64> = transaction
                        .query_row(
                            "SELECT 1 FROM conversations WHERE id=?1",
                            params![conversation_id],
                            |row| row.get(0),
                        )
                        .optional()?;
                    if exists.is_none() {
                        transaction.execute(
                            "INSERT INTO conversations(
                               id, title, archived, pinned, is_group, revision, created_at, updated_at
                             ) VALUES (?1, ?2, 0, 0, 0, 0, ?3, ?3)",
                            params![conversation_id, agent_id, now],
                        )?;
                        transaction.execute(
                            "INSERT INTO memberships(
                               id, conversation_id, principal_id, access, status, joined_at, left_at
                             ) VALUES (?1, ?2, ?3, 'owner', 'active', ?4, NULL)",
                            params![new_id("membership"), conversation_id, owner_principal.id, now],
                        )?;
                    }
                    let membership_id: Option<String> = transaction
                        .query_row(
                            "SELECT m.id FROM memberships m JOIN principals p ON p.id=m.principal_id
                             WHERE m.conversation_id=?1
                               AND p.kind='agent' AND p.agent_id=?2
                             ORDER BY CASE m.status WHEN 'active' THEN 0 ELSE 1 END,
                                      m.joined_at DESC, m.id DESC
                             LIMIT 1",
                            params![conversation_id, agent_id],
                            |row| row.get(0),
                        )
                        .optional()?;
                    let membership_id = if let Some(membership_id) = membership_id {
                        let active = lifecycle::reactivate_membership();
                        transaction.execute(
                            "UPDATE memberships
                             SET status=?2, left_at=NULL
                             WHERE id=?1 AND status=?3",
                            params![
                                membership_id,
                                active.as_str(),
                                super::MembershipStatus::Left.as_str()
                            ],
                        )?;
                        membership_id
                    } else {
                        let membership_id = new_id("membership");
                        transaction.execute(
                            "INSERT INTO memberships(
                               id, conversation_id, principal_id, access, status, joined_at, left_at
                             ) VALUES (?1, ?2, ?3, 'member', 'active', ?4, NULL)",
                            params![membership_id, conversation_id, agent_principal.id, now],
                        )?;
                        membership_id
                    };
                    if !session_id.trim().is_empty() {
                        transaction.execute(
                            "INSERT INTO migration_provenance(source_kind, source_identity, conversation_id)
                             VALUES ('projection', ?1, ?2)
                             ON CONFLICT(source_kind, source_identity) DO NOTHING",
                            params![source_identity, conversation_id],
                        )?;
                        transaction.execute(
                            "INSERT INTO source_links(id, conversation_id, source_kind, native_identity)
                             VALUES (?1, ?2, 'agent-runtime', ?3)
                             ON CONFLICT(source_kind, native_identity) DO NOTHING",
                            params![new_id("source"), conversation_id, source_identity],
                        )?;
                    }
                    (conversation_id, membership_id, true)
                };

            transaction.execute(
                "INSERT INTO conversation_dispatches(
                   id, conversation_id, membership_id, operation, state, session_mode,
                   runtime_conversation_path, error_code, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, 'send', ?4, ?5, NULL, NULL, ?6, ?6)",
                params![
                    dispatch_id,
                    conversation_id,
                    membership_id,
                    lifecycle::initial_dispatch().as_str(),
                    if session_id.trim().is_empty() { "new" } else { "resume" },
                    now,
                ],
            )?;
            let turn_causation_id = if append_user {
                let owner_membership: Option<String> = transaction
                    .query_row(
                        "SELECT m.id FROM memberships m JOIN principals p ON p.id=m.principal_id
                         WHERE m.conversation_id=?1 AND m.status='active' AND p.kind='human'
                         ORDER BY m.joined_at, m.id LIMIT 1",
                        params![conversation_id],
                        |row| row.get(0),
                    )
                    .optional()?;
                let user_event_id = new_id("event");
                insert_event(
                    &transaction,
                    &user_event_id,
                    &conversation_id,
                    owner_membership.as_deref(),
                    EventKind::Message,
                    &[NewEventPart {
                        id: String::new(),
                        kind: EventPartKind::Text,
                        content: text.to_owned(),
                    }],
                    causation_id,
                    Some(&dispatch_id),
                    true,
                    false,
                    now,
                )?;
                Some(user_event_id)
            } else {
                causation_id.map(str::to_owned)
            };
            let event_id = new_id("event");
            insert_event(
                &transaction,
                &event_id,
                &conversation_id,
                Some(&membership_id),
                EventKind::Message,
                &[],
                turn_causation_id.as_deref(),
                Some(&dispatch_id),
                false,
                false,
                now,
            )?;
            transaction.commit()?;
            Ok(ConversationRuntimeScope {
                dispatch_id,
                conversation_id,
                membership_id,
                event_id,
            })
        })
    }

    /// Record the host-admitted response mode for this actual PersistentTurn.
    /// Model JSON and caller flags cannot set this marker.
    pub fn admit_runtime_response_mode(
        &self,
        scope: &ConversationRuntimeScope,
        mode: &str,
    ) -> StoreResult<()> {
        let mode = mode.trim();
        if mode.is_empty() {
            return Err(anyhow!("runtime_response_mode_invalid"));
        }
        let content = crate::continuity::trusted_response_mode_metadata(mode);
        let now = now_ms();
        self.with_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let exists: Option<i64> = transaction
                .query_row(
                    "SELECT 1 FROM events
                     WHERE id=?1 AND conversation_id=?2 AND correlation_id=?3
                       AND author_membership_id=?4 AND finalized=0",
                    params![
                        scope.event_id,
                        scope.conversation_id,
                        scope.dispatch_id,
                        scope.membership_id,
                    ],
                    |row| row.get(0),
                )
                .optional()?;
            if exists.is_none() {
                return Err(anyhow!("runtime_dispatch_not_active"));
            }
            let already = load_trusted_response_mode(&transaction, &scope.event_id)?;
            if already.as_deref() == Some(mode) {
                transaction.commit()?;
                return Ok(());
            }
            if already.is_some() {
                return Err(anyhow!("runtime_response_mode_conflict"));
            }
            let ordinal: i64 = transaction.query_row(
                "SELECT COALESCE(MAX(ordinal), -1)+1 FROM event_parts WHERE event_id=?1",
                params![scope.event_id],
                |row| row.get(0),
            )?;
            insert_runtime_event_part(
                &transaction,
                &scope.conversation_id,
                &scope.event_id,
                ordinal,
                &NewEventPart {
                    id: String::new(),
                    kind: EventPartKind::Metadata,
                    content,
                },
                None,
                now,
            )?;
            transaction.commit()?;
            Ok(())
        })
    }

    pub fn runtime_response_mode(
        &self,
        scope: &ConversationRuntimeScope,
    ) -> StoreResult<Option<String>> {
        self.with_connection(|connection| load_trusted_response_mode(connection, &scope.event_id))
    }

    /// Replay cursors are allocated only for frames this store actually
    /// persists. User-speech is already a Canonical Message Event and must
    /// not occupy a runtime cursor.
    pub fn runtime_frame_commits_cursor(frame: &Value) -> bool {
        !runtime_frame_is_user_speech(frame)
    }

    /// Commit one replayable frame before it is published to any observer.
    /// Large frames are split across canonical metadata parts while preserving
    /// one runtime cursor.
    pub fn append_runtime_frame(
        &self,
        scope: &ConversationRuntimeScope,
        cursor: u64,
        frame: &Value,
    ) -> StoreResult<()> {
        if cursor == 0 || cursor > i64::MAX as u64 {
            return Err(anyhow!("runtime_cursor_invalid"));
        }
        // Human speech is already one Canonical Message Event. The live
        // observer may still emit this delta, but it must never become Parts
        // on the agent-authored turn Event.
        if runtime_frame_is_user_speech(frame) {
            return Ok(());
        }
        let encoded = serde_json::to_string(frame)?;
        let parts = runtime_frame_parts(&encoded);
        let now = now_ms();
        self.with_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let state: Option<(String, i64)> = transaction
                .query_row(
                    "SELECT d.state, e.finalized FROM conversation_dispatches d
                     JOIN events e ON e.id=?4 AND e.conversation_id=d.conversation_id
                       AND e.correlation_id=d.id AND e.author_membership_id=d.membership_id
                     WHERE d.id=?1 AND d.conversation_id=?2 AND d.membership_id=?3",
                    params![
                        scope.dispatch_id,
                        scope.conversation_id,
                        scope.membership_id,
                        scope.event_id,
                    ],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            if !matches!(state.as_ref(), Some((state, 0)) if matches!(state.as_str(), "accepted" | "running")) {
                return Err(anyhow!("runtime_dispatch_not_active"));
            }
            let cursor_exists: Option<i64> = transaction
                .query_row(
                    "SELECT 1 FROM event_parts WHERE event_id=?1 AND runtime_cursor=?2 LIMIT 1",
                    params![scope.event_id, cursor as i64],
                    |row| row.get(0),
                )
                .optional()?;
            if cursor_exists.is_some() {
                return Err(anyhow!("runtime_cursor_duplicate"));
            }
            let mut ordinal: i64 = transaction.query_row(
                "SELECT COALESCE(MAX(ordinal), -1)+1 FROM event_parts WHERE event_id=?1",
                params![scope.event_id],
                |row| row.get(0),
            )?;
            let trusted_mode = load_trusted_response_mode(&transaction, &scope.event_id)?;
            let completed_snapshot = frame.get("event").and_then(Value::as_str)
                == Some("agent.message.completed");
            let message_unit_marker = frame
                .pointer("/payload/messageUnit")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(|value| serde_json::json!({"messageUnit": value}).to_string());
            for mut part in runtime_semantic_parts(frame, trusted_mode.as_deref()) {
                if completed_snapshot && part.kind == EventPartKind::Text {
                    let visible_text: String = if let Some(marker) = message_unit_marker.as_deref() {
                        transaction.query_row(
                            "SELECT COALESCE(GROUP_CONCAT(content, ''), '') FROM (
                               SELECT content FROM event_parts
                               WHERE event_id=?1 AND kind='text'
                                 AND ordinal > COALESCE((
                                   SELECT MAX(ordinal) FROM event_parts
                                   WHERE event_id=?1 AND kind='metadata' AND content=?2
                                 ), -1)
                               ORDER BY ordinal
                             )",
                            params![scope.event_id, marker],
                            |row| row.get(0),
                        )?
                    } else {
                        transaction.query_row(
                            "SELECT COALESCE(GROUP_CONCAT(content, ''), '') FROM (
                               SELECT content FROM event_parts
                               WHERE event_id=?1 AND kind='text' ORDER BY ordinal
                             )",
                            params![scope.event_id],
                            |row| row.get(0),
                        )?
                    };
                    if !visible_text.is_empty() {
                        let Some(missing_suffix) = part.content.strip_prefix(&visible_text) else {
                            continue;
                        };
                        if missing_suffix.is_empty() {
                            continue;
                        }
                        part.content = missing_suffix.to_owned();
                    }
                }
                if part.kind == EventPartKind::Metadata
                    && (part.content.contains("\"lifecycle\"")
                        || part.content.contains("\"messageUnit\""))
                    && transaction
                        .query_row(
                            "SELECT 1 FROM event_parts
                             WHERE event_id=?1 AND kind='metadata' AND content=?2 LIMIT 1",
                            params![scope.event_id, part.content],
                            |row| row.get::<_, i64>(0),
                        )
                        .optional()?
                        .is_some()
                {
                    continue;
                }
                insert_runtime_event_part(
                    &transaction,
                    &scope.conversation_id,
                    &scope.event_id,
                    ordinal,
                    &part,
                    None,
                    now,
                )?;
                ordinal += 1;
            }
            for part in parts {
                insert_runtime_event_part(
                    &transaction,
                    &scope.conversation_id,
                    &scope.event_id,
                    ordinal,
                    &part,
                    Some(cursor as i64),
                    now,
                )?;
                ordinal += 1;
            }
            let running = lifecycle::start_dispatch();
            transaction.execute(
                "UPDATE conversation_dispatches SET state=?2, updated_at=?3
                 WHERE id=?1 AND state=?4",
                params![
                    scope.dispatch_id,
                    running.as_str(),
                    now,
                    DispatchState::Accepted.as_str()
                ],
            )?;
            bump_revision(&transaction, &scope.conversation_id, now)?;
            transaction.commit()?;
            Ok(())
        })
    }

    /// Page committed frames for a single canonical turn. The composite
    /// correlation/cursor index makes fallback proportional to returned data.
    pub fn runtime_frames_after(
        &self,
        scope: &ConversationRuntimeScope,
        after_cursor: u64,
        through_cursor: u64,
        limit: usize,
    ) -> StoreResult<Vec<Value>> {
        self.runtime_raw_frames_after(scope, after_cursor, through_cursor, limit)?
            .into_iter()
            .map(|frame| serde_json::from_str(&frame.raw_text).map_err(Into::into))
            .collect()
    }

    /// Persist the terminal lifecycle and dispatch state in one canonical
    /// transaction. Terminal metadata is not a replay cursor frame.
    pub fn finish_runtime_dispatch(
        &self,
        scope: &ConversationRuntimeScope,
        terminal: &Value,
        state: DispatchState,
        error_code: Option<&str>,
    ) -> StoreResult<DispatchState> {
        if !matches!(
            state,
            DispatchState::Completed | DispatchState::Failed | DispatchState::Cancelled
        ) {
            return Err(anyhow!("runtime_terminal_state_invalid"));
        }
        let now = now_ms();
        let runtime_conversation_path = terminal
            .get("sourcePath")
            .or_else(|| terminal.get("conversationPath"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty());
        self.with_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let finalized: Option<i64> = transaction
                .query_row(
                    "SELECT finalized FROM events
                     WHERE id=?1 AND conversation_id=?2 AND correlation_id=?3
                       AND author_membership_id=?4 AND kind='message'",
                    params![
                        scope.event_id,
                        scope.conversation_id,
                        scope.dispatch_id,
                        scope.membership_id,
                    ],
                    |row| row.get(0),
                )
                .optional()?;
            if finalized != Some(0) {
                return Err(anyhow!("runtime_dispatch_not_active"));
            }
            let trusted_mode = load_trusted_response_mode(&transaction, &scope.event_id)?;
            let envelope_mode =
                crate::continuity::is_assistant_turn_response_mode(trusted_mode.as_deref());
            let terminal_output = terminal
                .get("output")
                .and_then(Value::as_str)
                .filter(|output| !output.is_empty());
            let envelope_published = if envelope_mode && state == DispatchState::Completed {
                terminal_output.and_then(crate::continuity::published_terminal_envelope)
            } else {
                None
            };
            let envelope_invalid =
                envelope_mode && state == DispatchState::Completed && envelope_published.is_none();
            let persisted_state = if envelope_invalid {
                DispatchState::Failed
            } else {
                state
            };
            let persisted_error = if envelope_invalid {
                Some(crate::continuity::ASSISTANT_TURN_INVALID_ERROR.to_owned())
            } else {
                error_code.map(str::to_owned)
            };
            let error_code = persisted_error.as_deref();
            let current_dispatch: String = transaction.query_row(
                "SELECT state FROM conversation_dispatches WHERE id=?1",
                params![scope.dispatch_id],
                |row| row.get(0),
            )?;
            let current_dispatch = DispatchState::from_name(&current_dispatch)
                .ok_or_else(|| anyhow!("runtime_dispatch_state_invalid"))?;
            if !lifecycle::dispatch_permits(current_dispatch, persisted_state) {
                return Err(anyhow!("runtime_dispatch_transition_invalid"));
            }
            let changed = transaction.execute(
                "UPDATE conversation_dispatches SET state=?2, error_code=?3, updated_at=?4,
                   runtime_conversation_path=COALESCE(?5, runtime_conversation_path),
                   terminal_payload=?6
                 WHERE id=?1 AND state=?7",
                params![
                    scope.dispatch_id,
                    enum_wire(persisted_state)?,
                    error_code,
                    now,
                    runtime_conversation_path,
                    serde_json::to_string(terminal)?,
                    current_dispatch.as_str(),
                ],
            )?;
            if changed != 1 {
                return Err(anyhow!("runtime_dispatch_not_active"));
            }
            // A dispatch id claimed through the Subagent MCP door is also the
            // lineage claim id; settle that claim in the same transaction so
            // the claims table never lingers `running` past the turn.
            dispatches::writeback_subagent_claim_terminal(
                &transaction,
                &scope.dispatch_id,
                persisted_state,
            )?;
            let direct_turn_state = match persisted_state {
                DispatchState::Completed => TurnState::Succeeded,
                DispatchState::Cancelled => TurnState::Cancelled,
                DispatchState::Failed => TurnState::Failed,
                _ => unreachable!("terminal dispatch state validated above"),
            };
            transaction.execute(
                "UPDATE direct_turns SET state=?2
                 WHERE id=?1 AND conversation_id=?3 AND membership_id=?4 AND state='running'",
                params![
                    scope.dispatch_id,
                    enum_wire(direct_turn_state)?,
                    scope.conversation_id,
                    scope.membership_id,
                ],
            )?;
            let mut ordinal: i64 = transaction.query_row(
                "SELECT COALESCE(MAX(ordinal), -1)+1 FROM event_parts WHERE event_id=?1",
                params![scope.event_id],
                |row| row.get(0),
            )?;
            let visible_text: String = transaction.query_row(
                "SELECT COALESCE(GROUP_CONCAT(content, ''), '') FROM (
                   SELECT content FROM event_parts
                   WHERE event_id=?1 AND kind='text' ORDER BY ordinal
                 )",
                params![scope.event_id],
                |row| row.get(0),
            )?;
            if persisted_state == DispatchState::Completed {
                if let Some((reply, proposal)) = envelope_published.as_ref() {
                    let missing_suffix = if visible_text.is_empty() {
                        Some(reply.as_str())
                    } else {
                        reply
                            .strip_prefix(&visible_text)
                            .filter(|suffix| !suffix.is_empty())
                    };
                    if let Some(text) = missing_suffix {
                        insert_runtime_event_part(
                            &transaction,
                            &scope.conversation_id,
                            &scope.event_id,
                            ordinal,
                            &NewEventPart {
                                id: String::new(),
                                kind: EventPartKind::Text,
                                content: text.to_owned(),
                            },
                            None,
                            now,
                        )?;
                        ordinal += 1;
                    }
                    let proposal_exists: bool = transaction.query_row(
                        "SELECT EXISTS(SELECT 1 FROM event_parts
                         WHERE event_id=?1 AND kind='metadata' AND content=?2)",
                        params![scope.event_id, proposal],
                        |row| row.get(0),
                    )?;
                    if !proposal_exists {
                        insert_runtime_event_part(
                            &transaction,
                            &scope.conversation_id,
                            &scope.event_id,
                            ordinal,
                            &NewEventPart {
                                id: String::new(),
                                kind: EventPartKind::Metadata,
                                content: proposal.clone(),
                            },
                            None,
                            now,
                        )?;
                        ordinal += 1;
                    }
                } else if let Some(output) = terminal_output {
                    let missing_suffix = if visible_text.is_empty() {
                        Some(output)
                    } else {
                        output
                            .strip_prefix(&visible_text)
                            .filter(|suffix| !suffix.is_empty())
                    };
                    if let Some(text) = missing_suffix {
                        insert_runtime_event_part(
                            &transaction,
                            &scope.conversation_id,
                            &scope.event_id,
                            ordinal,
                            &NewEventPart {
                                id: String::new(),
                                kind: EventPartKind::Text,
                                content: text.to_owned(),
                            },
                            None,
                            now,
                        )?;
                        ordinal += 1;
                    }
                }
            } else {
                // A typed MCP tool-result is already the caller-visible
                // failure. A second diagnostic part would be classified as a
                // generic caller runtime failure and hide that code.
                let tool_result_exists: bool = transaction.query_row(
                    "SELECT EXISTS(SELECT 1 FROM event_parts
                     WHERE event_id=?1 AND kind='tool-result')",
                    params![scope.event_id],
                    |row| row.get(0),
                )?;
                if !tool_result_exists {
                    insert_runtime_event_part(
                        &transaction,
                        &scope.conversation_id,
                        &scope.event_id,
                        ordinal,
                        &NewEventPart {
                            id: String::new(),
                            kind: EventPartKind::Diagnostic,
                            content: runtime_terminal_diagnostic(terminal, error_code),
                        },
                        None,
                        now,
                    )?;
                    ordinal += 1;
                }
            }
            if persisted_state == DispatchState::Completed {
                for stage in [
                    "submitted",
                    "accepted",
                    "processing",
                    "responding",
                    "completed",
                ] {
                    let content = serde_json::json!({"lifecycle": stage}).to_string();
                    let exists: bool = transaction.query_row(
                        "SELECT EXISTS(SELECT 1 FROM event_parts
                         WHERE event_id=?1 AND kind='metadata' AND content=?2)",
                        params![scope.event_id, content],
                        |row| row.get(0),
                    )?;
                    if exists {
                        continue;
                    }
                    insert_runtime_event_part(
                        &transaction,
                        &scope.conversation_id,
                        &scope.event_id,
                        ordinal,
                        &NewEventPart {
                            id: String::new(),
                            kind: EventPartKind::Metadata,
                            content,
                        },
                        None,
                        now,
                    )?;
                    ordinal += 1;
                }
            } else {
                let lifecycle =
                    serde_json::json!({"lifecycle": enum_wire(persisted_state)?}).to_string();
                insert_runtime_event_part(
                    &transaction,
                    &scope.conversation_id,
                    &scope.event_id,
                    ordinal,
                    &NewEventPart {
                        id: String::new(),
                        kind: EventPartKind::Metadata,
                        content: lifecycle,
                    },
                    None,
                    now,
                )?;
            }
            transaction.execute(
                "UPDATE events SET finalized=1 WHERE id=?1 AND finalized=0",
                params![scope.event_id],
            )?;
            bump_revision(&transaction, &scope.conversation_id, now)?;
            transaction.commit()?;
            Ok(persisted_state)
        })
    }

    /// Bind the native session discovered after dispatch to the same canonical
    /// Conversation without exposing it through public projections.
    pub fn bind_runtime_session(
        &self,
        scope: &ConversationRuntimeScope,
        agent_id: &str,
        session_id: &str,
        runtime_conversation_path: Option<&str>,
        working_directory: Option<&str>,
    ) -> StoreResult<()> {
        if session_id.trim().is_empty() {
            return Ok(());
        }
        let source_identity = runtime_source_identity(agent_id, session_id, &scope.dispatch_id)?;
        self.with_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            native_sessions::record_native_session(&transaction, &scope.conversation_id, &scope.membership_id, session_id)?;
            transaction.execute(
                "INSERT INTO migration_provenance(source_kind, source_identity, conversation_id)
                 VALUES ('projection', ?1, ?2)
                 ON CONFLICT(source_kind, source_identity) DO NOTHING",
                params![source_identity, scope.conversation_id],
            )?;
            transaction.execute(
                "INSERT INTO source_links(id, conversation_id, source_kind, native_identity)
                 VALUES (?1, ?2, 'agent-runtime', ?3)
                 ON CONFLICT(source_kind, native_identity) DO NOTHING",
                params![new_id("source"), scope.conversation_id, source_identity],
            )?;
            transaction.execute(
                "INSERT INTO runtime_bindings(
                   id, conversation_id, membership_id, lane, availability, safe_reason,
                   runtime_session_id, runtime_conversation_path, working_directory
                 ) VALUES (?1, ?2, ?3, 'conversation', 'available', NULL, ?4, ?5, ?6)
                 ON CONFLICT(conversation_id, membership_id, lane) DO UPDATE SET
                   availability='available', safe_reason=NULL,
                   runtime_session_id=excluded.runtime_session_id,
                   runtime_conversation_path=COALESCE(excluded.runtime_conversation_path, runtime_bindings.runtime_conversation_path),
                   working_directory=COALESCE(excluded.working_directory, runtime_bindings.working_directory)",
                params![
                    new_id("runtime"),
                    scope.conversation_id,
                    scope.membership_id,
                    session_id,
                    runtime_conversation_path,
                    working_directory,
                ],
            )?;
            transaction.commit()?;
            Ok(())
        })
    }

    /// Read the exact private native binding for one active Membership. No
    /// generic path heuristic or completed-turn scan participates.
    pub fn private_runtime_binding(
        &self,
        conversation_id: &str,
        membership_id: &str,
    ) -> StoreResult<Option<PrivateRuntimeBinding>> {
        validate_identifier(conversation_id, "conversation_id")?;
        validate_identifier(membership_id, "membership_id")?;
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT rb.conversation_id, rb.membership_id,
                            rb.runtime_session_id, rb.runtime_conversation_path,
                            rb.working_directory
                     FROM runtime_bindings rb
                     JOIN memberships m ON m.id=rb.membership_id
                       AND m.conversation_id=rb.conversation_id
                       AND m.status='active'
                     WHERE rb.conversation_id=?1 AND rb.membership_id=?2
                       AND rb.lane='conversation' AND rb.availability='available'
                       AND rb.runtime_session_id IS NOT NULL
                     LIMIT 1",
                    params![conversation_id, membership_id],
                    |row| {
                        Ok(PrivateRuntimeBinding {
                            conversation_id: row.get(0)?,
                            membership_id: row.get(1)?,
                            runtime_session_id: row.get(2)?,
                            runtime_conversation_path: row.get(3)?,
                            working_directory: row.get(4)?,
                        })
                    },
                )
                .optional()
                .map_err(Into::into)
        })
    }

    pub fn create_conversation(&self, title: &str, owner: Principal) -> StoreResult<Conversation> {
        self.create_conversation_record(title, owner, &[], false)
    }

    /// Create one Conversation and its initial peer Memberships in one writer
    /// transaction. The client uses this for a user-confirmed group so a
    /// failed member insert can never leave a hidden partial direct chat.
    pub fn create_conversation_with_members(
        &self,
        title: &str,
        owner: Principal,
        members: &[(Principal, MembershipAccess)],
    ) -> StoreResult<Conversation> {
        self.create_conversation_record(title, owner, members, true)
    }

    /// Create a non-group Conversation: the same membership and profile
    /// machinery without group semantics (no required Agent member, no
    /// group-only affordances). Member Agent principals still receive their
    /// default Profile Intent so direct dispatch can apply their preferences.
    pub fn create_direct_conversation_with_members(
        &self,
        title: &str,
        owner: Principal,
        members: &[(Principal, MembershipAccess)],
    ) -> StoreResult<Conversation> {
        self.create_conversation_record(title, owner, members, false)
    }

    fn create_conversation_record(
        &self,
        title: &str,
        owner: Principal,
        members: &[(Principal, MembershipAccess)],
        is_group: bool,
    ) -> StoreResult<Conversation> {
        let id = new_id("conversation");
        validate_required_text(title, "conversation_title")?;
        validate_identifier(&owner.id, "principal_id")?;
        if is_group
            && !members
                .iter()
                .any(|(principal, _)| principal.kind == PrincipalKind::Agent)
        {
            return Err(anyhow!("invalid_request"));
        }
        let mut principal_ids = std::collections::HashSet::new();
        principal_ids.insert(owner.id.as_str());
        for (principal, _) in members {
            validate_identifier(&principal.id, "principal_id")?;
            validate_required_text(&principal.display_name, "principal_display_name")?;
            if !principal_ids.insert(principal.id.as_str()) {
                return Err(anyhow!("membership_principal_duplicate"));
            }
        }
        let now = now_ms();
        self.with_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            upsert_principal(&transaction, &owner)?;
            transaction.execute(
                "INSERT INTO conversations(
                   id, title, archived, pinned, is_group, revision, created_at, updated_at
                 ) VALUES (?1, ?2, 0, 0, ?3, 0, ?4, ?4)",
                params![id, title, is_group as i64, now],
            )?;
            transaction.execute(
                "INSERT INTO memberships(id, conversation_id, principal_id, access, status, joined_at)
                 VALUES (?1, ?2, ?3, 'owner', 'active', ?4)",
                params![new_id("membership"), id, owner.id, now],
            )?;
            for (principal, access) in members {
                upsert_principal(&transaction, principal)?;
                let membership_id = new_id("membership");
                transaction.execute(
                    "INSERT INTO memberships(id, conversation_id, principal_id, access, status, joined_at)
                     VALUES (?1, ?2, ?3, ?4, 'active', ?5)",
                    params![membership_id, id, principal.id, enum_wire(*access)?, now],
                )?;
                if principal.kind == PrincipalKind::Agent {
                    ensure_membership_profile_default(&transaction, &membership_id, now)?;
                }
                append_domain_event(
                    &transaction,
                    &id,
                    EventKind::MembershipChanged,
                    serde_json::json!({
                        "membershipId": membership_id,
                        "principalId": principal.id,
                        "change": "joined",
                    }),
                    now,
                )?;
            }
            if !members.is_empty() {
                bump_revision(&transaction, &id, now)?;
            }
            transaction.commit()?;
            self.get(&id)
        })
    }

    /// Create a Conversation with a caller-supplied stable identity. This is
    /// reserved for one-time migration/source adoption; normal product calls
    /// should use `create_conversation`.
    pub fn create_conversation_with_id(
        &self,
        id: &str,
        title: &str,
        owner: Principal,
    ) -> StoreResult<Conversation> {
        self.create_conversation_with_id_and_group(id, title, owner, false)
    }

    #[doc(hidden)]
    pub fn create_group_with_id(
        &self,
        id: &str,
        title: &str,
        owner: Principal,
    ) -> StoreResult<Conversation> {
        self.create_conversation_with_id_and_group(id, title, owner, true)
    }

    fn create_conversation_with_id_and_group(
        &self,
        id: &str,
        title: &str,
        owner: Principal,
        is_group: bool,
    ) -> StoreResult<Conversation> {
        validate_required_text(title, "conversation_title")?;
        validate_identifier(id, "conversation_id")?;
        validate_identifier(&owner.id, "principal_id")?;
        let now = now_ms();
        self.with_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            upsert_principal(&transaction, &owner)?;
            transaction.execute(
                "INSERT INTO conversations(
                   id, title, archived, pinned, is_group, revision, created_at, updated_at
                 ) VALUES (?1, ?2, 0, 0, ?3, 0, ?4, ?4)",
                params![id, title, is_group as i64, now],
            )?;
            let owner_membership_id = new_id("membership");
            transaction.execute(
                "INSERT INTO memberships(id, conversation_id, principal_id, access, status, joined_at)
                 VALUES (?1, ?2, ?3, 'owner', 'active', ?4)",
                params![owner_membership_id, id, owner.id, now],
            )?;
            if owner.kind == PrincipalKind::Agent {
                ensure_membership_profile_default(&transaction, &owner_membership_id, now)?;
            }
            transaction.commit()?;
            self.get(id)
        })
    }

    pub fn rename_conversation(&self, id: &str, title: &str) -> StoreResult<()> {
        validate_identifier(id, "conversation_id")?;
        validate_required_text(title, "conversation_title")?;
        self.with_connection(|connection| {
            let changed = connection.execute(
                "UPDATE conversations SET title=?2, revision=revision+1, updated_at=?3 WHERE id=?1",
                params![id, title, now_ms()],
            )?;
            if changed == 0 {
                return Err(anyhow!("conversation_not_found"));
            }
            Ok(())
        })
    }

    pub fn archive_conversation(&self, id: &str, archived: bool) -> StoreResult<()> {
        self.archive_conversation_tree(id, archived, false)?;
        Ok(())
    }

    /// Archive one Conversation together with everything it actually involves:
    /// the whole continuity-child subtree is archived recursively, and every
    /// member native session the conversations used is marked archived so
    /// browse catalogs stop surfacing it. The agents' own on-disk history is
    /// never deleted. With `reopen` (groups only) a fresh successor
    /// Conversation is created in the same transaction: memberships and the
    /// assistant profile are copied silently (no member joined/left churn), and
    /// one plain `conversation-reset` notice opens the successor.
    /// The reserved default local group can never stay archived — startup
    /// normalization always restores it — so an archive request resets it in
    /// place instead: children and member native sessions are archived the
    /// same way, the visible Event history is cleared (refusing in-flight work
    /// like `conversation.clear`), the Assistant membership rotates, and one
    /// `conversation-reset` notice reopens the same active, pinned
    /// Conversation. No successor is created, so no duplicate can survive.
    pub fn archive_conversation_tree(
        &self,
        id: &str,
        archived: bool,
        reopen: bool,
    ) -> StoreResult<ConversationArchiveReport> {
        validate_identifier(id, "conversation_id")?;
        if reopen && !archived {
            return Err(anyhow!("invalid_request"));
        }
        let now = now_ms();
        self.with_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            ensure_conversation(&transaction, id)?;
            if archived && id == DEFAULT_LOCAL_AGENT_GROUP_ID {
                let (archived_child_ids, archived_native_sessions) =
                    reset_reserved_default_group(&transaction, id, now)?;
                transaction.commit()?;
                return Ok(ConversationArchiveReport {
                    conversation_id: id.to_owned(),
                    archived_child_ids,
                    archived_native_sessions,
                    successor_conversation_id: None,
                    reset_in_place: true,
                });
            }
            transaction.execute(
                "UPDATE conversations SET archived=?2, revision=revision+1, updated_at=?3
                 WHERE id=?1 AND archived<>?2",
                params![id, archived as i64, now],
            )?;
            let mut archived_child_ids = Vec::new();
            let mut archived_native_sessions = Vec::new();
            let mut successor_conversation_id = None;
            if archived {
                archived_child_ids = archive_continuity_children(&transaction, id, now)?;
                let mut conversation_ids = vec![id.to_owned()];
                conversation_ids.extend(archived_child_ids.iter().cloned());
                archived_native_sessions =
                    mark_native_sessions_archived(&transaction, &conversation_ids, now)?;
                if reopen {
                    successor_conversation_id =
                        Some(reopen_group_conversation(&transaction, id, now)?);
                }
            } else {
                clear_native_sessions_archived(&transaction, id)?;
            }
            transaction.commit()?;
            Ok(ConversationArchiveReport {
                conversation_id: id.to_owned(),
                archived_child_ids,
                archived_native_sessions,
                successor_conversation_id,
                reset_in_place: false,
            })
        })
    }

    /// Empty one Conversation's visible Event history without touching Agent
    /// native sessions. Continuity children stay intact and are archived.
    /// An in-flight turn or unfinalized Event refuses the write.
    pub fn clear_conversation_history(
        &self,
        conversation_id: &str,
        owner_membership_id: &str,
    ) -> StoreResult<ConversationClearReport> {
        validate_identifier(conversation_id, "conversation_id")?;
        validate_identifier(owner_membership_id, "membership_id")?;
        let now = now_ms();
        self.with_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            ensure_conversation(&transaction, conversation_id)?;
            ensure_local_owner(&transaction, conversation_id, owner_membership_id)?;
            let is_group: bool = transaction.query_row(
                "SELECT is_group FROM conversations WHERE id=?1",
                params![conversation_id],
                |row| row.get::<_, i64>(0).map(|value| value != 0),
            )?;
            if !is_group {
                return Err(anyhow!("invalid_request"));
            }
            refuse_in_flight_group_work(&transaction, conversation_id)?;
            let archived_child_ids =
                archive_continuity_children(&transaction, conversation_id, now)?;
            delete_conversation_events(&transaction, conversation_id)?;
            let assistant_membership_id =
                rotate_assistant_membership(&transaction, conversation_id, now)?;
            bump_revision(&transaction, conversation_id, now)?;
            transaction.commit()?;
            Ok(ConversationClearReport {
                conversation_id: conversation_id.to_owned(),
                archived_child_ids,
                assistant_membership_id,
            })
        })
    }

    #[doc(hidden)]
    pub fn register_continuity_child_link(
        &self,
        parent_id: &str,
        child_id: &str,
        goal_id: &str,
    ) -> StoreResult<()> {
        validate_identifier(parent_id, "conversation_id")?;
        validate_identifier(child_id, "conversation_id")?;
        validate_identifier(goal_id, "goal_id")?;
        self.with_connection(|connection| {
            if !table_exists(connection, "continuity_task_relations")? {
                return Err(anyhow!("unsupported_action"));
            }
            connection.execute(
                "INSERT INTO continuity_task_relations(
                   goal_id, parent_conversation_id, child_conversation_id, card_event_id,
                   card_sequence, card_part_id, listing_kind, follow_through_kind,
                   created_event, completion_transition, revision
                 ) VALUES (?1, ?2, ?3, ?4, 0, NULL, 'child-task', 'durable', '{}', NULL, 1)",
                params![goal_id, parent_id, child_id, child_id],
            )?;
            Ok(())
        })
    }

    pub fn set_conversation_pinned(&self, id: &str, pinned: bool) -> StoreResult<()> {
        validate_identifier(id, "conversation_id")?;
        self.with_connection(|connection| {
            let changed = connection.execute(
                "UPDATE conversations
                 SET revision=revision + CASE WHEN pinned=?2 THEN 0 ELSE 1 END,
                     pinned=?2
                 WHERE id=?1",
                params![id, pinned as i64],
            )?;
            if changed == 0 {
                return Err(anyhow!("conversation_not_found"));
            }
            Ok(())
        })
    }

    pub fn set_conversation_strategy_revision(
        &self,
        id: &str,
        strategy_revision: Option<&str>,
    ) -> StoreResult<()> {
        validate_identifier(id, "conversation_id")?;
        let strategy_revision = strategy_revision
            .map(str::trim)
            .filter(|revision| !revision.is_empty());
        if let Some(revision) = strategy_revision {
            validate_identifier(revision, "strategy_revision")?;
        }
        self.with_connection(|connection| {
            let changed = connection.execute(
                "UPDATE conversations
                 SET revision=revision + CASE WHEN strategy_revision IS ?2 THEN 0 ELSE 1 END,
                     updated_at=CASE WHEN strategy_revision IS ?2 THEN updated_at ELSE ?3 END,
                     strategy_revision=?2
                 WHERE id=?1 AND is_group=1",
                params![id, strategy_revision, now_ms()],
            )?;
            if changed == 0 {
                let exists: bool = connection.query_row(
                    "SELECT EXISTS(SELECT 1 FROM conversations WHERE id=?1)",
                    params![id],
                    |row| row.get(0),
                )?;
                return Err(anyhow!(if exists {
                    "invalid_request"
                } else {
                    "conversation_not_found"
                }));
            }
            Ok(())
        })
    }

    /// Designate (or clear) the one visible long-lived Assistant Membership
    /// of a Conversation. The target must be an active Agent Membership of
    /// the same Conversation. Existing ambiguous groups stay undesignated
    /// until an explicit call; clearing is idempotent.
    pub fn set_conversation_assistant(
        &self,
        conversation_id: &str,
        owner_membership_id: &str,
        expected_revision: i64,
        membership_id: Option<&str>,
    ) -> StoreResult<()> {
        validate_identifier(conversation_id, "conversation_id")?;
        validate_identifier(owner_membership_id, "membership_id")?;
        let membership_id = membership_id
            .map(str::trim)
            .filter(|value| !value.is_empty());
        if let Some(membership_id) = membership_id {
            validate_identifier(membership_id, "membership_id")?;
        }
        self.with_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            ensure_conversation(&transaction, conversation_id)?;
            ensure_local_owner(&transaction, conversation_id, owner_membership_id)?;
            let (current_assistant, current_revision): (Option<String>, i64) = transaction
                .query_row(
                    "SELECT assistant_membership_id, revision FROM conversations WHERE id=?1",
                    params![conversation_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )?;
            if current_assistant.as_deref() == membership_id {
                transaction.commit()?;
                return Ok(());
            }
            if current_revision != expected_revision {
                return Err(anyhow!("conversation_revision_stale"));
            }
            if let Some(membership_id) = membership_id {
                let eligible: bool = transaction.query_row(
                    "SELECT EXISTS(
                       SELECT 1 FROM memberships m
                       JOIN principals p ON p.id=m.principal_id
                       WHERE m.id=?1 AND m.conversation_id=?2
                         AND m.status='active' AND p.kind='agent'
                     )",
                    params![membership_id, conversation_id],
                    |row| row.get(0),
                )?;
                if !eligible {
                    return Err(anyhow!("membership_not_found"));
                }
                ensure_membership_profile_default(&transaction, membership_id, now_ms())?;
            }
            let now = now_ms();
            if let Some(previous) = current_assistant.as_deref() {
                set_profile_responsibility(
                    &transaction,
                    previous,
                    ProfileResponsibility::Member,
                    now,
                )?;
            }
            if let Some(next) = membership_id {
                set_profile_responsibility(
                    &transaction,
                    next,
                    ProfileResponsibility::Assistant,
                    now,
                )?;
            }
            let changed = transaction.execute(
                "UPDATE conversations
                 SET assistant_membership_id=?2,
                     revision=revision + 1,
                     updated_at=?3
                 WHERE id=?1 AND revision=?4",
                params![conversation_id, membership_id, now, expected_revision],
            )?;
            if changed == 0 {
                return Err(anyhow!("conversation_not_found"));
            }
            transaction.commit()?;
            Ok(())
        })
    }

    /// Read one Membership's persistent Profile intent. Returns `None` only
    /// when the Membership does not exist or has no profile row.
    pub fn membership_profile(&self, membership_id: &str) -> StoreResult<Option<ProfileIntent>> {
        validate_identifier(membership_id, "membership_id")?;
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT revision, responsibility, required_capabilities,
                     preferred_capabilities, skill_references, preferred_model,
                     preferred_reasoning_effort, preferred_environment, updated_at,
                     preferred_permission_mode, preferred_instructions,
                     preferred_context_window
                     FROM membership_profiles
                     WHERE membership_id=?1",
                    params![membership_id],
                    profile_intent_from_row,
                )
                .optional()
                .map_err(Into::into)
        })
    }

    /// Persist bounded Profile intent for one active Agent Membership.
    /// Revisions must be monotonic: equal revisions are idempotent, older
    /// revisions are rejected with a stable code.
    pub fn set_membership_profile(
        &self,
        conversation_id: &str,
        membership_id: &str,
        owner_membership_id: &str,
        expected_revision: i64,
        update: &ProfileIntentUpdate,
    ) -> StoreResult<ProfileIntent> {
        validate_identifier(conversation_id, "conversation_id")?;
        validate_identifier(membership_id, "membership_id")?;
        validate_identifier(owner_membership_id, "membership_id")?;
        update.validate().map_err(anyhow::Error::msg)?;
        self.with_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            ensure_conversation(&transaction, conversation_id)?;
            ensure_local_owner(&transaction, conversation_id, owner_membership_id)?;
            let active: bool = transaction.query_row(
                "SELECT EXISTS(
                   SELECT 1 FROM memberships m
                   JOIN principals p ON p.id=m.principal_id
                   WHERE m.id=?1 AND m.conversation_id=?2
                     AND m.status='active' AND p.kind='agent'
                 )",
                params![membership_id, conversation_id],
                |row| row.get(0),
            )?;
            if !active {
                return Err(anyhow!("membership_not_found"));
            }
            let current: Option<ProfileIntent> = transaction
                .query_row(
                    "SELECT revision, responsibility, required_capabilities,
                     preferred_capabilities, skill_references, preferred_model,
                     preferred_reasoning_effort, preferred_environment, updated_at,
                     preferred_permission_mode, preferred_instructions,
                     preferred_context_window
                     FROM membership_profiles WHERE membership_id=?1",
                    params![membership_id],
                    profile_intent_from_row,
                )
                .optional()?;
            let current = current.ok_or_else(|| anyhow!("membership_not_found"))?;
            let same = current.required_capabilities == update.required_capabilities
                && current.preferred_capabilities == update.preferred_capabilities
                && current.preferred_model == update.preferred_model
                && current.preferred_reasoning_effort == update.preferred_reasoning_effort
                && current.preferred_environment == update.preferred_environment
                && current.preferred_permission_mode == update.preferred_permission_mode
                && current.preferred_instructions == update.preferred_instructions
                && current.preferred_context_window == update.preferred_context_window
                && normalized_skill_references(&update.skill_references, current.responsibility)
                    == current.skill_references;
            if same {
                transaction.commit()?;
                return Ok(current);
            }
            if current.revision != expected_revision {
                return Err(anyhow!("profile_revision_stale"));
            }
            let required_capabilities = serde_json::to_string(&update.required_capabilities)?;
            let preferred_capabilities = serde_json::to_string(&update.preferred_capabilities)?;
            let skill_references =
                normalized_skill_references(&update.skill_references, current.responsibility);
            let encoded_skill_references = serde_json::to_string(&skill_references)?;
            let now = now_ms();
            transaction.execute(
                "UPDATE membership_profiles
                 SET revision=revision+1,
                     required_capabilities=?2,
                     preferred_capabilities=?3,
                     skill_references=?4,
                     preferred_model=?5,
                     preferred_reasoning_effort=?6,
                     preferred_environment=?7,
                     preferred_permission_mode=?8,
                     preferred_instructions=?9,
                     preferred_context_window=?10,
                     updated_at=?11
                 WHERE membership_id=?1 AND revision=?12",
                params![
                    membership_id,
                    required_capabilities,
                    preferred_capabilities,
                    encoded_skill_references,
                    update.preferred_model,
                    update.preferred_reasoning_effort,
                    update.preferred_environment,
                    update.preferred_permission_mode,
                    update.preferred_instructions,
                    update.preferred_context_window.map(|value| value as i64),
                    now,
                    expected_revision,
                ],
            )?;
            transaction.commit()?;
            Ok(ProfileIntent {
                revision: expected_revision + 1,
                responsibility: current.responsibility,
                required_capabilities: update.required_capabilities.clone(),
                preferred_capabilities: update.preferred_capabilities.clone(),
                skill_references,
                preferred_model: update.preferred_model.clone(),
                preferred_reasoning_effort: update.preferred_reasoning_effort.clone(),
                preferred_environment: update.preferred_environment.clone(),
                preferred_permission_mode: update.preferred_permission_mode.clone(),
                preferred_instructions: update.preferred_instructions.clone(),
                preferred_context_window: update.preferred_context_window,
                updated_at_unix_ms: now,
            })
        })
    }

    /// All active Agent Memberships of one Conversation paired with their
    /// persistent Profile intents (a bounded default when no row exists).
    pub fn membership_profiles(
        &self,
        conversation_id: &str,
    ) -> StoreResult<Vec<(Membership, ProfileIntent)>> {
        validate_identifier(conversation_id, "conversation_id")?;
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT m.id, m.conversation_id, p.id, p.kind, p.display_name,
                 p.agent_id, p.created_at, m.access, m.status, m.joined_at, m.left_at,
                 COALESCE(fp.revision, 0),
                 COALESCE(fp.responsibility, 'member'),
                 COALESCE(fp.required_capabilities, '[]'),
                 COALESCE(fp.preferred_capabilities, '[]'),
                 COALESCE(fp.skill_references, '[]'),
                 fp.preferred_model, fp.preferred_reasoning_effort,
                 fp.preferred_environment,
                 COALESCE(fp.updated_at, m.joined_at),
                 fp.preferred_permission_mode, fp.preferred_instructions,
                 fp.preferred_context_window
                 FROM memberships m
                 JOIN principals p ON p.id=m.principal_id
                 LEFT JOIN membership_profiles fp ON fp.membership_id=m.id
                 WHERE m.conversation_id=?1 AND m.status='active' AND p.kind='agent'
                 ORDER BY m.joined_at ASC, m.id ASC",
            )?;
            let rows = statement.query_map(params![conversation_id], |row| {
                let membership = membership_from_row(row)?;
                let intent = ProfileIntent {
                    revision: row.get(11)?,
                    responsibility: parse_profile_responsibility(&row.get::<_, String>(12)?)?,
                    required_capabilities: serde_json::from_str(&row.get::<_, String>(13)?)
                        .unwrap_or_default(),
                    preferred_capabilities: serde_json::from_str(&row.get::<_, String>(14)?)
                        .unwrap_or_default(),
                    skill_references: serde_json::from_str(&row.get::<_, String>(15)?)
                        .unwrap_or_default(),
                    preferred_model: row.get(16)?,
                    preferred_reasoning_effort: row.get(17)?,
                    preferred_environment: row.get(18)?,
                    preferred_permission_mode: row.get(20)?,
                    preferred_instructions: row.get(21)?,
                    preferred_context_window: row
                        .get::<_, Option<i64>>(22)?
                        .and_then(|value| u64::try_from(value).ok()),
                    updated_at_unix_ms: row.get(19)?,
                };
                Ok((membership, intent))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(Into::into)
        })
    }

    pub fn add_member(
        &self,
        conversation_id: &str,
        principal: Principal,
        access: MembershipAccess,
    ) -> StoreResult<Membership> {
        validate_identifier(conversation_id, "conversation_id")?;
        validate_identifier(&principal.id, "principal_id")?;
        validate_required_text(&principal.display_name, "principal_display_name")?;
        let now = now_ms();
        self.with_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            ensure_conversation(&transaction, conversation_id)?;
            upsert_principal(&transaction, &principal)?;
            let existing_active: Option<String> = transaction
                .query_row(
                    "SELECT id FROM memberships WHERE conversation_id=?1 AND principal_id=?2 AND status='active'",
                    params![conversation_id, principal.id],
                    |row| row.get(0),
                )
                .optional()?;
            if existing_active.is_some() {
                return Err(anyhow!("membership_already_active"));
            }
            let existing_left: Option<String> = transaction
                .query_row(
                    "SELECT id FROM memberships
                     WHERE conversation_id=?1 AND principal_id=?2 AND status='left'
                     ORDER BY joined_at DESC, id DESC LIMIT 1",
                    params![conversation_id, principal.id],
                    |row| row.get(0),
                )
                .optional()?;
            let membership_id = if let Some(membership_id) = existing_left {
                let active = lifecycle::reactivate_membership();
                transaction.execute(
                    "UPDATE memberships
                     SET status=?2, left_at=NULL, access=?3
                     WHERE id=?1 AND status=?4",
                    params![
                        membership_id,
                        active.as_str(),
                        enum_wire(access)?,
                        super::MembershipStatus::Left.as_str()
                    ],
                )?;
                membership_id
            } else {
                let membership_id = new_id("membership");
                transaction.execute(
                    "INSERT INTO memberships(id, conversation_id, principal_id, access, status, joined_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        membership_id,
                        conversation_id,
                        principal.id,
                        enum_wire(access)?,
                        lifecycle::initial_membership().as_str(),
                        now
                    ],
                )?;
                membership_id
            };
            if principal.kind == PrincipalKind::Agent {
                ensure_membership_profile_default(&transaction, &membership_id, now)?;
            }
            append_domain_event(
                &transaction,
                conversation_id,
                EventKind::MembershipChanged,
                serde_json::json!({
                    "membershipId": membership_id,
                    "principalId": principal.id,
                    "change": "joined",
                }),
                now,
            )?;
            bump_revision(&transaction, conversation_id, now)?;
            transaction.commit()?;
            self.membership(conversation_id, &membership_id)
        })
    }

    pub fn leave_member(&self, conversation_id: &str, membership_id: &str) -> StoreResult<()> {
        self.with_connection(|connection| {
            let transaction = connection.unchecked_transaction()?;
            let principal_kind: Option<String> = transaction
                .query_row(
                    "SELECT p.kind FROM memberships m JOIN principals p ON p.id=m.principal_id
                     WHERE m.id=?1 AND m.conversation_id=?2 AND m.status='active'",
                    params![membership_id, conversation_id],
                    |row| row.get(0),
                )
                .optional()?;
            if principal_kind.is_none() {
                return Err(anyhow!("membership_not_found"));
            }
            let owners: i64 = transaction.query_row(
                "SELECT COUNT(*) FROM memberships WHERE conversation_id=?1 AND access='owner' AND status='active'",
                params![conversation_id], |row| row.get(0))?;
            let access: String = transaction.query_row(
                "SELECT access FROM memberships WHERE id=?1", params![membership_id], |row| row.get(0))?;
            if access == "owner" && owners <= 1 {
                return Err(anyhow!("conversation_requires_owner"));
            }
            let now = now_ms();
            let left = lifecycle::leave_membership();
            let was_assistant: bool = transaction.query_row(
                "SELECT assistant_membership_id IS ?2 FROM conversations WHERE id=?1",
                params![conversation_id, membership_id],
                |row| row.get(0),
            )?;
            if was_assistant {
                set_profile_responsibility(
                    &transaction,
                    membership_id,
                    ProfileResponsibility::Member,
                    now,
                )?;
                transaction.execute(
                    "UPDATE conversations SET assistant_membership_id=NULL WHERE id=?1",
                    params![conversation_id],
                )?;
            }
            transaction.execute(
                "UPDATE memberships SET status=?3, left_at=?4
                 WHERE id=?1 AND conversation_id=?2 AND status=?5",
                params![
                    membership_id,
                    conversation_id,
                    left.as_str(),
                    now,
                    super::MembershipStatus::Active.as_str()
                ],
            )?;
            append_domain_event(
                &transaction,
                conversation_id,
                EventKind::MembershipChanged,
                serde_json::json!({
                    "membershipId": membership_id,
                    "change": "left",
                }),
                now,
            )?;
            bump_revision(&transaction, conversation_id, now)?;
            transaction.commit()?;
            Ok(())
        })
    }

    pub fn set_member_access(
        &self,
        conversation_id: &str,
        membership_id: &str,
        access: MembershipAccess,
    ) -> StoreResult<Membership> {
        validate_identifier(conversation_id, "conversation_id")?;
        validate_identifier(membership_id, "membership_id")?;
        self.with_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let current: Option<String> = transaction
                .query_row(
                    "SELECT access FROM memberships
                     WHERE id=?1 AND conversation_id=?2 AND status='active'",
                    params![membership_id, conversation_id],
                    |row| row.get(0),
                )
                .optional()?;
            let Some(current) = current else {
                return Err(anyhow!("membership_not_found"));
            };
            let next = enum_wire(access)?;
            if current == next {
                transaction.commit()?;
                return self.membership(conversation_id, membership_id);
            }
            if current == "owner" && next == "member" {
                let owners: i64 = transaction.query_row(
                    "SELECT COUNT(*) FROM memberships
                     WHERE conversation_id=?1 AND access='owner' AND status='active'",
                    params![conversation_id],
                    |row| row.get(0),
                )?;
                if owners <= 1 {
                    return Err(anyhow!("conversation_requires_owner"));
                }
            }
            let now = now_ms();
            transaction.execute(
                "UPDATE memberships SET access=?3 WHERE id=?1 AND conversation_id=?2",
                params![membership_id, conversation_id, next],
            )?;
            append_domain_event(
                &transaction,
                conversation_id,
                EventKind::MembershipChanged,
                serde_json::json!({
                    "membershipId": membership_id,
                    "change": "access-set",
                    "access": next,
                }),
                now,
            )?;
            bump_revision(&transaction, conversation_id, now)?;
            transaction.commit()?;
            self.membership(conversation_id, membership_id)
        })
    }

    pub fn list(&self, include_archived: bool) -> StoreResult<Vec<ConversationSummary>> {
        self.with_connection(|connection| list_inner(connection, include_archived))
    }

    pub fn get(&self, id: &str) -> StoreResult<Conversation> {
        validate_identifier(id, "conversation_id")?;
        self.with_connection(|connection| get_inner(connection, id))
    }

    pub fn page_events(
        &self,
        conversation_id: &str,
        after_sequence: Option<i64>,
        requested_limit: usize,
    ) -> StoreResult<EventPage> {
        self.page_events_window(
            conversation_id,
            EventPagePosition::After(after_sequence.unwrap_or(0)),
            requested_limit,
        )
    }

    pub fn page_events_window(
        &self,
        conversation_id: &str,
        position: EventPagePosition,
        requested_limit: usize,
    ) -> StoreResult<EventPage> {
        validate_identifier(conversation_id, "conversation_id")?;
        let limit = requested_limit.clamp(1, MAX_EVENT_PAGE_SIZE);
        self.with_connection(|connection| {
            page_events_window_inner(connection, conversation_id, position, limit)
        })
    }

    pub fn search(&self, query: &str, limit: usize) -> StoreResult<Vec<ConversationEvent>> {
        validate_text(query, "search_query")?;
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }
        let limit = limit.clamp(1, MAX_EVENT_PAGE_SIZE);
        let fts_query = format!("\"{}\"", query.trim().replace('"', "\"\""));
        self.with_connection(|connection| search_inner(connection, &fts_query, limit))
    }

    pub fn append_event(
        &self,
        conversation_id: &str,
        author_membership_id: Option<&str>,
        kind: EventKind,
        parts: &[NewEventPart],
        causation_id: Option<&str>,
        correlation_id: Option<&str>,
        finalized: bool,
    ) -> StoreResult<ConversationEvent> {
        validate_identifier(conversation_id, "conversation_id")?;
        if let Some(author) = author_membership_id {
            validate_identifier(author, "membership_id")?;
        }
        if parts.len() > 4096 {
            return Err(anyhow!("event_parts_limit_exceeded"));
        }
        let event_id = new_id("event");
        let now = now_ms();
        self.with_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let event = insert_event(
                &transaction,
                &event_id,
                conversation_id,
                author_membership_id,
                kind,
                parts,
                causation_id,
                correlation_id,
                finalized,
                true,
                now,
            )?;
            transaction.commit()?;
            Ok(event)
        })
    }

    /// Persist one human Message and its exact structured mention work in one
    /// writer transaction. A committed Message can therefore never point at a
    /// partially created Direct Turn set.
    pub fn post_message_with_mentions(
        &self,
        conversation_id: &str,
        author_membership_id: Option<&str>,
        content: &str,
        correlation_id: Option<&str>,
        membership_ids: &[String],
    ) -> StoreResult<(ConversationEvent, Vec<DirectTurn>)> {
        self.post_message_with_attachments(
            conversation_id,
            author_membership_id,
            content,
            correlation_id,
            membership_ids,
            &[],
        )
    }

    /// Persist one human Message carrying admitted image attachments. Each
    /// attachment becomes one image Event Part after the text part, inside the
    /// same writer transaction as the Event and its Direct Turn set. An
    /// image-only post omits the text part; without attachments the text stays
    /// required.
    pub fn post_message_with_attachments(
        &self,
        conversation_id: &str,
        author_membership_id: Option<&str>,
        content: &str,
        correlation_id: Option<&str>,
        membership_ids: &[String],
        attachments: &[ImageAttachment],
    ) -> StoreResult<(ConversationEvent, Vec<DirectTurn>)> {
        validate_identifier(conversation_id, "conversation_id")?;
        if attachments.is_empty() {
            validate_required_text(content, "message_content")?;
        } else {
            validate_text(content, "message_content")?;
        }
        if let Some(author) = author_membership_id {
            validate_identifier(author, "membership_id")?;
        }
        if membership_ids.len() > 100 {
            return Err(anyhow!("mention_limit_exceeded"));
        }
        let event_id = new_id("event");
        let now = now_ms();
        let mut parts = Vec::with_capacity(attachments.len() + 1);
        if !content.trim().is_empty() {
            parts.push(NewEventPart {
                id: String::new(),
                kind: EventPartKind::Text,
                content: content.to_owned(),
            });
        }
        parts.extend(attachments.iter().map(NewEventPart::image));
        self.with_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let event = insert_event(
                &transaction,
                &event_id,
                conversation_id,
                author_membership_id,
                EventKind::Message,
                &parts,
                None,
                correlation_id,
                true,
                true,
                now,
            )?;
            let turns = enqueue_mention_turns_in_transaction(
                &transaction,
                conversation_id,
                &event.id,
                membership_ids,
            )?;
            transaction.commit()?;
            Ok((event, turns))
        })
    }

    /// The stored text of one posted message Event, validated to belong to
    /// the Conversation. Dispatch addressing reads this instead of trusting
    /// caller-supplied content. An image-only post carries no text part and
    /// reads back as empty text so its attachments still dispatch.
    pub fn posted_event_text(&self, conversation_id: &str, event_id: &str) -> StoreResult<String> {
        validate_identifier(conversation_id, "conversation_id")?;
        validate_identifier(event_id, "event_id")?;
        self.with_connection(|connection| {
            let row: Option<(String, Option<String>)> = connection
                .query_row(
                    "SELECT e.conversation_id,
                            (SELECT ep.content FROM event_parts ep
                              WHERE ep.event_id=e.id AND ep.kind='text'
                                AND ep.runtime_cursor IS NULL
                              ORDER BY ep.ordinal ASC LIMIT 1)
                     FROM events e WHERE e.id=?1",
                    params![event_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            let (owner, text) = row.ok_or_else(|| anyhow!("conversation_event_not_found"))?;
            if owner != conversation_id {
                return Err(anyhow!("mention_event_mismatch"));
            }
            if let Some(text) = text.filter(|content| !content.trim().is_empty()) {
                return Ok(text);
            }
            let has_image = connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM event_parts
                  WHERE event_id=?1 AND kind='image' AND runtime_cursor IS NULL)",
                params![event_id],
                |row| row.get::<_, bool>(0),
            )?;
            if has_image {
                Ok(String::new())
            } else {
                Err(anyhow!("conversation_event_not_found"))
            }
        })
    }

    /// The stored text of one posted Event Part, validated to belong to the
    /// Conversation and that Event. Image parts read back as empty text so a
    /// Part-scoped SourceRef cannot widen to another Part's content.
    pub fn posted_event_part_text(
        &self,
        conversation_id: &str,
        event_id: &str,
        part_id: &str,
    ) -> StoreResult<String> {
        validate_identifier(conversation_id, "conversation_id")?;
        validate_identifier(event_id, "event_id")?;
        validate_identifier(part_id, "part_id")?;
        self.with_connection(|connection| {
            let row: Option<(String, String, String)> = connection
                .query_row(
                    "SELECT e.conversation_id, ep.kind, ep.content
                     FROM events e
                     JOIN event_parts ep ON ep.event_id=e.id
                     WHERE e.id=?1 AND ep.id=?2 AND ep.runtime_cursor IS NULL",
                    params![event_id, part_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()?;
            let (owner, kind, content) =
                row.ok_or_else(|| anyhow!("conversation_event_not_found"))?;
            if owner != conversation_id {
                return Err(anyhow!("mention_event_mismatch"));
            }
            match kind.as_str() {
                "text" | "reasoning" => Ok(content),
                "image" => Ok(String::new()),
                _ => Err(anyhow!("conversation_event_not_found")),
            }
        })
    }

    /// Load one Event by identity after proving it belongs to the Conversation.
    pub fn event(
        &self,
        conversation_id: &str,
        event_id: &str,
    ) -> StoreResult<Option<ConversationEvent>> {
        validate_identifier(conversation_id, "conversation_id")?;
        validate_identifier(event_id, "event_id")?;
        self.with_connection(|connection| {
            let owner: Option<String> = connection
                .query_row(
                    "SELECT conversation_id FROM events WHERE id=?1",
                    params![event_id],
                    |row| row.get(0),
                )
                .optional()?;
            let Some(owner) = owner else {
                return Ok(None);
            };
            if owner != conversation_id {
                return Err(anyhow!("mention_event_mismatch"));
            }
            Ok(Some(event_by_id(connection, event_id)?))
        })
    }

    /// Exact image attachment refs for one Event owned by the Conversation.
    pub fn image_attachments_for_event(
        &self,
        conversation_id: &str,
        event_id: &str,
    ) -> StoreResult<Vec<ImageAttachmentReference>> {
        if self.event(conversation_id, event_id)?.is_none() {
            return Ok(Vec::new());
        }
        self.with_connection(|connection| image_attachments_for_event(connection, event_id))
    }

    /// Exact image attachment for one Part on an Event owned by the Conversation.
    pub fn image_attachment_for_part(
        &self,
        conversation_id: &str,
        event_id: &str,
        part_id: &str,
    ) -> StoreResult<Option<ImageAttachmentReference>> {
        if self.event(conversation_id, event_id)?.is_none() {
            return Ok(None);
        }
        self.with_connection(|connection| image_attachment_for_part(connection, event_id, part_id))
    }

    pub fn event_sequence(
        &self,
        conversation_id: &str,
        event_id: &str,
    ) -> StoreResult<Option<i64>> {
        validate_identifier(conversation_id, "conversation_id")?;
        validate_identifier(event_id, "event_id")?;
        self.with_connection(|connection| {
            let row: Option<(String, i64)> = connection
                .query_row(
                    "SELECT conversation_id, sequence FROM events WHERE id=?1",
                    params![event_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            match row {
                Some((owner, sequence)) if owner == conversation_id => Ok(Some(sequence)),
                Some(_) => Err(anyhow!("mention_event_mismatch")),
                None => Ok(None),
            }
        })
    }

    /// Delete one local owner's posted message and its complete derived turn
    /// subtree. Active work is never interrupted by deletion; the caller can
    /// retry or delete only after every addressed turn has settled.
    pub fn delete_posted_message(
        &self,
        conversation_id: &str,
        event_id: &str,
        owner_membership_id: &str,
    ) -> StoreResult<()> {
        validate_identifier(conversation_id, "conversation_id")?;
        validate_identifier(event_id, "event_id")?;
        validate_identifier(owner_membership_id, "membership_id")?;
        let now = now_ms();
        self.with_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let eligible: bool = transaction.query_row(
                "SELECT EXISTS(
                   SELECT 1 FROM events e
                   JOIN memberships m ON m.id=e.author_membership_id
                   JOIN principals p ON p.id=m.principal_id
                   WHERE e.id=?1 AND e.conversation_id=?2
                     AND e.author_membership_id=?3 AND e.kind='message'
                     AND m.conversation_id=?2 AND m.access='owner'
                     AND m.status='active' AND p.kind='human'
                 )",
                params![event_id, conversation_id, owner_membership_id],
                |row| row.get(0),
            )?;
            if !eligible {
                return Err(anyhow!("message_delete_forbidden"));
            }
            let active: bool = transaction.query_row(
                "WITH RECURSIVE removed(id) AS (
                   SELECT ?1
                   UNION
                   SELECT e.id FROM events e JOIN removed r ON e.causation_id=r.id
                   WHERE e.conversation_id=?2
                 )
                 SELECT EXISTS(
                   SELECT 1 FROM direct_turns
                   WHERE source_event_id IN (SELECT id FROM removed)
                     AND state IN ('pending','claimed','running','waiting-for-human')
                 )",
                params![event_id, conversation_id],
                |row| row.get(0),
            )?;
            if active {
                return Err(anyhow!("message_turn_active"));
            }
            transaction.execute(
                "WITH RECURSIVE removed(id) AS (
                   SELECT ?1
                   UNION
                   SELECT e.id FROM events e JOIN removed r ON e.causation_id=r.id
                   WHERE e.conversation_id=?2
                 )
                 DELETE FROM conversation_dispatches
                 WHERE id IN (
                   SELECT id FROM direct_turns
                   WHERE source_event_id IN (SELECT id FROM removed)
                 )",
                params![event_id, conversation_id],
            )?;
            transaction.execute(
                "WITH RECURSIVE removed(id) AS (
                   SELECT ?1
                   UNION
                   SELECT e.id FROM events e JOIN removed r ON e.causation_id=r.id
                   WHERE e.conversation_id=?2
                 )
                 DELETE FROM event_search WHERE event_id IN (SELECT id FROM removed)",
                params![event_id, conversation_id],
            )?;
            transaction.execute(
                "WITH RECURSIVE removed(id) AS (
                   SELECT ?1
                   UNION
                   SELECT e.id FROM events e JOIN removed r ON e.causation_id=r.id
                   WHERE e.conversation_id=?2
                 )
                 DELETE FROM events WHERE id IN (SELECT id FROM removed)",
                params![event_id, conversation_id],
            )?;
            bump_revision(&transaction, conversation_id, now)?;
            transaction.commit()?;
            Ok(())
        })
    }

    pub fn append_event_part(&self, event_id: &str, part: NewEventPart) -> StoreResult<EventPart> {
        validate_identifier(event_id, "event_id")?;
        validate_text(&part.content, "event_part_content")?;
        self.with_connection(|connection| {
            let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let (conversation_id, finalized): (String, bool) = transaction.query_row(
                "SELECT conversation_id, finalized FROM events WHERE id=?1",
                params![event_id],
                |row| Ok((row.get(0)?, row.get::<_, i64>(1)? != 0)),
            )?;
            if finalized {
                return Err(anyhow!("event_already_finalized"));
            }
            let ordinal: i64 = transaction.query_row(
                "SELECT COALESCE(MAX(ordinal), -1)+1 FROM event_parts WHERE event_id=?1", params![event_id], |row| row.get(0))?;
            let now = now_ms();
            let id = if part.id.trim().is_empty() { new_id("part") } else { part.id };
            transaction.execute(
                "INSERT INTO event_parts(id, event_id, ordinal, kind, content, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![id, event_id, ordinal, enum_wire(part.kind)?, part.content, now],
            )?;
            if matches!(part.kind, super::EventPartKind::Text | super::EventPartKind::Reasoning) {
                transaction.execute(
                    "INSERT INTO event_search(event_id, conversation_id, content) VALUES (?1, ?2, ?3)",
                    params![event_id, conversation_id, part.content],
                )?;
            }
            bump_revision(&transaction, &conversation_id, now)?;
            transaction.commit()?;
            Ok(EventPart { id, event_id: event_id.to_owned(), ordinal, kind: part.kind, content: part.content, created_at_unix_ms: now })
        })
    }

    pub fn enqueue_mention_turns(
        &self,
        conversation_id: &str,
        source_event_id: &str,
        membership_ids: &[String],
    ) -> StoreResult<Vec<DirectTurn>> {
        if membership_ids.len() > 100 {
            return Err(anyhow!("mention_limit_exceeded"));
        }
        self.with_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let turns = enqueue_mention_turns_in_transaction(
                &transaction,
                conversation_id,
                source_event_id,
                membership_ids,
            )?;
            transaction.commit()?;
            Ok(turns)
        })
    }

    pub fn direct_turn_for_source(
        &self,
        conversation_id: &str,
        source_event_id: &str,
        membership_id: &str,
    ) -> StoreResult<Option<DirectTurn>> {
        validate_identifier(conversation_id, "conversation_id")?;
        validate_identifier(source_event_id, "event_id")?;
        validate_identifier(membership_id, "membership_id")?;
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT id, conversation_id, source_event_id, membership_id, state, ordinal
                     FROM direct_turns
                     WHERE conversation_id=?1 AND source_event_id=?2 AND membership_id=?3",
                    params![conversation_id, source_event_id, membership_id],
                    direct_turn_from_row,
                )
                .optional()
                .map_err(Into::into)
        })
    }

    /// Claim exactly one pending Direct Turn and load all private execution
    /// context inside the same transaction. A second claimant receives None.
    #[doc(hidden)]
    pub fn claim_direct_turn(
        &self,
        turn_id: &str,
    ) -> StoreResult<Option<DirectTurnExecutionContext>> {
        validate_identifier(turn_id, "direct_turn_id")?;
        self.with_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let changed = transaction.execute(
                "UPDATE direct_turns SET state='claimed' WHERE id=?1 AND state='pending'",
                params![turn_id],
            )?;
            if changed == 0 {
                transaction.commit()?;
                return Ok(None);
            }
            let context = direct_turn_execution_context(&transaction, turn_id)?;
            transaction.commit()?;
            Ok(Some(context))
        })
    }

    /// Atomically claim the oldest safe-boundary continuation for one
    /// Conversation Membership and reload its current runtime/Profile facts.
    pub fn claim_next_pending_direct_turn(
        &self,
        conversation_id: &str,
        membership_id: &str,
    ) -> StoreResult<Option<DirectTurnExecutionContext>> {
        validate_identifier(conversation_id, "conversation_id")?;
        validate_identifier(membership_id, "membership_id")?;
        self.with_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let turn_id = transaction
                .query_row(
                    "SELECT t.id FROM direct_turns t
                     JOIN events e ON e.id=t.source_event_id
                     WHERE t.conversation_id=?1 AND t.membership_id=?2
                       AND t.state='pending'
                     ORDER BY e.sequence ASC, t.ordinal ASC, t.id ASC LIMIT 1",
                    params![conversation_id, membership_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            let Some(turn_id) = turn_id else {
                transaction.commit()?;
                return Ok(None);
            };
            let changed = transaction.execute(
                "UPDATE direct_turns SET state='running' WHERE id=?1 AND state='pending'",
                params![turn_id],
            )?;
            if changed != 1 {
                transaction.commit()?;
                return Ok(None);
            }
            let context = direct_turn_execution_context(&transaction, &turn_id)?;
            transaction.commit()?;
            Ok(Some(context))
        })
    }

    #[doc(hidden)]
    pub fn mark_direct_turn_running(&self, turn_id: &str) -> StoreResult<bool> {
        validate_identifier(turn_id, "direct_turn_id")?;
        self.with_connection(|connection| {
            Ok(connection.execute(
                "UPDATE direct_turns SET state='running' WHERE id=?1 AND state='claimed'",
                params![turn_id],
            )? == 1)
        })
    }

    /// Settle a mention turn whose dispatch was never opened. When a dispatch
    /// row with the turn's identity exists, the dispatch completion authority
    /// already owns the terminal write and this call is a no-op.
    pub fn fail_direct_turn_unless_dispatched(
        &self,
        turn_id: &str,
        diagnostic: &str,
    ) -> StoreResult<bool> {
        validate_identifier(turn_id, "direct_turn_id")?;
        validate_required_text(diagnostic, "direct_turn_diagnostic")?;
        let opened = self.with_connection(|connection| {
            Ok(connection
                .query_row(
                    "SELECT 1 FROM conversation_dispatches WHERE id=?1",
                    params![turn_id],
                    |row| row.get::<_, i64>(0),
                )
                .optional()?
                .is_some())
        })?;
        if opened {
            return Ok(false);
        }
        self.fail_direct_turn(turn_id, diagnostic)?;
        Ok(true)
    }

    pub(super) fn fail_direct_turn(
        &self,
        turn_id: &str,
        diagnostic: &str,
    ) -> StoreResult<ConversationEvent> {
        validate_identifier(turn_id, "direct_turn_id")?;
        validate_required_text(diagnostic, "direct_turn_diagnostic")?;
        let event_id = new_id("event");
        let now = now_ms();
        self.with_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let (conversation_id, source_event_id, membership_id, current_state): (
                String,
                String,
                String,
                String,
            ) = transaction.query_row(
                "SELECT conversation_id, source_event_id, membership_id, state
                 FROM direct_turns WHERE id=?1",
                params![turn_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )?;
            if current_state == "running" {
                let changed = transaction.execute(
                    "UPDATE direct_turns SET state='failed' WHERE id=?1 AND state='running'",
                    params![turn_id],
                )?;
                if changed != 1 {
                    return Err(anyhow!("direct_turn_not_running"));
                }
            } else if current_state != "failed" {
                return Err(anyhow!("direct_turn_not_running"));
            }
            let runtime_event_id: Option<String> = transaction
                .query_row(
                    "SELECT id FROM events
                     WHERE conversation_id=?1 AND author_membership_id=?2
                       AND causation_id=?3 AND correlation_id=?4
                       AND kind='message' AND finalized=1
                     ORDER BY sequence DESC LIMIT 1",
                    params![conversation_id, membership_id, source_event_id, turn_id],
                    |row| row.get(0),
                )
                .optional()?;
            let event = if let Some(runtime_event_id) = runtime_event_id {
                event_by_id(&transaction, &runtime_event_id)?
            } else {
                insert_event(
                    &transaction,
                    &event_id,
                    &conversation_id,
                    Some(&membership_id),
                    EventKind::Message,
                    &[NewEventPart {
                        id: String::new(),
                        kind: EventPartKind::Diagnostic,
                        content: diagnostic.to_owned(),
                    }],
                    Some(&source_event_id),
                    Some(turn_id),
                    true,
                    false,
                    now,
                )?
            };
            transaction.commit()?;
            Ok(event)
        })
    }

    #[doc(hidden)]
    pub fn direct_turn(&self, turn_id: &str) -> StoreResult<DirectTurn> {
        self.direct_turn_record(turn_id)?
            .ok_or_else(|| anyhow!("direct_turn_not_found"))
    }

    /// The durable record for one direct turn, or `None` when the store holds
    /// none. Callers that must tell "no such turn" apart from a failed read
    /// use this, since the error of [`Self::direct_turn`] cannot.
    pub fn direct_turn_record(&self, turn_id: &str) -> StoreResult<Option<DirectTurn>> {
        validate_identifier(turn_id, "direct_turn_id")?;
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT id, conversation_id, source_event_id, membership_id, state, ordinal
                     FROM direct_turns WHERE id=?1",
                    params![turn_id],
                    direct_turn_from_row,
                )
                .optional()
                .map_err(Into::into)
        })
    }

    pub fn finalize_event(&self, event_id: &str) -> StoreResult<()> {
        self.with_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let conversation_id: String = transaction.query_row(
                "SELECT conversation_id FROM events WHERE id=?1",
                params![event_id],
                |row| row.get(0),
            )?;
            let changed = transaction.execute(
                "UPDATE events SET finalized=1 WHERE id=?1 AND finalized=0",
                params![event_id],
            )?;
            if changed == 0 {
                return Err(anyhow!("event_already_finalized"));
            }
            bump_revision(&transaction, &conversation_id, now_ms())?;
            transaction.commit()?;
            Ok(())
        })
    }

    pub fn source_link(
        &self,
        conversation_id: &str,
        source_kind: &str,
        native_identity: &str,
    ) -> StoreResult<SourceLink> {
        validate_required_text(source_kind, "source_kind")?;
        validate_required_text(native_identity, "native_identity")?;
        let id = new_id("source");
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO source_links(id, conversation_id, source_kind, native_identity)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(source_kind, native_identity) DO NOTHING",
                params![id, conversation_id, source_kind, native_identity],
            )?;
            connection
                .query_row(
                    "SELECT id, conversation_id, source_kind, native_identity FROM source_links
                 WHERE source_kind=?1 AND native_identity=?2",
                    params![source_kind, native_identity],
                    source_link_from_row,
                )
                .map_err(Into::into)
        })
    }

    pub fn runtime_binding(&self, binding: RuntimeBinding) -> StoreResult<()> {
        validate_identifier(&binding.conversation_id, "conversation_id")?;
        validate_identifier(&binding.membership_id, "membership_id")?;
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO runtime_bindings(id, conversation_id, membership_id, lane, availability, safe_reason)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(conversation_id, membership_id, lane) DO UPDATE SET
                 availability=excluded.availability, safe_reason=excluded.safe_reason",
                params![binding.id, binding.conversation_id, binding.membership_id, binding.lane,
                    binding.availability, binding.safe_reason],
            )?;
            Ok(())
        })
    }

    #[doc(hidden)]
    pub fn runtime_binding_with_private_location(
        &self,
        binding: RuntimeBinding,
        runtime_session_id: Option<&str>,
        runtime_conversation_path: Option<&str>,
        working_directory: Option<&str>,
    ) -> StoreResult<()> {
        validate_identifier(&binding.conversation_id, "conversation_id")?;
        validate_identifier(&binding.membership_id, "membership_id")?;
        self.with_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if let Some(session_id) = runtime_session_id {
                native_sessions::record_native_session(
                    &transaction,
                    &binding.conversation_id,
                    &binding.membership_id,
                    session_id,
                )?;
            }
            transaction.execute(
                "INSERT INTO runtime_bindings(
                   id, conversation_id, membership_id, lane, availability, safe_reason,
                   runtime_session_id, runtime_conversation_path, working_directory
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 ON CONFLICT(conversation_id, membership_id, lane) DO UPDATE SET
                   availability=excluded.availability,
                   safe_reason=excluded.safe_reason,
                   runtime_session_id=excluded.runtime_session_id,
                   runtime_conversation_path=excluded.runtime_conversation_path,
                   working_directory=excluded.working_directory",
                params![
                    binding.id,
                    binding.conversation_id,
                    binding.membership_id,
                    binding.lane,
                    binding.availability,
                    binding.safe_reason,
                    runtime_session_id,
                    runtime_conversation_path,
                    working_directory,
                ],
            )?;
            transaction.commit()?;
            Ok(())
        })
    }

    pub fn create_dispatch(
        &self,
        conversation_id: &str,
        membership_id: &str,
        operation: &str,
        session_mode: DispatchSessionMode,
    ) -> StoreResult<ConversationDispatch> {
        validate_identifier(conversation_id, "conversation_id")?;
        validate_identifier(membership_id, "membership_id")?;
        validate_required_text(operation, "dispatch_operation")?;
        let dispatch_id = new_id("dispatch");
        let now = now_ms();
        self.with_connection(|connection| {
            let eligible: Option<i64> = connection
                .query_row(
                    "SELECT 1 FROM memberships m JOIN principals p ON p.id=m.principal_id
                     WHERE m.id=?1 AND m.conversation_id=?2 AND m.status='active' AND p.kind='agent'",
                    params![membership_id, conversation_id],
                    |row| row.get(0),
                )
                .optional()?;
            if eligible.is_none() {
                return Err(anyhow!("dispatch_membership_unavailable"));
            }
            connection.execute(
                "INSERT INTO conversation_dispatches(
                   id, conversation_id, membership_id, operation, state, session_mode,
                   runtime_conversation_path, error_code, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, NULL, ?7, ?7)",
                params![
                    dispatch_id,
                    conversation_id,
                    membership_id,
                    operation,
                    lifecycle::initial_dispatch().as_str(),
                    enum_wire(session_mode)?,
                    now,
                ],
            )?;
            dispatch_by_id(connection, &dispatch_id)
        })
    }

    pub fn update_dispatch(
        &self,
        dispatch_id: &str,
        state: DispatchState,
        runtime_conversation_path: Option<&str>,
        error_code: Option<&str>,
    ) -> StoreResult<()> {
        validate_identifier(dispatch_id, "dispatch_id")?;
        if runtime_conversation_path.is_some_and(|value| value.trim().is_empty()) {
            return Err(anyhow!("dispatch_runtime_location_invalid"));
        }
        self.with_connection(|connection| {
            let current: Option<String> = connection
                .query_row(
                    "SELECT state FROM conversation_dispatches WHERE id=?1",
                    params![dispatch_id],
                    |row| row.get(0),
                )
                .optional()?;
            let Some(current) = current else {
                return Err(anyhow!("dispatch_not_found"));
            };
            let current = DispatchState::from_name(&current)
                .ok_or_else(|| anyhow!("dispatch_state_invalid"))?;
            if !lifecycle::dispatch_permits(current, state) {
                return Err(anyhow!("dispatch_transition_invalid"));
            }
            let changed = connection.execute(
                "UPDATE conversation_dispatches SET state=?2,
                   runtime_conversation_path=COALESCE(?3, runtime_conversation_path),
                   error_code=?4, updated_at=?5 WHERE id=?1 AND state=?6",
                params![
                    dispatch_id,
                    enum_wire(state)?,
                    runtime_conversation_path,
                    error_code,
                    now_ms(),
                    current.as_str(),
                ],
            )?;
            if changed == 0 {
                return Err(anyhow!("dispatch_not_found"));
            }
            Ok(())
        })
    }

    pub fn latest_resumable_dispatch(
        &self,
        conversation_id: &str,
        membership_id: &str,
    ) -> StoreResult<Option<ConversationDispatch>> {
        validate_identifier(conversation_id, "conversation_id")?;
        validate_identifier(membership_id, "membership_id")?;
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT id, conversation_id, membership_id, operation, state, session_mode,
                       runtime_conversation_path, error_code, created_at, updated_at
                     FROM conversation_dispatches
                     WHERE conversation_id=?1 AND membership_id=?2 AND state='completed'
                       AND runtime_conversation_path IS NOT NULL
                     ORDER BY updated_at DESC, id DESC LIMIT 1",
                    params![conversation_id, membership_id],
                    dispatch_from_row,
                )
                .optional()
                .map_err(Into::into)
        })
    }

    /// Read one canonical dispatch record by its durable identity. An unknown
    /// identity returns an absent record rather than an error, so restart
    /// recovery distinguishes a never-opened dispatch from any recorded
    /// state without adding a second bookkeeping row.
    pub fn dispatch_record(&self, dispatch_id: &str) -> StoreResult<Option<ConversationDispatch>> {
        validate_identifier(dispatch_id, "dispatch_id")?;
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT id, conversation_id, membership_id, operation, state, session_mode,
                       runtime_conversation_path, error_code, created_at, updated_at
                     FROM conversation_dispatches WHERE id=?1",
                    params![dispatch_id],
                    dispatch_from_row,
                )
                .optional()
                .map_err(Into::into)
        })
    }

    /// Latest send dispatch for one Membership. Ordinary send bookkeeping only;
    /// child-work recovery uses the persisted operation identity, not this scan.
    pub fn latest_send_dispatch(
        &self,
        conversation_id: &str,
        membership_id: &str,
    ) -> StoreResult<Option<ConversationDispatch>> {
        validate_identifier(conversation_id, "conversation_id")?;
        validate_identifier(membership_id, "membership_id")?;
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT id, conversation_id, membership_id, operation, state, session_mode,
                       runtime_conversation_path, error_code, created_at, updated_at
                     FROM conversation_dispatches
                     WHERE conversation_id=?1 AND membership_id=?2 AND operation='send'
                     ORDER BY created_at DESC, id DESC LIMIT 1",
                    params![conversation_id, membership_id],
                    dispatch_from_row,
                )
                .optional()
                .map_err(Into::into)
        })
    }

    /// Canonical agent-authored turn Event for one dispatch identity.
    /// Correlation plus author Membership is the existing unit; this does not
    /// invent a second transcript.
    pub fn agent_turn_event_for_dispatch(
        &self,
        conversation_id: &str,
        dispatch_id: &str,
    ) -> StoreResult<Option<ConversationEvent>> {
        validate_identifier(conversation_id, "conversation_id")?;
        validate_identifier(dispatch_id, "dispatch_id")?;
        self.with_connection(|connection| {
            let event_id: Option<String> = connection
                .query_row(
                    "SELECT e.id FROM events e
                     JOIN conversation_dispatches d ON d.id=e.correlation_id
                     WHERE e.conversation_id=?1 AND e.correlation_id=?2
                       AND e.author_membership_id=d.membership_id
                       AND e.kind='message'
                     ORDER BY e.sequence DESC LIMIT 1",
                    params![conversation_id, dispatch_id],
                    |row| row.get(0),
                )
                .optional()?;
            match event_id {
                Some(id) => event_by_id(connection, &id).map(Some),
                None => Ok(None),
            }
        })
    }

    pub fn migration_conversation(
        &self,
        source_kind: &str,
        source_identity: &str,
    ) -> StoreResult<Option<String>> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT conversation_id FROM migration_provenance WHERE source_kind=?1 AND source_identity=?2",
                    params![source_kind, source_identity],
                    |row| row.get(0),
                )
                .optional()
                .map_err(Into::into)
        })
    }

    /// Remove a partially written deterministic migration target before the
    /// source is replayed. A completed provenance row always wins and is never
    /// modified here.
    #[doc(hidden)]
    pub fn reset_incomplete_migration_conversation(
        &self,
        source_kind: &str,
        source_identity: &str,
        conversation_id: &str,
    ) -> StoreResult<()> {
        validate_required_text(source_kind, "migration_source_kind")?;
        validate_required_text(source_identity, "migration_source_identity")?;
        validate_identifier(conversation_id, "conversation_id")?;
        self.with_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let completed: Option<i64> = transaction
                .query_row(
                    "SELECT 1 FROM migration_provenance
                     WHERE source_kind=?1 AND source_identity=?2",
                    params![source_kind, source_identity],
                    |row| row.get(0),
                )
                .optional()?;
            if completed.is_none() {
                transaction.execute(
                    "DELETE FROM event_search WHERE conversation_id=?1",
                    params![conversation_id],
                )?;
                transaction.execute(
                    "DELETE FROM event_parts WHERE event_id IN
                     (SELECT id FROM events WHERE conversation_id=?1)",
                    params![conversation_id],
                )?;
                transaction.execute(
                    "DELETE FROM direct_turns WHERE conversation_id=?1",
                    params![conversation_id],
                )?;
                transaction.execute(
                    "DELETE FROM events WHERE conversation_id=?1",
                    params![conversation_id],
                )?;
                for table in [
                    "subagent_dispatch_deliveries",
                    "subagent_dispatch_claims",
                    "conversation_dispatches",
                    "runtime_bindings",
                    "source_links",
                    "memberships",
                ] {
                    transaction.execute(
                        &format!("DELETE FROM {table} WHERE conversation_id=?1"),
                        params![conversation_id],
                    )?;
                }
                transaction.execute(
                    "DELETE FROM conversations WHERE id=?1",
                    params![conversation_id],
                )?;
            }
            transaction.commit()?;
            Ok(())
        })
    }

    pub fn record_migration(
        &self,
        source_kind: &str,
        source_identity: &str,
        conversation_id: &str,
    ) -> StoreResult<()> {
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO migration_provenance(source_kind, source_identity, conversation_id)
                 VALUES (?1, ?2, ?3) ON CONFLICT(source_kind, source_identity) DO NOTHING",
                params![source_kind, source_identity, conversation_id],
            )?;
            Ok(())
        })
    }

    /// Export public Conversation facts without runtime handles, executor
    /// claims, absolute paths, or migration internals.
    pub fn export_bundle(
        &self,
        destination: &Path,
        conversation_ids: &[String],
    ) -> StoreResult<serde_json::Value> {
        path_security::validate_export_destination(destination)?;
        self.with_connection(|connection| {
            let selected_ids = if conversation_ids.is_empty() {
                list_inner(connection, false)?
                    .into_iter()
                    .map(|summary| summary.id)
                    .collect::<Vec<_>>()
            } else {
                conversation_ids
                    .iter()
                    .map(|id| id.trim().to_owned())
                    .filter(|id| !id.is_empty())
                    .collect::<Vec<_>>()
            };
            if selected_ids.is_empty() {
                return Err(anyhow!("conversation_export_empty"));
            }
            if selected_ids.len() > 500 {
                return Err(anyhow!("conversation_export_entry_limit_exceeded"));
            }
            let mut conversations = Vec::new();
            for id in &selected_ids {
                let conversation = get_inner(connection, id)?;
                let mut events = Vec::new();
                let mut cursor = None;
                loop {
                    let page = page_events_inner(connection, id, cursor, MAX_EVENT_PAGE_SIZE)?;
                    let done = page.next_cursor.is_none() || page.events.is_empty();
                    cursor = page.next_cursor.and_then(|value| value.parse::<i64>().ok());
                    events.extend(page.events);
                    if done {
                        break;
                    }
                }
                conversations
                    .push(serde_json::json!({ "conversation": conversation, "events": events }));
            }
            let bundle = serde_json::json!({
                "kind": "lico-conversation-bundle",
                "schemaVersion": super::CONVERSATION_SCHEMA_VERSION,
                "conversations": conversations,
            });
            let text = serde_json::to_string_pretty(&bundle)?;
            path_security::atomic_write_private_text(destination, &text)?;
            Ok(serde_json::json!({ "ok": true, "count": conversations.len() }))
        })
    }

    /// Import one current-schema public bundle. Identity collisions never
    /// overwrite existing local facts.
    pub fn import_bundle(&self, source: &Path) -> StoreResult<serde_json::Value> {
        let bytes = std::fs::read(source)?;
        if bytes.len() > 64 * 1024 * 1024 {
            return Err(anyhow!("conversation_import_too_large"));
        }
        let bundle: serde_json::Value = serde_json::from_slice(&bytes)?;
        if bundle.get("kind").and_then(serde_json::Value::as_str)
            != Some("lico-conversation-bundle")
            || bundle
                .get("schemaVersion")
                .and_then(serde_json::Value::as_str)
                != Some(super::CONVERSATION_SCHEMA_VERSION)
        {
            return Err(anyhow!("conversation_import_schema_invalid"));
        }
        let entries = bundle
            .get("conversations")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| anyhow!("conversation_import_payload_invalid"))?;
        if entries.len() > 500 {
            return Err(anyhow!("conversation_import_entry_limit_exceeded"));
        }
        let mut parsed = Vec::with_capacity(entries.len());
        let mut already_present = 0usize;
        for entry in entries {
            let conversation: Conversation = serde_json::from_value(
                entry
                    .get("conversation")
                    .cloned()
                    .ok_or_else(|| anyhow!("conversation_import_payload_invalid"))?,
            )?;
            let raw_events = entry
                .get("events")
                .and_then(serde_json::Value::as_array)
                .ok_or_else(|| anyhow!("conversation_import_payload_invalid"))?;
            if raw_events.len() > 100_000 {
                return Err(anyhow!("conversation_import_event_limit_exceeded"));
            }
            let events = raw_events
                .iter()
                .cloned()
                .map(serde_json::from_value::<ConversationEvent>)
                .collect::<std::result::Result<Vec<_>, _>>()?;
            validate_import_entry(&conversation, &events)?;
            match self.get(&conversation.id) {
                Ok(existing) if existing == conversation => {
                    if self.all_events(&conversation.id)? != events {
                        return Err(anyhow!("conversation_import_identity_conflict"));
                    }
                    already_present += 1;
                }
                Ok(_) => return Err(anyhow!("conversation_import_identity_conflict")),
                Err(error) if error.to_string() == "conversation_not_found" => {
                    parsed.push((conversation, events));
                }
                Err(error) => return Err(error),
            }
        }
        let imported = parsed.len();
        self.with_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            for (conversation, events) in &parsed {
                insert_import_entry(&transaction, conversation, events)?;
            }
            transaction.commit()?;
            Ok(())
        })?;
        Ok(serde_json::json!({
            "ok": true,
            "imported": imported,
            "alreadyPresent": already_present
        }))
    }

    fn all_events(&self, conversation_id: &str) -> StoreResult<Vec<ConversationEvent>> {
        self.with_connection(|connection| all_events_inner(connection, conversation_id))
    }

    fn membership(&self, conversation_id: &str, membership_id: &str) -> StoreResult<Membership> {
        self.with_connection(|connection| connection.query_row("SELECT m.id, m.conversation_id, p.id, p.kind, p.display_name, p.agent_id, p.created_at, m.access, m.status, m.joined_at, m.left_at FROM memberships m JOIN principals p ON p.id=m.principal_id WHERE m.conversation_id=?1 AND m.id=?2", params![conversation_id, membership_id], membership_from_row).map_err(Into::into))
    }
}

fn all_events_inner(
    connection: &impl CountedSqlite,
    conversation_id: &str,
) -> StoreResult<Vec<ConversationEvent>> {
    let mut events = Vec::new();
    let mut cursor = None;
    loop {
        let page = page_events_inner(connection, conversation_id, cursor, MAX_EVENT_PAGE_SIZE)?;
        let Some(last) = page.events.last() else {
            break;
        };
        cursor = Some(last.sequence);
        events.extend(page.events);
        if events.len() >= page.total_count as usize {
            break;
        }
    }
    Ok(events)
}

fn memberships(
    connection: &impl CountedSqlite,
    conversation_id: &str,
) -> StoreResult<Vec<Membership>> {
    let mut statement = connection.prepare("SELECT m.id, m.conversation_id, p.id, p.kind, p.display_name, p.agent_id, p.created_at, m.access, m.status, m.joined_at, m.left_at FROM memberships m JOIN principals p ON p.id=m.principal_id WHERE m.conversation_id=?1 ORDER BY m.joined_at ASC, m.id ASC")?;
    let rows = statement.query_map(params![conversation_id], membership_from_row)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewEventPart {
    #[serde(default)]
    pub id: String,
    pub kind: EventPartKind,
    pub content: String,
}

impl NewEventPart {
    /// One image part whose content is the canonical attachment metadata
    /// encoding. Admission and validation happen before this write door.
    pub fn image(attachment: &ImageAttachment) -> Self {
        Self {
            id: String::new(),
            kind: EventPartKind::Image,
            content: attachment.part_content(),
        }
    }
}
fn converge_duplicate_memberships(
    connection: &Connection,
    conversation_id: Option<&str>,
) -> StoreResult<()> {
    let mut groups: Vec<(String, String)> = Vec::new();
    {
        let mut statement = if conversation_id.is_some() {
            connection.prepare(
                "SELECT conversation_id, principal_id FROM memberships
                 WHERE conversation_id=?1
                 GROUP BY conversation_id, principal_id
                 HAVING COUNT(*) > 1",
            )?
        } else {
            connection.prepare(
                "SELECT conversation_id, principal_id FROM memberships
                 GROUP BY conversation_id, principal_id
                 HAVING COUNT(*) > 1",
            )?
        };
        if let Some(conversation_id) = conversation_id {
            let rows = statement.query_map(params![conversation_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            for row in rows {
                groups.push(row?);
            }
        } else {
            let rows = statement.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            for row in rows {
                groups.push(row?);
            }
        }
    }
    for (conversation_id, principal_id) in groups {
        retarget_duplicate_membership_group(connection, &conversation_id, &principal_id)?;
    }
    Ok(())
}

fn retarget_duplicate_membership_group(
    connection: &Connection,
    conversation_id: &str,
    principal_id: &str,
) -> StoreResult<()> {
    let mut statement = connection.prepare(
        "SELECT id, status, joined_at FROM memberships
         WHERE conversation_id=?1 AND principal_id=?2
         ORDER BY CASE status WHEN 'active' THEN 0 ELSE 1 END,
                  joined_at DESC, id DESC",
    )?;
    let rows = statement.query_map(params![conversation_id, principal_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
        ))
    })?;
    let mut ids = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    drop(statement);
    let Some((survivor, _, _)) = ids.first().cloned() else {
        return Ok(());
    };
    for (discarded, _, _) in ids.drain(1..) {
        retarget_membership_id(connection, &discarded, &survivor)?;
        connection.execute("DELETE FROM memberships WHERE id=?1", params![discarded])?;
    }
    Ok(())
}

fn retarget_membership_id(connection: &Connection, from: &str, to: &str) -> StoreResult<()> {
    if from == to {
        return Ok(());
    }
    let has_native_sessions: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='conversation_native_sessions')",
        [], |row|row.get(0),
    )?;
    if has_native_sessions {
        connection.execute(
            "INSERT OR IGNORE INTO conversation_native_sessions
             SELECT conversation_id,?2,native_session_id FROM conversation_native_sessions WHERE membership_id=?1",
            params![from, to],
        )?;
        connection.execute(
            "DELETE FROM conversation_native_sessions WHERE membership_id=?1",
            params![from],
        )?;
    }
    connection.execute(
        "UPDATE events SET author_membership_id=?2 WHERE author_membership_id=?1",
        params![from, to],
    )?;
    connection.execute(
        "UPDATE conversations SET assistant_membership_id=?2 WHERE assistant_membership_id=?1",
        params![from, to],
    )?;
    let survivor_has_profile: Option<i64> = connection
        .query_row(
            "SELECT 1 FROM membership_profiles WHERE membership_id=?1",
            params![to],
            |row| row.get(0),
        )
        .optional()?;
    if survivor_has_profile.is_some() {
        connection.execute(
            "DELETE FROM membership_profiles WHERE membership_id=?1",
            params![from],
        )?;
    } else {
        connection.execute(
            "UPDATE membership_profiles SET membership_id=?2 WHERE membership_id=?1",
            params![from, to],
        )?;
    }
    connection.execute(
        "DELETE FROM runtime_bindings
         WHERE membership_id=?1 AND (conversation_id, lane) IN (
           SELECT conversation_id, lane FROM runtime_bindings WHERE membership_id=?2
         )",
        params![from, to],
    )?;
    connection.execute(
        "UPDATE runtime_bindings SET membership_id=?2 WHERE membership_id=?1",
        params![from, to],
    )?;
    connection.execute(
        "DELETE FROM direct_turns
         WHERE membership_id=?1 AND source_event_id IN (
           SELECT source_event_id FROM direct_turns WHERE membership_id=?2
         )",
        params![from, to],
    )?;
    connection.execute(
        "UPDATE direct_turns SET membership_id=?2 WHERE membership_id=?1",
        params![from, to],
    )?;
    connection.execute(
        "UPDATE conversation_dispatches SET membership_id=?2 WHERE membership_id=?1",
        params![from, to],
    )?;
    const ACTIVE_CLAIM: &str = "'claimed','running','cancel-requested','reconciliation-required'";
    connection.execute(
        &format!(
            "UPDATE subagent_dispatch_claims SET parent_dispatch_id=NULL
             WHERE parent_dispatch_id IN (
               SELECT id FROM subagent_dispatch_claims discarded
               WHERE (discarded.caller_membership_id=?1 OR discarded.target_membership_id=?1)
                 AND discarded.state IN ({ACTIVE_CLAIM})
                 AND EXISTS (
                   SELECT 1 FROM subagent_dispatch_claims kept
                   WHERE kept.conversation_id=discarded.conversation_id
                     AND kept.caller_membership_id=CASE
                       WHEN discarded.caller_membership_id=?1 THEN ?2
                       ELSE discarded.caller_membership_id END
                     AND kept.target_membership_id=CASE
                       WHEN discarded.target_membership_id=?1 THEN ?2
                       ELSE discarded.target_membership_id END
                     AND kept.state IN ({ACTIVE_CLAIM})
                 )
             )"
        ),
        params![from, to],
    )?;
    connection.execute(
        &format!(
            "DELETE FROM subagent_dispatch_claims
             WHERE (caller_membership_id=?1 OR target_membership_id=?1)
               AND state IN ({ACTIVE_CLAIM})
               AND EXISTS (
                 SELECT 1 FROM subagent_dispatch_claims kept
                 WHERE kept.conversation_id=subagent_dispatch_claims.conversation_id
                   AND kept.caller_membership_id=CASE
                     WHEN subagent_dispatch_claims.caller_membership_id=?1 THEN ?2
                     ELSE subagent_dispatch_claims.caller_membership_id END
                   AND kept.target_membership_id=CASE
                     WHEN subagent_dispatch_claims.target_membership_id=?1 THEN ?2
                     ELSE subagent_dispatch_claims.target_membership_id END
                   AND kept.state IN ({ACTIVE_CLAIM})
               )"
        ),
        params![from, to],
    )?;
    connection.execute(
        "UPDATE subagent_dispatch_claims SET caller_membership_id=?2 WHERE caller_membership_id=?1",
        params![from, to],
    )?;
    connection.execute(
        "UPDATE subagent_dispatch_claims SET target_membership_id=?2 WHERE target_membership_id=?1",
        params![from, to],
    )?;
    connection.execute(
        "UPDATE subagent_mcp_inbound SET caller_membership_id=?2 WHERE caller_membership_id=?1",
        params![from, to],
    )?;
    connection.execute(
        "UPDATE subagent_mcp_inbound SET target_membership_id=?2 WHERE target_membership_id=?1",
        params![from, to],
    )?;
    Ok(())
}

fn normalize_reserved_group_after_legacy_import(connection: &mut Connection) -> StoreResult<()> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    normalize_reserved_group(&transaction)?;
    rename_reserved_group(&transaction)?;
    transaction.commit()?;
    Ok(())
}

fn ensure_default_local_group_inner(
    connection: &mut CountedConnection<'_>,
) -> StoreResult<Conversation> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let canonical_exists: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM conversations WHERE id=?1)",
        params![DEFAULT_LOCAL_AGENT_GROUP_ID],
        |row| row.get(0),
    )?;

    if !canonical_exists {
        if let Some(existing_id) = reserved_group_conversation_id(&transaction)? {
            transaction.execute(
                "INSERT INTO conversations(
                   id, title, archived, pinned, is_group, strategy_revision,
                   revision, created_at, updated_at
                 ) SELECT ?2, title, archived, pinned, 1, strategy_revision,
                          revision, created_at, updated_at
                   FROM conversations WHERE id=?1",
                params![existing_id, DEFAULT_LOCAL_AGENT_GROUP_ID],
            )?;
            for table in [
                "memberships",
                "events",
                "direct_turns",
                "source_links",
                "runtime_bindings",
                "conversation_dispatches",
                "subagent_dispatch_claims",
                "subagent_mcp_inbound",
                "migration_provenance",
                "event_admission_keys",
            ] {
                transaction.execute(
                    &format!("UPDATE {table} SET conversation_id=?2 WHERE conversation_id=?1"),
                    params![existing_id, DEFAULT_LOCAL_AGENT_GROUP_ID],
                )?;
            }
            transaction.execute(
                "UPDATE event_search SET conversation_id=?2 WHERE conversation_id=?1",
                params![existing_id, DEFAULT_LOCAL_AGENT_GROUP_ID],
            )?;
            transaction.execute(
                "DELETE FROM conversations WHERE id=?1",
                params![existing_id],
            )?;
        } else {
            let now = now_ms();
            transaction.execute(
                "INSERT INTO conversations(
                   id, title, archived, pinned, is_group, revision, created_at, updated_at
                 ) VALUES (?1, ?2, 0, 1, 1, 0, ?3, ?3)",
                params![
                    DEFAULT_LOCAL_AGENT_GROUP_ID,
                    DEFAULT_LOCAL_AGENT_GROUP_TITLE,
                    now
                ],
            )?;
        }
    }

    let now = now_ms();
    let owner = Principal {
        id: "human:local".into(),
        kind: PrincipalKind::Human,
        display_name: "Local User".into(),
        agent_id: None,
        created_at_unix_ms: now,
    };
    upsert_principal(&transaction, &owner)?;
    let active_owner: Option<String> = transaction
        .query_row(
            "SELECT id FROM memberships
             WHERE conversation_id=?1 AND principal_id=?2 AND status='active'
             ORDER BY joined_at ASC, id ASC LIMIT 1",
            params![DEFAULT_LOCAL_AGENT_GROUP_ID, owner.id],
            |row| row.get(0),
        )
        .optional()?;
    let membership_changed = if let Some(membership_id) = active_owner {
        transaction.execute(
            "UPDATE memberships SET access='owner'
             WHERE id=?1 AND access<>'owner'",
            params![membership_id],
        )? > 0
    } else {
        let left_membership: Option<String> = transaction
            .query_row(
                "SELECT id FROM memberships
                 WHERE conversation_id=?1 AND principal_id=?2 AND status='left'
                 ORDER BY joined_at ASC, id ASC LIMIT 1",
                params![DEFAULT_LOCAL_AGENT_GROUP_ID, owner.id],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(membership_id) = left_membership {
            let active = lifecycle::reactivate_membership();
            transaction.execute(
                "UPDATE memberships
                 SET access='owner', status=?2, left_at=NULL
                 WHERE id=?1 AND status=?3",
                params![
                    membership_id,
                    active.as_str(),
                    super::MembershipStatus::Left.as_str()
                ],
            )?;
        } else {
            transaction.execute(
                "INSERT INTO memberships(
                   id, conversation_id, principal_id, access, status, joined_at
                 ) VALUES (?1, ?2, ?3, 'owner', 'active', ?4)",
                params![
                    new_id("membership"),
                    DEFAULT_LOCAL_AGENT_GROUP_ID,
                    owner.id,
                    now
                ],
            )?;
        }
        true
    };
    let conversation_changed = transaction.execute(
        "UPDATE conversations
         SET title=CASE
               WHEN trim(title)='' OR lower(trim(title)) IN ('lico', 'lico-group-default')
               THEN ?2 ELSE title END,
             archived=0, pinned=1, is_group=1,
             revision=revision+1, updated_at=?3
         WHERE id=?1 AND (
           trim(title)='' OR lower(trim(title)) IN ('lico', 'lico-group-default') OR
           archived<>0 OR pinned<>1 OR is_group<>1
         )",
        params![
            DEFAULT_LOCAL_AGENT_GROUP_ID,
            DEFAULT_LOCAL_AGENT_GROUP_TITLE,
            now
        ],
    )? > 0;
    if membership_changed && !conversation_changed {
        bump_revision(&transaction, DEFAULT_LOCAL_AGENT_GROUP_ID, now)?;
    }
    transaction.commit()?;
    self::get_inner(connection, DEFAULT_LOCAL_AGENT_GROUP_ID)
}

fn reserved_group_conversation_id(connection: &Connection) -> StoreResult<Option<String>> {
    for query in [
        "SELECT conversation_id FROM migration_provenance
         WHERE source_kind='group' AND source_identity=?1",
        "SELECT conversation_id FROM source_links
         WHERE source_kind='group' AND native_identity=?1",
        "SELECT id FROM conversations WHERE id=?1",
    ] {
        let found: Option<String> = connection
            .query_row(query, params![DEFAULT_LOCAL_AGENT_GROUP_ID], |row| {
                row.get(0)
            })
            .optional()?;
        if found.is_some() {
            return Ok(found);
        }
    }
    Ok(None)
}

fn rename_reserved_group(connection: &Connection) -> StoreResult<()> {
    let Some(conversation_id) = reserved_group_conversation_id(connection)? else {
        return Ok(());
    };
    connection.execute(
        "UPDATE conversations SET title=?2, revision=revision+1
         WHERE id=?1 AND lower(trim(title)) IN ('lico', 'lico-group-default')",
        params![conversation_id, DEFAULT_LOCAL_AGENT_GROUP_TITLE],
    )?;
    Ok(())
}

/// Deterministic reserved-default-group normalization. Resolves at most one
/// conversation by the historical identity order (migration provenance, then
/// group source link, then the direct default-group id), removes only strictly
/// classified automatic joined/left membership-changed events, retains exactly
/// one active durable membership per historically known Agent principal, and
/// resequences the surviving events contiguously in their original order.
/// A missing reserved group is a successful no-op and never creates anything.
fn normalize_reserved_group(connection: &Connection) -> StoreResult<()> {
    let Some(conversation_id) = reserved_group_conversation_id(connection)? else {
        return Ok(());
    };

    let mut membership_events: Vec<(String, String)> = Vec::new();
    {
        let mut statement = connection.prepare(
            "SELECT e.id, p.content FROM events e
             JOIN event_parts p ON p.event_id=e.id AND p.ordinal=0 AND p.kind='metadata'
             WHERE e.conversation_id=?1
               AND e.kind='membership-changed'
               AND e.finalized=1
               AND e.author_membership_id IS NULL
               AND e.causation_id IS NULL
               AND e.correlation_id IS NULL
               AND (SELECT COUNT(*) FROM event_parts ep WHERE ep.event_id=e.id)=1",
        )?;
        let rows = statement.query_map(params![conversation_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            membership_events.push(row?);
        }
    }

    let mut historically_known: std::collections::BTreeSet<String> =
        std::collections::BTreeSet::new();
    let mut automatic: Vec<String> = Vec::new();
    for (event_id, content) in &membership_events {
        if let Some((membership_id, principal_id)) = automatic_membership_churn(content) {
            let matches_agent_membership: Option<i64> = connection
                .query_row(
                    "SELECT 1 FROM memberships m
                     JOIN principals p ON p.id=m.principal_id
                     WHERE m.id=?1 AND m.conversation_id=?2
                       AND m.principal_id=?3 AND p.kind='agent'",
                    params![membership_id, conversation_id, principal_id],
                    |row| row.get(0),
                )
                .optional()?;
            if matches_agent_membership.is_some() {
                automatic.push(event_id.clone());
            }
        }
    }
    {
        let mut statement = connection.prepare(
            "SELECT p.id FROM principals p
             JOIN memberships m ON m.principal_id=p.id
             WHERE m.conversation_id=?1 AND p.kind='agent'",
        )?;
        let rows = statement.query_map(params![conversation_id], |row| row.get::<_, String>(0))?;
        for principal in rows {
            historically_known.insert(principal?);
        }
    }

    if !automatic.is_empty() {
        let placeholders = std::iter::repeat("?")
            .take(automatic.len())
            .collect::<Vec<_>>()
            .join(",");
        connection.execute(
            &format!("DELETE FROM event_search WHERE event_id IN ({placeholders})"),
            rusqlite::params_from_iter(automatic.iter()),
        )?;
        connection.execute(
            &format!("DELETE FROM events WHERE id IN ({placeholders})"),
            rusqlite::params_from_iter(automatic.iter()),
        )?;
    }

    for principal_id in historically_known {
        let active: Option<String> = connection
            .query_row(
                "SELECT id FROM memberships
                 WHERE conversation_id=?1 AND principal_id=?2 AND status='active'",
                params![conversation_id, principal_id],
                |row| row.get(0),
            )
            .optional()?;
        if active.is_some() {
            continue;
        }
        let left: Option<String> = connection
            .query_row(
                "SELECT id FROM memberships
                 WHERE conversation_id=?1 AND principal_id=?2 AND status='left'
                 ORDER BY joined_at DESC, id DESC LIMIT 1",
                params![conversation_id, principal_id],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(membership_id) = left {
            let active = lifecycle::reactivate_membership();
            connection.execute(
                "UPDATE memberships SET status=?3, left_at=NULL
                 WHERE id=?1 AND conversation_id=?2 AND status=?4",
                params![
                    membership_id,
                    conversation_id,
                    active.as_str(),
                    super::MembershipStatus::Left.as_str()
                ],
            )?;
            continue;
        }
    }

    converge_duplicate_memberships(connection, Some(&conversation_id))?;

    let mut sequence = 0i64;
    let mut statement = connection
        .prepare("SELECT id FROM events WHERE conversation_id=?1 ORDER BY sequence ASC, id ASC")?;
    let rows = statement.query_map(params![conversation_id], |row| row.get::<_, String>(0))?;
    let event_ids: Vec<String> = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    for event_id in event_ids {
        sequence += 1;
        connection.execute(
            "UPDATE events SET sequence=?2 WHERE id=?1 AND conversation_id=?3",
            params![event_id, sequence, conversation_id],
        )?;
    }
    Ok(())
}

/// Parse the exact retired metadata envelope. Event-level system/finalization
/// predicates and the same-group Agent Membership match are checked by the
/// caller; any missing or additional field is preserved.
fn automatic_membership_churn(content: &str) -> Option<(String, String)> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(content) else {
        return None;
    };
    let Some(object) = value.as_object() else {
        return None;
    };
    if object.len() != 3 {
        return None;
    }
    let Some(membership_id) = object
        .get("membershipId")
        .and_then(serde_json::Value::as_str)
    else {
        return None;
    };
    let Some(principal_id) = object
        .get("principalId")
        .and_then(serde_json::Value::as_str)
    else {
        return None;
    };
    let Some(change) = object.get("change").and_then(serde_json::Value::as_str) else {
        return None;
    };
    if membership_id.is_empty()
        || principal_id.is_empty()
        || (change != "joined" && change != "left")
    {
        return None;
    }
    Some((membership_id.to_owned(), principal_id.to_owned()))
}

fn ensure_column(
    connection: &Connection,
    table: &str,
    column: &str,
    declaration: &str,
) -> StoreResult<()> {
    let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
    let existing = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if !existing.iter().any(|name| name == column) {
        connection.execute_batch(&format!(
            "ALTER TABLE {table} ADD COLUMN {column} {declaration}"
        ))?;
    }
    Ok(())
}

fn list_inner(
    connection: &impl CountedSqlite,
    include_archived: bool,
) -> StoreResult<Vec<ConversationSummary>> {
    let mut statement = connection.prepare(
        "SELECT c.id, c.title, c.archived, c.pinned, c.is_group, c.revision, c.updated_at,
         (SELECT COUNT(*) FROM memberships m WHERE m.conversation_id=c.id AND m.status='active'),
         (SELECT COUNT(*) FROM events e WHERE e.conversation_id=c.id),
         (SELECT group_concat(p.agent_id, char(31) ORDER BY m.joined_at ASC, m.rowid ASC)
           FROM memberships m
           JOIN principals p ON p.id=m.principal_id
           WHERE m.conversation_id=c.id AND m.status='active' AND p.kind='agent'
             AND p.agent_id IS NOT NULL),
         (SELECT EXISTS(SELECT 1 FROM memberships lm
           JOIN principals lp ON lp.id=lm.principal_id
           WHERE lm.conversation_id=c.id AND lm.status='active'
             AND lp.kind='human' AND lm.access='owner'))
         FROM conversations c WHERE (?1=1 OR c.archived=0)
         ORDER BY c.pinned DESC, c.updated_at DESC, c.id DESC",
    )?;
    let rows = statement.query_map(params![include_archived as i64], summary_from_row)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

fn get_inner(connection: &impl CountedSqlite, id: &str) -> StoreResult<Conversation> {
    let base = connection
        .query_row(
            "SELECT id, title, archived, pinned, is_group, strategy_revision,
             assistant_membership_id, revision, created_at, updated_at,
             (SELECT COUNT(*) FROM events WHERE conversation_id=c.id)
             FROM conversations c WHERE id=?1",
            params![id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)? != 0,
                    row.get::<_, i64>(3)? != 0,
                    row.get::<_, i64>(4)? != 0,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, i64>(8)?,
                    row.get::<_, i64>(9)?,
                    row.get::<_, i64>(10)?,
                ))
            },
        )
        .optional()?;
    let Some((
        id,
        title,
        archived,
        pinned,
        is_group,
        strategy_revision,
        assistant_membership_id,
        revision,
        created,
        updated,
        event_count,
    )) = base
    else {
        return Err(anyhow!("conversation_not_found"));
    };
    let memberships = memberships(connection, &id)?;
    Ok(Conversation {
        id,
        title,
        archived,
        pinned,
        is_group,
        strategy_revision,
        assistant_membership_id,
        revision,
        created_at_unix_ms: created,
        updated_at_unix_ms: updated,
        memberships,
        event_count,
    })
}

fn page_events_inner(
    connection: &impl CountedSqlite,
    conversation_id: &str,
    after_sequence: Option<i64>,
    limit: usize,
) -> StoreResult<EventPage> {
    page_events_window_inner(
        connection,
        conversation_id,
        EventPagePosition::After(after_sequence.unwrap_or(0)),
        limit,
    )
}

fn page_events_window_inner(
    connection: &impl CountedSqlite,
    conversation_id: &str,
    position: EventPagePosition,
    limit: usize,
) -> StoreResult<EventPage> {
    ensure_conversation(connection, conversation_id)?;
    let (operator, order, cursor) = match position {
        EventPagePosition::After(sequence) => (">", "ASC", sequence),
        EventPagePosition::Before(sequence) => ("<", "DESC", sequence),
        EventPagePosition::Latest => ("<=", "DESC", i64::MAX),
    };
    let mut statement = connection.prepare(&format!(
        "SELECT id, conversation_id, sequence, author_membership_id, kind,
         causation_id, correlation_id, created_at, finalized
         FROM events WHERE conversation_id=?1 AND sequence {operator} ?2
         ORDER BY sequence {order} LIMIT ?3"
    ))?;
    let rows = statement.query_map(
        params![conversation_id, cursor, limit as i64],
        event_from_row,
    )?;
    let mut events = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    if !matches!(position, EventPagePosition::After(_)) {
        events.reverse();
    }
    let event_ids = events
        .iter()
        .map(|event| event.id.clone())
        .collect::<Vec<_>>();
    let parts_by_event = parts_batch(connection, &event_ids)?;
    for event in &mut events {
        if let Some(parts) = parts_by_event.get(&event.id) {
            event.parts.clone_from(parts);
        }
    }
    let first_sequence = events.first().map(|event| event.sequence);
    let (total_count, has_earlier): (i64, bool) = connection.query_row(
        "SELECT COUNT(*), EXISTS(
           SELECT 1 FROM events WHERE conversation_id=?1 AND sequence<?2
         ) FROM events WHERE conversation_id=?1",
        params![conversation_id, first_sequence],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let next_cursor = events.last().map(|event| event.sequence.to_string());
    Ok(EventPage {
        events,
        next_cursor,
        total_count,
        has_earlier,
        next_before_sequence: first_sequence.filter(|_| has_earlier),
    })
}

fn search_inner(
    connection: &impl CountedSqlite,
    fts_query: &str,
    limit: usize,
) -> StoreResult<Vec<ConversationEvent>> {
    let mut statement = connection.prepare(
        "SELECT e.id, e.conversation_id, e.sequence, e.author_membership_id,
         e.kind, e.causation_id, e.correlation_id, e.created_at, e.finalized
         FROM event_search s JOIN events e ON e.id=s.event_id
         WHERE s.content MATCH ?1 ORDER BY e.created_at DESC LIMIT ?2",
    )?;
    let rows = statement.query_map(params![fts_query, limit as i64], event_from_row)?;
    let mut events = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    let event_ids = events
        .iter()
        .map(|event| event.id.clone())
        .collect::<Vec<_>>();
    let parts_by_event = parts_batch(connection, &event_ids)?;
    for event in &mut events {
        if let Some(parts) = parts_by_event.get(&event.id) {
            event.parts.clone_from(parts);
        }
    }
    Ok(events)
}

fn event_by_id(connection: &impl CountedSqlite, event_id: &str) -> StoreResult<ConversationEvent> {
    let mut event = connection.query_row(
        "SELECT id, conversation_id, sequence, author_membership_id, kind,
         causation_id, correlation_id, created_at, finalized
         FROM events WHERE id=?1",
        params![event_id],
        event_from_row,
    )?;
    if let Some(parts) = parts_batch(connection, &[event_id.to_owned()])?.remove(event_id) {
        event.parts = parts;
    }
    Ok(event)
}

/// Load all event parts for one bounded event set with a single indexed
/// query, folded by stable event identity and part ordinal.
fn parts_batch(
    connection: &impl CountedSqlite,
    event_ids: &[String],
) -> StoreResult<HashMap<String, Vec<EventPart>>> {
    let mut parts_by_event = HashMap::with_capacity(event_ids.len());
    if event_ids.is_empty() {
        return Ok(parts_by_event);
    }
    let placeholders = vec!["?"; event_ids.len()].join(",");
    let sql = format!(
        "SELECT id, event_id, ordinal, kind, content, created_at
         FROM event_parts WHERE event_id IN ({placeholders})
           AND runtime_cursor IS NULL
         ORDER BY event_id ASC, ordinal ASC"
    );
    let mut statement = connection.prepare(&sql)?;
    let rows = statement.query_map(
        params_from_iter(event_ids.iter().map(String::as_str)),
        part_from_row,
    )?;
    for part in rows {
        let part = part?;
        parts_by_event
            .entry(part.event_id.clone())
            .or_default()
            .push(part);
    }
    Ok(parts_by_event)
}

fn ensure_conversation(connection: &impl CountedSqlite, id: &str) -> StoreResult<()> {
    let exists: Option<i64> = connection
        .query_row(
            "SELECT 1 FROM conversations WHERE id=?1",
            params![id],
            |row| row.get(0),
        )
        .optional()?;
    if exists.is_none() {
        return Err(anyhow!("conversation_not_found"));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn insert_event(
    connection: &impl CountedSqlite,
    event_id: &str,
    conversation_id: &str,
    author_membership_id: Option<&str>,
    kind: EventKind,
    parts: &[NewEventPart],
    causation_id: Option<&str>,
    correlation_id: Option<&str>,
    finalized: bool,
    require_active_author: bool,
    now: i64,
) -> StoreResult<ConversationEvent> {
    ensure_conversation(connection, conversation_id)?;
    if let Some(author) = author_membership_id {
        let author_status: Option<String> = connection
            .query_row(
                "SELECT status FROM memberships WHERE id=?1 AND conversation_id=?2",
                params![author, conversation_id],
                |row| row.get(0),
            )
            .optional()?;
        match author_status.as_deref() {
            None => return Err(anyhow!("event_author_not_found")),
            Some("active") => {}
            Some(_) if require_active_author => return Err(anyhow!("event_author_not_active")),
            Some(_) => {}
        }
    }
    let sequence: i64 = connection.query_row(
        "SELECT COALESCE(MAX(sequence), 0)+1 FROM events WHERE conversation_id=?1",
        params![conversation_id],
        |row| row.get(0),
    )?;
    connection.execute(
        "INSERT INTO events(id, conversation_id, sequence, author_membership_id, kind,
         causation_id, correlation_id, created_at, finalized)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            event_id,
            conversation_id,
            sequence,
            author_membership_id,
            enum_wire(kind)?,
            causation_id,
            correlation_id,
            now,
            finalized as i64,
        ],
    )?;
    let mut stored_parts = Vec::with_capacity(parts.len());
    for (ordinal, part) in parts.iter().enumerate() {
        if require_active_author || part.kind != EventPartKind::Text {
            validate_text(&part.content, "event_part_content")?;
        }
        let part_id = if part.id.trim().is_empty() {
            new_id("part")
        } else {
            part.id.clone()
        };
        connection.execute(
            "INSERT INTO event_parts(id, event_id, ordinal, kind, content, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                part_id,
                event_id,
                ordinal as i64,
                enum_wire(part.kind)?,
                part.content,
                now,
            ],
        )?;
        if matches!(part.kind, EventPartKind::Text | EventPartKind::Reasoning) {
            connection.execute(
                "INSERT INTO event_search(event_id, conversation_id, content) VALUES (?1, ?2, ?3)",
                params![event_id, conversation_id, part.content],
            )?;
        }
        stored_parts.push(EventPart {
            id: part_id,
            event_id: event_id.to_owned(),
            ordinal: ordinal as i64,
            kind: part.kind,
            content: part.content.clone(),
            created_at_unix_ms: now,
        });
    }
    bump_revision(connection, conversation_id, now)?;
    Ok(ConversationEvent {
        id: event_id.to_owned(),
        conversation_id: conversation_id.to_owned(),
        sequence,
        author_membership_id: author_membership_id.map(str::to_owned),
        kind,
        causation_id: causation_id.map(str::to_owned),
        correlation_id: correlation_id.map(str::to_owned),
        created_at_unix_ms: now,
        finalized,
        parts: stored_parts,
    })
}

fn enqueue_mention_turns_in_transaction(
    connection: &impl CountedSqlite,
    conversation_id: &str,
    source_event_id: &str,
    membership_ids: &[String],
) -> StoreResult<Vec<DirectTurn>> {
    let event_conversation: String = connection.query_row(
        "SELECT conversation_id FROM events WHERE id=?1",
        params![source_event_id],
        |row| row.get(0),
    )?;
    if event_conversation != conversation_id {
        return Err(anyhow!("mention_event_mismatch"));
    }
    let mut seen = std::collections::HashSet::with_capacity(membership_ids.len());
    let mut turns = Vec::new();
    for membership_id in membership_ids {
        if !seen.insert(membership_id.as_str()) {
            continue;
        }
        let runnable: Option<i64> = connection
            .query_row(
                "SELECT 1 FROM memberships m
                 JOIN principals p ON p.id=m.principal_id
                 WHERE m.id=?1 AND m.conversation_id=?2
                   AND m.status='active' AND p.kind='agent'
                   AND p.agent_id IS NOT NULL AND p.agent_id<>''",
                params![membership_id, conversation_id],
                |row| row.get(0),
            )
            .optional()?;
        if runnable.is_none() {
            continue;
        }
        let turn = DirectTurn {
            id: new_id("turn"),
            conversation_id: conversation_id.to_owned(),
            source_event_id: source_event_id.to_owned(),
            membership_id: membership_id.to_owned(),
            state: TurnState::Pending,
            ordinal: turns.len() as i64,
        };
        connection.execute(
            "INSERT INTO direct_turns(
               id, conversation_id, source_event_id, membership_id, state, ordinal
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                turn.id,
                turn.conversation_id,
                turn.source_event_id,
                turn.membership_id,
                enum_wire(turn.state)?,
                turn.ordinal,
            ],
        )?;
        turns.push(turn);
    }
    Ok(turns)
}

fn direct_turn_execution_context(
    connection: &impl CountedSqlite,
    turn_id: &str,
) -> StoreResult<DirectTurnExecutionContext> {
    let mut context: DirectTurnExecutionContext = connection.query_row(
        "SELECT t.id, t.conversation_id, t.source_event_id, t.membership_id,
                t.state, t.ordinal, p.agent_id,
                (SELECT ep.content FROM event_parts ep
                 WHERE ep.event_id=t.source_event_id AND ep.kind='text'
                 ORDER BY ep.ordinal ASC LIMIT 1),
                rb.runtime_session_id, rb.runtime_conversation_path,
                rb.working_directory, fp.preferred_model,
                fp.preferred_reasoning_effort,
                EXISTS(SELECT 1 FROM conversations c
                       WHERE c.id=t.conversation_id
                         AND c.assistant_membership_id=t.membership_id),
                fp.preferred_permission_mode, fp.preferred_instructions,
                fp.preferred_context_window
         FROM direct_turns t
         JOIN memberships m ON m.id=t.membership_id
            AND m.conversation_id=t.conversation_id AND m.status='active'
         JOIN principals p ON p.id=m.principal_id
            AND p.kind='agent' AND p.agent_id IS NOT NULL AND p.agent_id<>''
         LEFT JOIN runtime_bindings rb
            ON rb.conversation_id=t.conversation_id
            AND rb.membership_id=t.membership_id AND rb.lane='conversation'
         LEFT JOIN membership_profiles fp ON fp.membership_id=t.membership_id
         WHERE t.id=?1",
        params![turn_id],
        |row| {
            Ok(DirectTurnExecutionContext {
                turn: DirectTurn {
                    id: row.get(0)?,
                    conversation_id: row.get(1)?,
                    source_event_id: row.get(2)?,
                    membership_id: row.get(3)?,
                    state: decode(row.get(4)?)?,
                    ordinal: row.get(5)?,
                },
                agent_id: row.get(6)?,
                // An image-only source Event has no text part: the subquery
                // yields NULL and the turn dispatches on its attachments.
                source_content: row.get::<_, Option<String>>(7)?.unwrap_or_default(),
                source_attachments: Vec::new(),
                is_assistant: row.get(13)?,
                runtime_session_id: row.get(8)?,
                runtime_conversation_path: row.get(9)?,
                working_directory: row.get(10)?,
                preferred_model: row.get(11)?,
                preferred_reasoning_effort: row.get(12)?,
                preferred_permission_mode: row.get(14)?,
                preferred_instructions: row.get(15)?,
                preferred_context_window: row
                    .get::<_, Option<i64>>(16)?
                    .and_then(|value| u64::try_from(value).ok()),
            })
        },
    )?;
    context.source_attachments =
        image_attachments_for_event(connection, &context.turn.source_event_id)?;
    Ok(context)
}

/// Load the posted image attachment references of one source Event in part
/// order. Malformed part content is skipped so one unreadable part can never
/// block a member turn from dispatching.
fn image_attachments_for_event(
    connection: &impl CountedSqlite,
    event_id: &str,
) -> StoreResult<Vec<ImageAttachmentReference>> {
    let mut statement = connection.prepare(
        "SELECT id, content FROM event_parts
         WHERE event_id=?1 AND kind='image' AND runtime_cursor IS NULL
         ORDER BY ordinal ASC",
    )?;
    let rows = statement.query_map(params![event_id], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut references = Vec::new();
    for row in rows {
        let (part_id, content) = row?;
        if let Some(attachment) = ImageAttachment::from_part_content(&content) {
            references.push(ImageAttachmentReference {
                part_id,
                attachment,
            });
        }
    }
    Ok(references)
}

fn image_attachment_for_part(
    connection: &impl CountedSqlite,
    event_id: &str,
    part_id: &str,
) -> StoreResult<Option<ImageAttachmentReference>> {
    let row: Option<(String, String)> = connection
        .query_row(
            "SELECT id, content FROM event_parts
             WHERE event_id=?1 AND id=?2 AND kind='image' AND runtime_cursor IS NULL",
            params![event_id, part_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((part_id, content)) = row else {
        return Ok(None);
    };
    Ok(
        ImageAttachment::from_part_content(&content).map(|attachment| ImageAttachmentReference {
            part_id,
            attachment,
        }),
    )
}

fn upsert_principal(connection: &impl CountedSqlite, principal: &Principal) -> StoreResult<()> {
    let kind = enum_wire(principal.kind)?;
    let existing_kind: Option<String> = connection
        .query_row(
            "SELECT kind FROM principals WHERE id=?1",
            params![principal.id],
            |row| row.get(0),
        )
        .optional()?;
    if existing_kind
        .as_deref()
        .is_some_and(|existing| existing != kind)
    {
        return Err(anyhow!("principal_kind_immutable"));
    }
    connection.execute(
        "INSERT INTO principals(id, kind, display_name, agent_id, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(id) DO UPDATE SET display_name=excluded.display_name,
           agent_id=excluded.agent_id",
        params![
            principal.id,
            kind,
            principal.display_name,
            principal.agent_id,
            principal.created_at_unix_ms,
        ],
    )?;
    Ok(())
}

fn validate_import_entry(
    conversation: &Conversation,
    events: &[ConversationEvent],
) -> StoreResult<()> {
    validate_identifier(&conversation.id, "conversation_id")?;
    validate_text(&conversation.title, "conversation_title")?;
    if conversation.event_count != events.len() as i64 {
        return Err(anyhow!("conversation_import_count_mismatch"));
    }
    let mut membership_ids = std::collections::HashSet::new();
    let mut active_owners = 0usize;
    for membership in &conversation.memberships {
        if membership.conversation_id != conversation.id
            || !membership_ids.insert(membership.id.as_str())
        {
            return Err(anyhow!("conversation_import_membership_invalid"));
        }
        validate_identifier(&membership.id, "membership_id")?;
        validate_identifier(&membership.principal.id, "principal_id")?;
        validate_text(&membership.principal.display_name, "principal_display_name")?;
        if membership.status == super::MembershipStatus::Active
            && membership.access == MembershipAccess::Owner
        {
            active_owners += 1;
        }
    }
    if active_owners == 0 {
        return Err(anyhow!("conversation_import_owner_missing"));
    }
    let mut event_ids = std::collections::HashSet::new();
    let mut last_sequence = 0;
    for event in events {
        if event.conversation_id != conversation.id
            || event.sequence <= last_sequence
            || !event_ids.insert(event.id.as_str())
            || event
                .author_membership_id
                .as_deref()
                .is_some_and(|id| !membership_ids.contains(id))
        {
            return Err(anyhow!("conversation_import_event_invalid"));
        }
        last_sequence = event.sequence;
        for (ordinal, part) in event.parts.iter().enumerate() {
            if part.event_id != event.id || part.ordinal != ordinal as i64 {
                return Err(anyhow!("conversation_import_part_invalid"));
            }
            validate_text(&part.content, "event_part_content")?;
        }
    }
    Ok(())
}

fn insert_import_entry(
    connection: &impl CountedSqlite,
    conversation: &Conversation,
    events: &[ConversationEvent],
) -> StoreResult<()> {
    for membership in &conversation.memberships {
        let principal = &membership.principal;
        let existing: Option<(String, String, Option<String>, i64)> = connection
            .query_row(
                "SELECT kind, display_name, agent_id, created_at FROM principals WHERE id=?1",
                params![principal.id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?;
        let expected = (
            enum_wire(principal.kind)?,
            principal.display_name.clone(),
            principal.agent_id.clone(),
            principal.created_at_unix_ms,
        );
        if existing.as_ref().is_some_and(|value| value != &expected) {
            return Err(anyhow!("conversation_import_principal_conflict"));
        }
        connection.execute(
            "INSERT INTO principals(id, kind, display_name, agent_id, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT(id) DO NOTHING",
            params![
                principal.id,
                expected.0,
                principal.display_name,
                principal.agent_id,
                principal.created_at_unix_ms,
            ],
        )?;
    }
    connection.execute(
        "INSERT INTO conversations(
           id, title, archived, pinned, is_group, strategy_revision,
           revision, created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            conversation.id,
            conversation.title,
            conversation.archived as i64,
            conversation.pinned as i64,
            conversation.is_group as i64,
            conversation.strategy_revision,
            conversation.revision,
            conversation.created_at_unix_ms,
            conversation.updated_at_unix_ms,
        ],
    )?;
    for membership in &conversation.memberships {
        connection.execute(
            "INSERT INTO memberships(
               id, conversation_id, principal_id, access, status, joined_at, left_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                membership.id,
                conversation.id,
                membership.principal.id,
                enum_wire(membership.access)?,
                enum_wire(membership.status)?,
                membership.joined_at_unix_ms,
                membership.left_at_unix_ms,
            ],
        )?;
    }
    for event in events {
        connection.execute(
            "INSERT INTO events(
               id, conversation_id, sequence, author_membership_id, kind,
               causation_id, correlation_id, created_at, finalized
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                event.id,
                conversation.id,
                event.sequence,
                event.author_membership_id,
                enum_wire(event.kind)?,
                event.causation_id,
                event.correlation_id,
                event.created_at_unix_ms,
                event.finalized as i64,
            ],
        )?;
        for part in &event.parts {
            connection.execute(
                "INSERT INTO event_parts(id, event_id, ordinal, kind, content, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    part.id,
                    event.id,
                    part.ordinal,
                    enum_wire(part.kind)?,
                    part.content,
                    part.created_at_unix_ms,
                ],
            )?;
            if matches!(part.kind, EventPartKind::Text | EventPartKind::Reasoning) {
                connection.execute(
                    "INSERT INTO event_search(event_id, conversation_id, content)
                     VALUES (?1, ?2, ?3)",
                    params![event.id, conversation.id, part.content],
                )?;
            }
        }
    }
    Ok(())
}

fn table_exists(connection: &impl CountedSqlite, name: &str) -> StoreResult<bool> {
    let found: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
        params![name],
        |row| row.get(0),
    )?;
    Ok(found)
}

fn archive_continuity_children(
    connection: &impl CountedSqlite,
    parent_id: &str,
    now: i64,
) -> StoreResult<Vec<String>> {
    if !table_exists(connection, "continuity_task_relations")? {
        return Ok(Vec::new());
    }
    // Descend the whole subtree: a group's involved conversations include
    // grandchildren, not just the direct children.
    let mut archived = Vec::new();
    let mut frontier = vec![parent_id.to_owned()];
    let mut seen = std::collections::HashSet::from([parent_id.to_owned()]);
    let mut statement = connection.prepare(
        "SELECT child_conversation_id FROM continuity_task_relations
         WHERE parent_conversation_id=?1
         ORDER BY card_sequence ASC, goal_id ASC",
    )?;
    while let Some(current) = frontier.pop() {
        let child_ids = statement
            .query_map(params![current], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for child_id in child_ids {
            if !seen.insert(child_id.clone()) {
                continue;
            }
            let changed = connection.execute(
                "UPDATE conversations
                 SET archived=1, revision=revision+1, updated_at=?2
                 WHERE id=?1 AND archived=0",
                params![child_id, now],
            )?;
            if changed > 0 {
                archived.push(child_id.clone());
            }
            frontier.push(child_id);
        }
    }
    Ok(archived)
}

/// Mark every native session the given Conversations used as archived, keyed
/// by (agent, native session). Marks are idempotent and attribute the archive
/// to the conversation that owns the session, so restoring one conversation
/// resurfaces exactly its own sessions again.
fn mark_native_sessions_archived(
    connection: &impl CountedSqlite,
    conversation_ids: &[String],
    now: i64,
) -> StoreResult<Vec<NativeSessionReference>> {
    let mut statement = connection.prepare(
        "SELECT s.membership_id, p.agent_id, s.native_session_id
         FROM conversation_native_sessions s
         JOIN memberships m ON m.id=s.membership_id AND m.conversation_id=s.conversation_id
         JOIN principals p ON p.id=m.principal_id
         WHERE s.conversation_id=?1 AND p.kind='agent'",
    )?;
    let mut marked = Vec::new();
    for conversation_id in conversation_ids {
        let references = statement
            .query_map(params![conversation_id], |row| {
                Ok(NativeSessionReference {
                    membership_id: row.get(0)?,
                    agent_id: row.get(1)?,
                    native_session_id: row.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for reference in references {
            connection.execute(
                "INSERT INTO archived_native_sessions(agent_id, native_session_id, conversation_id, archived_at)
                 VALUES (?1, ?2, ?3, ?4) ON CONFLICT DO NOTHING",
                params![
                    reference.agent_id,
                    reference.native_session_id,
                    conversation_id,
                    now
                ],
            )?;
            marked.push(reference);
        }
    }
    Ok(marked)
}

/// Restore is the inverse of archive for browse visibility: the archived
/// marks owned by this conversation come back out.
fn clear_native_sessions_archived(
    connection: &impl CountedSqlite,
    conversation_id: &str,
) -> StoreResult<()> {
    connection.execute(
        "DELETE FROM archived_native_sessions WHERE conversation_id=?1",
        params![conversation_id],
    )?;
    Ok(())
}

/// Refuse a destructive group write while any work is still in flight:
/// unfinalized Events, active Direct Turns, running dispatches, or claimed
/// subagent dispatches would lose their settlement records.
///
/// The states are [`admission`]'s shared vocabulary, so this per-group refusal
/// and the host-wide unfinished-work read cannot disagree about what "in
/// flight" means.
fn refuse_in_flight_group_work(
    connection: &impl CountedSqlite,
    conversation_id: &str,
) -> StoreResult<()> {
    let blocked: bool = connection.query_row(
        &format!(
            "SELECT EXISTS(
               SELECT 1 FROM events
               WHERE conversation_id=?1 AND {event_predicate}
             ) OR EXISTS(
               SELECT 1 FROM direct_turns
               WHERE conversation_id=?1
                 AND state IN ({turn_states})
             ) OR EXISTS(
               SELECT 1 FROM conversation_dispatches
               WHERE conversation_id=?1
                 AND state IN ({dispatch_states})
             )",
            event_predicate = admission::UNFINISHED_EVENT_SQL,
            turn_states = admission::UNFINISHED_DIRECT_TURN_STATES,
            dispatch_states = admission::UNFINISHED_DISPATCH_STATES,
        ),
        params![conversation_id],
        |row| row.get(0),
    )?;
    if blocked {
        return Err(anyhow!("conversation_clear_blocked"));
    }
    if table_exists(connection, "subagent_dispatch_claims")? {
        let subagent_active: bool = connection.query_row(
            &format!(
                "SELECT EXISTS(
                   SELECT 1 FROM subagent_dispatch_claims
                   WHERE conversation_id=?1
                     AND state IN ({claim_states})
                 )",
                claim_states = admission::UNFINISHED_CLAIM_STATES,
            ),
            params![conversation_id],
            |row| row.get(0),
        )?;
        if subagent_active {
            return Err(anyhow!("conversation_clear_blocked"));
        }
    }
    Ok(())
}

/// Reset the reserved default local group in place: its continuity children
/// and member native sessions are archived exactly like a group archive, the
/// visible Event history is deleted, the Assistant membership rotates with
/// its Profile preserved, and one `conversation-reset` notice reopens the
/// same Conversation — still active, pinned, and grouped. The reserved group
/// can never stay archived, so this replaces both archive and reopen for it;
/// a successor copy would survive as a permanent duplicate.
fn reset_reserved_default_group(
    connection: &impl CountedSqlite,
    conversation_id: &str,
    now: i64,
) -> StoreResult<(Vec<String>, Vec<NativeSessionReference>)> {
    refuse_in_flight_group_work(connection, conversation_id)?;
    let archived_child_ids = archive_continuity_children(connection, conversation_id, now)?;
    let mut conversation_ids = vec![conversation_id.to_owned()];
    conversation_ids.extend(archived_child_ids.iter().cloned());
    let archived_native_sessions =
        mark_native_sessions_archived(connection, &conversation_ids, now)?;
    delete_conversation_events(connection, conversation_id)?;
    rotate_assistant_membership(connection, conversation_id, now)?;
    connection.execute(
        "UPDATE conversations
         SET archived=0, pinned=1, is_group=1,
             title=CASE
               WHEN trim(title)='' OR lower(trim(title)) IN ('lico', 'lico-group-default')
               THEN ?2 ELSE title END
         WHERE id=?1",
        params![conversation_id, DEFAULT_LOCAL_AGENT_GROUP_TITLE],
    )?;
    append_domain_event(
        connection,
        conversation_id,
        EventKind::ConversationReset,
        serde_json::json!({"resetInPlace": true}),
        now,
    )?;
    bump_revision(connection, conversation_id, now)?;
    Ok((archived_child_ids, archived_native_sessions))
}

/// Create the fresh successor of an archived group: same title and strategy,
/// every active membership copied with its profile, and the assistant pointer
/// preserved. Member copies emit no events — a reopened conversation is not
/// news — and a single `conversation-reset` notice opens the successor.
fn reopen_group_conversation(
    connection: &impl CountedSqlite,
    conversation_id: &str,
    now: i64,
) -> StoreResult<String> {
    let source: Option<(String, Option<String>, Option<String>)> = connection
        .query_row(
            "SELECT title, strategy_revision, assistant_membership_id
             FROM conversations WHERE id=?1 AND is_group=1",
            params![conversation_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let Some((title, strategy_revision, assistant_membership_id)) = source else {
        return Err(anyhow!("invalid_request"));
    };
    let successor_id = new_id("conversation");
    connection.execute(
        "INSERT INTO conversations(
           id, title, archived, pinned, is_group, strategy_revision, revision, created_at, updated_at
         ) VALUES (?1, ?2, 0, 0, 1, ?3, 0, ?4, ?4)",
        params![successor_id, title, strategy_revision, now],
    )?;
    let mut membership_statement = connection.prepare(
        "SELECT id, principal_id, access FROM memberships
         WHERE conversation_id=?1 AND status='active'
         ORDER BY joined_at ASC, id ASC",
    )?;
    let memberships = membership_statement
        .query_map(params![conversation_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut successor_assistant = None;
    for (membership_id, principal_id, access) in memberships {
        let next_id = new_id("membership");
        connection.execute(
            "INSERT INTO memberships(id, conversation_id, principal_id, access, status, joined_at)
             VALUES (?1, ?2, ?3, ?4, 'active', ?5)",
            params![next_id, successor_id, principal_id, access, now],
        )?;
        connection.execute(
            "INSERT INTO membership_profiles(
               membership_id, revision, responsibility, required_capabilities,
               preferred_capabilities, skill_references, preferred_model,
               preferred_reasoning_effort, preferred_environment,
               preferred_permission_mode, preferred_instructions,
               preferred_context_window, updated_at
             )
             SELECT ?2, 0, responsibility, required_capabilities,
                    preferred_capabilities, skill_references, preferred_model,
                    preferred_reasoning_effort, preferred_environment,
                    preferred_permission_mode, preferred_instructions,
                    preferred_context_window, ?3
             FROM membership_profiles WHERE membership_id=?1",
            params![membership_id, next_id, now],
        )?;
        if assistant_membership_id.as_deref() == Some(membership_id.as_str()) {
            successor_assistant = Some(next_id);
        }
    }
    if let Some(assistant) = successor_assistant {
        connection.execute(
            "UPDATE conversations SET assistant_membership_id=?2 WHERE id=?1",
            params![successor_id, assistant],
        )?;
    }
    append_domain_event(
        connection,
        &successor_id,
        EventKind::ConversationReset,
        serde_json::json!({"reopenedFromConversationId": conversation_id}),
        now,
    )?;
    bump_revision(connection, &successor_id, now)?;
    Ok(successor_id)
}

fn rotate_assistant_membership(
    connection: &impl CountedSqlite,
    conversation_id: &str,
    now: i64,
) -> StoreResult<Option<String>> {
    let current: Option<String> = connection.query_row(
        "SELECT assistant_membership_id FROM conversations WHERE id=?1",
        params![conversation_id],
        |row| row.get(0),
    )?;
    let Some(current_id) = current.filter(|value| !value.trim().is_empty()) else {
        return Ok(None);
    };
    let principal: (String, String, String, Option<String>, String) = connection.query_row(
        "SELECT p.id, p.display_name, p.kind, p.agent_id, m.access
         FROM memberships m
         JOIN principals p ON p.id=m.principal_id
         WHERE m.id=?1 AND m.conversation_id=?2 AND m.status='active'",
        params![current_id, conversation_id],
        |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        },
    )?;
    if principal.2 != "agent" {
        return Ok(Some(current_id));
    }
    let profile: Option<(
        String,
        String,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<i64>,
    )> = connection
        .query_row(
            "SELECT required_capabilities, preferred_capabilities, skill_references,
                    preferred_model, preferred_reasoning_effort, preferred_environment,
                    preferred_permission_mode, preferred_instructions,
                    preferred_context_window
             FROM membership_profiles WHERE membership_id=?1",
            params![current_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                ))
            },
        )
        .optional()?;
    connection.execute(
        "UPDATE conversations SET assistant_membership_id=NULL WHERE id=?1",
        params![conversation_id],
    )?;
    connection.execute(
        "DELETE FROM memberships WHERE id=?1 AND conversation_id=?2",
        params![current_id, conversation_id],
    )?;
    let next_id = new_id("membership");
    connection.execute(
        "INSERT INTO memberships(id, conversation_id, principal_id, access, status, joined_at)
         VALUES (?1, ?2, ?3, ?4, 'active', ?5)",
        params![next_id, conversation_id, principal.0, principal.4, now],
    )?;
    if let Some(profile) = profile {
        connection.execute(
            "INSERT INTO membership_profiles(
               membership_id, revision, responsibility, required_capabilities,
               preferred_capabilities, skill_references, preferred_model,
               preferred_reasoning_effort, preferred_environment,
               preferred_permission_mode, preferred_instructions,
               preferred_context_window, updated_at
             ) VALUES (?1, 0, 'assistant', ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                next_id, profile.0, profile.1, profile.2, profile.3, profile.4, profile.5,
                profile.6, profile.7, profile.8, now
            ],
        )?;
    } else {
        ensure_membership_profile_default(connection, &next_id, now)?;
        set_profile_responsibility(connection, &next_id, ProfileResponsibility::Assistant, now)?;
    }
    connection.execute(
        "UPDATE conversations SET assistant_membership_id=?2 WHERE id=?1",
        params![conversation_id, next_id],
    )?;
    Ok(Some(next_id))
}

fn delete_conversation_events(
    connection: &impl CountedSqlite,
    conversation_id: &str,
) -> StoreResult<()> {
    connection.execute(
        "DELETE FROM event_search WHERE conversation_id=?1",
        params![conversation_id],
    )?;
    connection.execute(
        "DELETE FROM conversation_dispatches WHERE conversation_id=?1",
        params![conversation_id],
    )?;
    connection.execute(
        "DELETE FROM direct_turns WHERE conversation_id=?1",
        params![conversation_id],
    )?;
    if table_exists(connection, "subagent_dispatch_deliveries")? {
        connection.execute(
            "DELETE FROM subagent_dispatch_deliveries WHERE conversation_id=?1",
            params![conversation_id],
        )?;
    }
    if table_exists(connection, "subagent_dispatch_claims")? {
        connection.execute(
            "DELETE FROM subagent_dispatch_claims WHERE conversation_id=?1",
            params![conversation_id],
        )?;
    }
    if table_exists(connection, "subagent_mcp_inbound")? {
        connection.execute(
            "DELETE FROM subagent_mcp_inbound WHERE conversation_id=?1",
            params![conversation_id],
        )?;
    }
    // An admission key names the Event it authorized, so it goes with that
    // Event: a cleared Conversation does not keep a record that would refuse a
    // later admission and name a row that no longer exists.
    if table_exists(connection, "event_admission_keys")? {
        connection.execute(
            "DELETE FROM event_admission_keys WHERE conversation_id=?1",
            params![conversation_id],
        )?;
    }
    connection.execute(
        "DELETE FROM event_parts WHERE event_id IN
         (SELECT id FROM events WHERE conversation_id=?1)",
        params![conversation_id],
    )?;
    connection.execute(
        "DELETE FROM events WHERE conversation_id=?1",
        params![conversation_id],
    )?;
    if table_exists(connection, "continuity_source_cursors")? {
        connection.execute(
            "DELETE FROM continuity_source_cursors WHERE conversation_id=?1",
            params![conversation_id],
        )?;
    }
    if table_exists(connection, "continuity_matter_associations")? {
        connection.execute(
            "DELETE FROM continuity_matter_associations WHERE conversation_id=?1",
            params![conversation_id],
        )?;
    }
    Ok(())
}

fn bump_revision(
    connection: &impl CountedSqlite,
    conversation_id: &str,
    now: i64,
) -> StoreResult<()> {
    let changed = connection.execute(
        "UPDATE conversations SET revision=revision+1, updated_at=?2 WHERE id=?1",
        params![conversation_id, now],
    )?;
    if changed == 0 {
        return Err(anyhow!("conversation_not_found"));
    }
    Ok(())
}

/// Ensure one active Agent Membership has a bounded default Profile intent
/// row. Idempotent: an existing row is never overwritten.
fn ensure_membership_profile_default(
    connection: &impl CountedSqlite,
    membership_id: &str,
    now: i64,
) -> StoreResult<()> {
    connection.execute(
        "INSERT OR IGNORE INTO membership_profiles(
           membership_id, revision, responsibility, required_capabilities,
           preferred_capabilities, skill_references, preferred_model,
           preferred_reasoning_effort, preferred_environment,
           preferred_permission_mode, preferred_instructions,
           preferred_context_window, updated_at
         ) VALUES (?1, 0, 'member', '[]', '[]', '[]', NULL, NULL, NULL, NULL, NULL, NULL, ?2)",
        params![membership_id, now],
    )?;
    Ok(())
}

fn profile_intent_from_row(row: &Row<'_>) -> rusqlite::Result<ProfileIntent> {
    Ok(ProfileIntent {
        revision: row.get(0)?,
        responsibility: parse_profile_responsibility(&row.get::<_, String>(1)?)?,
        required_capabilities: serde_json::from_str(&row.get::<_, String>(2)?).unwrap_or_default(),
        preferred_capabilities: serde_json::from_str(&row.get::<_, String>(3)?).unwrap_or_default(),
        skill_references: serde_json::from_str(&row.get::<_, String>(4)?).unwrap_or_default(),
        preferred_model: row.get(5)?,
        preferred_reasoning_effort: row.get(6)?,
        preferred_environment: row.get(7)?,
        preferred_permission_mode: row.get(9)?,
        preferred_instructions: row.get(10)?,
        preferred_context_window: row
            .get::<_, Option<i64>>(11)?
            .and_then(|value| u64::try_from(value).ok()),
        updated_at_unix_ms: row.get(8)?,
    })
}

fn parse_profile_responsibility(value: &str) -> rusqlite::Result<ProfileResponsibility> {
    match value {
        "assistant" => Ok(ProfileResponsibility::Assistant),
        "member" => Ok(ProfileResponsibility::Member),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

fn ensure_local_owner(
    connection: &impl CountedSqlite,
    conversation_id: &str,
    membership_id: &str,
) -> StoreResult<()> {
    let valid: bool = connection.query_row(
        "SELECT EXISTS(
           SELECT 1 FROM memberships m
           JOIN principals p ON p.id=m.principal_id
           WHERE m.id=?1 AND m.conversation_id=?2 AND m.status='active'
             AND m.access='owner' AND p.kind='human'
         )",
        params![membership_id, conversation_id],
        |row| row.get(0),
    )?;
    if !valid {
        return Err(anyhow!("local_owner_required"));
    }
    Ok(())
}

fn normalized_skill_references(
    references: &[String],
    responsibility: ProfileResponsibility,
) -> Vec<String> {
    let mut normalized = references
        .iter()
        .filter(|reference| reference.as_str() != LICOUP_GUIDE_SKILL_ID)
        .cloned()
        .collect::<Vec<_>>();
    if responsibility == ProfileResponsibility::Assistant {
        normalized.push(LICOUP_GUIDE_SKILL_ID.to_owned());
    }
    normalized.sort();
    normalized.dedup();
    normalized
}

fn set_profile_responsibility(
    connection: &impl CountedSqlite,
    membership_id: &str,
    responsibility: ProfileResponsibility,
    now: i64,
) -> StoreResult<()> {
    let current = connection.query_row(
        "SELECT revision, responsibility, required_capabilities,
         preferred_capabilities, skill_references, preferred_model,
         preferred_reasoning_effort, preferred_environment, updated_at,
         preferred_permission_mode, preferred_instructions,
         preferred_context_window
         FROM membership_profiles WHERE membership_id=?1",
        params![membership_id],
        profile_intent_from_row,
    )?;
    let skill_references = normalized_skill_references(&current.skill_references, responsibility);
    if current.responsibility == responsibility && current.skill_references == skill_references {
        return Ok(());
    }
    connection.execute(
        "UPDATE membership_profiles
         SET revision=revision+1, responsibility=?2, skill_references=?3, updated_at=?4
         WHERE membership_id=?1",
        params![
            membership_id,
            match responsibility {
                ProfileResponsibility::Assistant => "assistant",
                ProfileResponsibility::Member => "member",
            },
            serde_json::to_string(&skill_references)?,
            now,
        ],
    )?;
    Ok(())
}

fn validate_identifier(value: &str, label: &str) -> StoreResult<()> {
    if value.is_empty()
        || value.len() > 160
        || value
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
    {
        return Err(anyhow!("{label}_invalid"));
    }
    Ok(())
}

fn validate_text(value: &str, label: &str) -> StoreResult<()> {
    if value.len() > 1_048_576 || value.chars().any(|character| character == '\0') {
        return Err(anyhow!("{label}_invalid"));
    }
    Ok(())
}

fn validate_required_text(value: &str, label: &str) -> StoreResult<()> {
    validate_text(value, label)?;
    if value.trim().is_empty() {
        return Err(anyhow!("{label}_invalid"));
    }
    Ok(())
}

fn new_id(prefix: &str) -> String {
    format!("{prefix}:{}", Uuid::new_v4())
}

fn runtime_source_identity(
    agent_id: &str,
    session_id: &str,
    dispatch_id: &str,
) -> StoreResult<String> {
    let identity = if session_id.trim().is_empty() {
        format!("pending:{dispatch_id}")
    } else {
        format!("{}:{agent_id}:{session_id}", agent_id.len())
    };
    validate_identifier(&identity, "runtime_source_identity")?;
    Ok(identity)
}

fn runtime_frame_is_user_speech(frame: &Value) -> bool {
    frame.get("event").and_then(Value::as_str) == Some(crate::projection::USER_MESSAGE_EVENT_KIND)
}

fn load_trusted_response_mode(
    connection: &impl CountedSqlite,
    event_id: &str,
) -> StoreResult<Option<String>> {
    let mut statement = connection.prepare(
        "SELECT content FROM event_parts WHERE event_id=?1 AND kind='metadata' AND runtime_cursor IS NULL ORDER BY ordinal",
    )?;
    let rows = statement.query_map(params![event_id], |row| row.get::<_, String>(0))?;
    for row in rows {
        let content = row?;
        if let Some(mode) = crate::continuity::trusted_response_mode_from_metadata(&content) {
            return Ok(Some(mode));
        }
    }
    Ok(None)
}

fn runtime_frame_parts(encoded: &str) -> Vec<NewEventPart> {
    const CHUNK_BYTES: usize = 512 * 1024;
    if encoded.is_empty() {
        return vec![NewEventPart {
            id: String::new(),
            kind: EventPartKind::Metadata,
            content: String::new(),
        }];
    }
    let mut parts = Vec::with_capacity(encoded.len().div_ceil(CHUNK_BYTES));
    let mut start = 0;
    while start < encoded.len() {
        let mut end = (start + CHUNK_BYTES).min(encoded.len());
        while end > start && !encoded.is_char_boundary(end) {
            end -= 1;
        }
        parts.push(NewEventPart {
            id: String::new(),
            kind: EventPartKind::Metadata,
            content: encoded[start..end].to_owned(),
        });
        start = end;
    }
    parts
}

fn runtime_semantic_parts(frame: &Value, trusted_mode: Option<&str>) -> Vec<NewEventPart> {
    let event = frame
        .get("event")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let payload = frame.get("payload").unwrap_or(&Value::Null);
    let text = payload
        .get("text")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty());
    let mut parts = payload
        .get("lifecyclePrefix")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|stage| {
            matches!(
                *stage,
                "submitted" | "accepted" | "processing" | "responding" | "completed"
            )
        })
        .map(|stage| {
            (
                EventPartKind::Metadata,
                serde_json::json!({"lifecycle": stage}).to_string(),
            )
        })
        .collect::<Vec<_>>();
    let semantic_parts = match event {
        "agent.turn.accepted" => Vec::new(),
        "agent.message.chunk" | "agent.message.completed" => {
            let mut parts = Vec::new();
            if let Some(value) = text {
                if crate::continuity::is_assistant_turn_response_mode(trusted_mode) {
                    if event == "agent.message.completed"
                        && let Some((reply, proposal)) =
                            crate::continuity::published_terminal_envelope(value)
                    {
                        parts.push((EventPartKind::Text, reply));
                        parts.push((EventPartKind::Metadata, proposal));
                    }
                } else {
                    parts.push((EventPartKind::Text, value.to_owned()));
                }
            }
            parts
        }
        "agent.turn.processing" => {
            let evidence = payload
                .get("evidenceKind")
                .and_then(Value::as_str)
                .unwrap_or("activity");
            match evidence {
                "reasoning" | "plan" => vec![(
                    EventPartKind::Reasoning,
                    text.unwrap_or(evidence).to_owned(),
                )],
                "tool" => vec![(
                    EventPartKind::ToolCall,
                    payload
                        .get("toolName")
                        .and_then(Value::as_str)
                        .filter(|value| !value.is_empty())
                        .or(text)
                        .unwrap_or("tool")
                        .to_owned(),
                )],
                _ => Vec::new(),
            }
        }
        "dispatch.turn.failed" | "agent.turn.failed" => Vec::new(),
        "permission.denied" => vec![(
            EventPartKind::Diagnostic,
            serde_json::json!({
                "code": "permission_denied",
                "stage": "permission",
            })
            .to_string(),
        )],
        "agent.tool.result" => vec![(
            EventPartKind::ToolResult,
            text.unwrap_or("tool result").to_owned(),
        )],
        _ if event.contains("tool") && event.contains("result") => vec![(
            EventPartKind::ToolResult,
            text.unwrap_or("tool result").to_owned(),
        )],
        _ if event.contains("tool") => vec![(
            EventPartKind::ToolCall,
            payload
                .get("toolName")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .or(text)
                .unwrap_or("tool")
                .to_owned(),
        )],
        _ if event.contains("artifact") => vec![(
            EventPartKind::Artifact,
            text.unwrap_or("artifact").to_owned(),
        )],
        _ if event.contains("diagnostic") || event.contains("failed") => vec![(
            EventPartKind::Diagnostic,
            runtime_terminal_diagnostic(payload, Some("runtime_failed")),
        )],
        _ => Vec::new(),
    };
    if matches!(event, "agent.message.chunk" | "agent.message.completed")
        && let Some(message_unit) = payload
            .get("messageUnit")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
    {
        parts.push((
            EventPartKind::Metadata,
            serde_json::json!({"messageUnit": message_unit}).to_string(),
        ));
    }
    parts.extend(semantic_parts);
    parts
        .into_iter()
        .map(|(kind, content)| NewEventPart {
            id: String::new(),
            kind,
            content,
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn insert_runtime_event_part(
    connection: &impl CountedSqlite,
    conversation_id: &str,
    event_id: &str,
    ordinal: i64,
    part: &NewEventPart,
    runtime_cursor: Option<i64>,
    now: i64,
) -> StoreResult<()> {
    if runtime_cursor == Some(0) {
        return Err(anyhow!("runtime_cursor_invalid"));
    }
    if part.kind != EventPartKind::Text {
        validate_text(&part.content, "event_part_content")?;
    }
    let part_id = if part.id.trim().is_empty() {
        new_id("part")
    } else {
        part.id.clone()
    };
    connection.execute(
        "INSERT INTO event_parts(
           id, event_id, ordinal, kind, content, runtime_cursor, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            part_id,
            event_id,
            ordinal,
            enum_wire(part.kind)?,
            part.content,
            runtime_cursor,
            now,
        ],
    )?;
    if matches!(part.kind, EventPartKind::Text | EventPartKind::Reasoning) {
        connection.execute(
            "INSERT INTO event_search(event_id, conversation_id, content) VALUES (?1, ?2, ?3)",
            params![event_id, conversation_id, part.content],
        )?;
    }
    Ok(())
}

fn runtime_terminal_diagnostic(terminal: &Value, error_code: Option<&str>) -> String {
    let nested = terminal.get("error").unwrap_or(terminal);
    let code = error_code
        .or_else(|| nested.get("code").and_then(Value::as_str))
        .unwrap_or("agent_conversation_dispatch_failed");
    let stage = nested
        .get("stage")
        .and_then(Value::as_str)
        .unwrap_or("conversation/dispatch");
    let turn_status = nested
        .get("turnStatus")
        .or_else(|| terminal.get("turnStatus"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let mut diagnostic = serde_json::Map::new();
    diagnostic.insert("code".into(), serde_json::Value::String(code.to_owned()));
    diagnostic.insert("stage".into(), serde_json::Value::String(stage.to_owned()));
    if let Some(turn_status) = turn_status {
        diagnostic.insert(
            "turnStatus".into(),
            serde_json::Value::String(turn_status.to_owned()),
        );
    }
    if let Some(component) = nested
        .get("component")
        .and_then(Value::as_str)
        .filter(|value| {
            matches!(
                *value,
                "conversation_runtime"
                    | "native_cli"
                    | "runtime_adapter"
                    | "runtime_process"
                    | "stdio_rpc"
            )
        })
    {
        diagnostic.insert(
            "component".into(),
            serde_json::Value::String(component.to_owned()),
        );
    }
    if let Some(retryable) = nested.get("retryable").and_then(Value::as_bool) {
        diagnostic.insert("retryable".into(), serde_json::Value::Bool(retryable));
    }
    if let Some(root_cause) = nested
        .get("rootCause")
        .and_then(Value::as_str)
        .filter(|value| {
            matches!(
                *value,
                "auth" | "network_unreachable" | "env_mismatch" | "quota" | "unknown"
            )
        })
    {
        diagnostic.insert(
            "rootCause".into(),
            serde_json::Value::String(root_cause.to_owned()),
        );
    }
    if let Some(recovery) = nested
        .get("recovery")
        .and_then(Value::as_str)
        .filter(|value| {
            matches!(
                *value,
                "review_terminal_result"
                    | "preserve_draft_and_retry"
                    | "select_available_model_or_wait_for_quota_reset"
                    | "reauthenticate_provider_and_retry"
                    | "restore_network_reachability_and_retry"
                    | "subagent_env_mismatch: LicoUp-launched CLI environment differs from user terminal"
            )
        })
    {
        diagnostic.insert(
            "recovery".into(),
            serde_json::Value::String(recovery.to_owned()),
        );
    }
    serde_json::Value::Object(diagnostic).to_string()
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

fn enum_wire<T: Serialize>(value: T) -> StoreResult<String> {
    Ok(serde_json::to_value(value)?
        .as_str()
        .unwrap_or_default()
        .to_owned())
}
fn decode<T: DeserializeOwned>(value: String) -> rusqlite::Result<T> {
    serde_json::from_str(&format!("\"{value}\"")).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
    })
}

fn summary_from_row(row: &Row<'_>) -> rusqlite::Result<ConversationSummary> {
    Ok(ConversationSummary {
        id: row.get(0)?,
        title: row.get(1)?,
        archived: row.get::<_, i64>(2)? != 0,
        pinned: row.get::<_, i64>(3)? != 0,
        is_group: row.get::<_, i64>(4)? != 0,
        revision: row.get(5)?,
        updated_at_unix_ms: row.get(6)?,
        membership_count: row.get(7)?,
        event_count: row.get(8)?,
        member_agent_ids: row
            .get::<_, Option<String>>(9)?
            .map(|joined| {
                joined
                    .split('\u{1f}')
                    .filter(|id| !id.trim().is_empty())
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
        has_local_owner: row.get::<_, i64>(10)? != 0,
    })
}
fn principal_from_row(row: &Row<'_>, offset: usize) -> rusqlite::Result<Principal> {
    Ok(Principal {
        id: row.get(offset)?,
        kind: decode(row.get(offset + 1)?)?,
        display_name: row.get(offset + 2)?,
        agent_id: row.get(offset + 3)?,
        created_at_unix_ms: row.get(offset + 4)?,
    })
}
fn membership_from_row(row: &Row<'_>) -> rusqlite::Result<Membership> {
    Ok(Membership {
        id: row.get(0)?,
        conversation_id: row.get(1)?,
        principal: principal_from_row(row, 2)?,
        access: decode(row.get(7)?)?,
        status: decode(row.get(8)?)?,
        joined_at_unix_ms: row.get(9)?,
        left_at_unix_ms: row.get(10)?,
    })
}
fn event_from_row(row: &Row<'_>) -> rusqlite::Result<ConversationEvent> {
    Ok(ConversationEvent {
        id: row.get(0)?,
        conversation_id: row.get(1)?,
        sequence: row.get(2)?,
        author_membership_id: row.get(3)?,
        kind: decode(row.get(4)?)?,
        causation_id: row.get(5)?,
        correlation_id: row.get(6)?,
        created_at_unix_ms: row.get(7)?,
        finalized: row.get::<_, i64>(8)? != 0,
        parts: Vec::new(),
    })
}
fn direct_turn_from_row(row: &Row<'_>) -> rusqlite::Result<DirectTurn> {
    Ok(DirectTurn {
        id: row.get(0)?,
        conversation_id: row.get(1)?,
        source_event_id: row.get(2)?,
        membership_id: row.get(3)?,
        state: decode(row.get(4)?)?,
        ordinal: row.get(5)?,
    })
}
fn part_from_row(row: &Row<'_>) -> rusqlite::Result<EventPart> {
    Ok(EventPart {
        id: row.get(0)?,
        event_id: row.get(1)?,
        ordinal: row.get(2)?,
        kind: decode(row.get(3)?)?,
        content: row.get(4)?,
        created_at_unix_ms: row.get(5)?,
    })
}
fn source_link_from_row(row: &Row<'_>) -> rusqlite::Result<SourceLink> {
    Ok(SourceLink {
        id: row.get(0)?,
        conversation_id: row.get(1)?,
        source_kind: row.get(2)?,
        native_identity: row.get(3)?,
    })
}

fn dispatch_by_id(
    connection: &impl CountedSqlite,
    dispatch_id: &str,
) -> StoreResult<ConversationDispatch> {
    connection
        .query_row(
            "SELECT id, conversation_id, membership_id, operation, state, session_mode,
               runtime_conversation_path, error_code, created_at, updated_at
             FROM conversation_dispatches WHERE id=?1",
            params![dispatch_id],
            dispatch_from_row,
        )
        .map_err(Into::into)
}

fn dispatch_from_row(row: &Row<'_>) -> rusqlite::Result<ConversationDispatch> {
    Ok(ConversationDispatch {
        id: row.get(0)?,
        conversation_id: row.get(1)?,
        membership_id: row.get(2)?,
        operation: row.get(3)?,
        state: decode(row.get(4)?)?,
        session_mode: decode(row.get(5)?)?,
        runtime_conversation_path: row.get(6)?,
        error_code: row.get(7)?,
        created_at_unix_ms: row.get(8)?,
        updated_at_unix_ms: row.get(9)?,
    })
}

fn append_domain_event(
    connection: &impl CountedSqlite,
    conversation_id: &str,
    kind: EventKind,
    metadata: serde_json::Value,
    now: i64,
) -> StoreResult<()> {
    let sequence: i64 = connection.query_row(
        "SELECT COALESCE(MAX(sequence), 0)+1 FROM events WHERE conversation_id=?1",
        params![conversation_id],
        |row| row.get(0),
    )?;
    let event_id = new_id("event");
    connection.execute(
        "INSERT INTO events(id, conversation_id, sequence, kind, created_at, finalized)
         VALUES (?1, ?2, ?3, ?4, ?5, 1)",
        params![event_id, conversation_id, sequence, enum_wire(kind)?, now],
    )?;
    connection.execute(
        "INSERT INTO event_parts(id, event_id, ordinal, kind, content, created_at)
         VALUES (?1, ?2, 0, ?3, ?4, ?5)",
        params![
            new_id("part"),
            event_id,
            enum_wire(EventPartKind::Metadata)?,
            serde_json::to_string(&metadata)?,
            now,
        ],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client_conversation::{EventPartKind, MembershipStatus, PrincipalKind};

    // Shared data includes domains this crate does not own. Only the independent
    // Conversation layout is used here; the native tests exercise the full root.
    #[allow(dead_code)]
    mod released_source {
        include!("../../../../tests/fixtures/client_state_migration/released_source.rs");
        include!("../../../../tests/fixtures/client_state_migration/owner_layouts.rs");
    }

    fn owner() -> Principal {
        Principal {
            id: "human:local".into(),
            kind: PrincipalKind::Human,
            display_name: "You".into(),
            agent_id: None,
            created_at_unix_ms: 1,
        }
    }

    #[test]
    fn checkpoint_flushes_an_open_store() {
        let store = ConversationStore::open_in_memory().unwrap();
        store.ensure_default_local_group().unwrap();
        store.checkpoint().unwrap();
        assert!(
            store
                .list(false)
                .unwrap()
                .iter()
                .any(|conversation| conversation.is_group)
        );
    }

    fn agent_principal(agent_id: &str) -> Principal {
        Principal {
            id: format!("agent:{agent_id}"),
            kind: PrincipalKind::Agent,
            display_name: agent_id.into(),
            agent_id: Some(agent_id.into()),
            created_at_unix_ms: 1,
        }
    }

    fn membership_of(conversation: &Conversation, kind: PrincipalKind) -> Membership {
        conversation
            .memberships
            .iter()
            .find(|membership| membership.principal.kind == kind)
            .unwrap()
            .clone()
    }

    #[test]
    fn archive_tree_cascades_marks_native_sessions_and_reopens_silently() {
        let root = std::env::temp_dir().join(format!("licoup-archive-tree-{}", Uuid::new_v4()));
        path_security::ensure_private_dir(&root).unwrap();
        let store = ConversationStore::open(&root).unwrap();

        let group = store
            .create_conversation_with_members(
                "Local",
                owner(),
                &[(agent_principal("antigravity"), MembershipAccess::Member)],
            )
            .unwrap();
        let group_agent = membership_of(&group, PrincipalKind::Agent);
        let group_owner = membership_of(&group, PrincipalKind::Human);
        store
            .set_conversation_assistant(
                &group.id,
                &group_owner.id,
                group.revision,
                Some(&group_agent.id),
            )
            .unwrap();
        let group_scope = store
            .prepare_runtime_dispatch(
                "antigravity",
                "native-group",
                "Synthetic request",
                Some(&group.id),
                Some(&group_agent.id),
                None,
                None,
            )
            .unwrap();
        store
            .bind_runtime_session(&group_scope, "antigravity", "native-group", None, None)
            .unwrap();

        let child = store
            .create_conversation_with_members(
                "child",
                owner(),
                &[(agent_principal("antigravity"), MembershipAccess::Member)],
            )
            .unwrap();
        let child_agent = membership_of(&child, PrincipalKind::Agent);
        let child_scope = store
            .prepare_runtime_dispatch(
                "antigravity",
                "native-child",
                "Synthetic request",
                Some(&child.id),
                Some(&child_agent.id),
                None,
                None,
            )
            .unwrap();
        store
            .bind_runtime_session(&child_scope, "antigravity", "native-child", None, None)
            .unwrap();

        let untouched = store
            .create_conversation_with_members(
                "unrelated",
                owner(),
                &[(agent_principal("antigravity"), MembershipAccess::Member)],
            )
            .unwrap();
        let untouched_agent = membership_of(&untouched, PrincipalKind::Agent);
        let untouched_scope = store
            .prepare_runtime_dispatch(
                "antigravity",
                "native-untouched",
                "Synthetic request",
                Some(&untouched.id),
                Some(&untouched_agent.id),
                None,
                None,
            )
            .unwrap();
        store
            .bind_runtime_session(
                &untouched_scope,
                "antigravity",
                "native-untouched",
                None,
                None,
            )
            .unwrap();

        let report = store
            .archive_conversation_tree(&group.id, true, true)
            .unwrap();
        assert_eq!(report.conversation_id, group.id);
        assert!(report.archived_child_ids.is_empty());
        let marked: Vec<&str> = report
            .archived_native_sessions
            .iter()
            .map(|reference| reference.native_session_id.as_str())
            .collect();
        assert_eq!(marked, ["native-group"]);
        assert!(store.get(&group.id).unwrap().archived);
        assert!(!store.get(&child.id).unwrap().archived);
        assert!(!store.get(&untouched.id).unwrap().archived);

        // The successor reopens the group silently: same title and members,
        // assistant preserved, exactly one plain reset notice, and no member
        // joined/left churn.
        let successor_id = report.successor_conversation_id.unwrap();
        let successor = store.get(&successor_id).unwrap();
        assert_eq!(successor.title, "Local");
        assert!(successor.is_group);
        assert!(!successor.archived);
        let successor_agent = membership_of(&successor, PrincipalKind::Agent);
        assert_eq!(
            successor_agent.principal.agent_id.as_deref(),
            Some("antigravity")
        );
        assert_eq!(
            successor.assistant_membership_id.as_deref(),
            Some(successor_agent.id.as_str())
        );
        let events = store.event_page(&successor_id, None, 16).unwrap().events;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, EventKind::ConversationReset);
        assert!(
            !events
                .iter()
                .any(|event| event.kind == EventKind::MembershipChanged)
        );

        // Archived native sessions are readable for catalog filtering; the
        // unrelated session is not marked.
        store.checkpoint().unwrap();
        let marked_ids =
            ConversationStore::archived_native_session_ids(&root, "antigravity").unwrap();
        assert_eq!(
            marked_ids.iter().map(String::as_str).collect::<Vec<_>>(),
            ["native-group"]
        );

        // Restore is the inverse for browse visibility: the marks owned by the
        // restored conversation come back out.
        store
            .archive_conversation_tree(&group.id, false, false)
            .unwrap();
        assert!(!store.get(&group.id).unwrap().archived);
        store.checkpoint().unwrap();
        assert!(
            ConversationStore::archived_native_session_ids(&root, "antigravity")
                .unwrap()
                .is_empty()
        );

        // Reopen only applies to groups, and never to a restore.
        let direct = store.create_conversation("direct", owner()).unwrap();
        assert!(
            store
                .archive_conversation_tree(&direct.id, true, true)
                .is_err()
        );
        assert!(
            store
                .archive_conversation_tree(&group.id, false, true)
                .is_err()
        );

        drop(store);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn archiving_the_default_local_group_resets_it_in_place() {
        let root = std::env::temp_dir().join(format!("licoup-reset-default-{}", Uuid::new_v4()));
        path_security::ensure_private_dir(&root).unwrap();
        let store = ConversationStore::open(&root).unwrap();
        let local = store.ensure_default_local_group().unwrap();
        assert_eq!(local.id, DEFAULT_LOCAL_AGENT_GROUP_ID);
        let owner_membership = membership_of(&local, PrincipalKind::Human);
        store
            .post_message_with_mentions(
                &local.id,
                Some(&owner_membership.id),
                "remember this",
                None,
                &[],
            )
            .unwrap();
        let report = store
            .archive_conversation_tree(&local.id, true, true)
            .unwrap();

        assert!(report.reset_in_place);
        assert!(report.successor_conversation_id.is_none());
        let reset = store.get(&local.id).unwrap();
        assert!(!reset.archived);
        assert!(reset.pinned);
        assert!(reset.is_group);
        // The visible history is cleared and one plain reset notice reopens
        // the same reserved Conversation.
        let events = store.event_page(&local.id, None, 16).unwrap().events;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, EventKind::ConversationReset);
        // The active list still holds exactly one default group: itself.
        assert_eq!(
            store
                .list(false)
                .unwrap()
                .iter()
                .filter(|conversation| conversation.id == DEFAULT_LOCAL_AGENT_GROUP_ID)
                .count(),
            1
        );

        // A plain archive request resets the reserved group the same way.
        store
            .post_message_with_mentions(&local.id, Some(&owner_membership.id), "again", None, &[])
            .unwrap();
        let second = store
            .archive_conversation_tree(&local.id, true, false)
            .unwrap();
        assert!(second.reset_in_place);
        assert!(second.successor_conversation_id.is_none());
        let events = store.event_page(&local.id, None, 16).unwrap().events;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, EventKind::ConversationReset);
        assert!(!store.get(&local.id).unwrap().archived);

        drop(store);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn conversation_store_commits_with_full_sync() {
        let store = ConversationStore::open_in_memory().unwrap();
        store
            .with_connection(|connection| {
                let synchronous: i64 =
                    connection.query_row("PRAGMA synchronous", [], |row| row.get(0))?;
                assert_eq!(synchronous, 2);
                let journal: String =
                    connection.query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
                assert_eq!(journal.to_ascii_lowercase(), "wal");
                #[cfg(target_os = "macos")]
                {
                    let fullfsync: i64 =
                        connection.query_row("PRAGMA fullfsync", [], |row| row.get(0))?;
                    assert_eq!(fullfsync, 1);
                    let checkpoint_fullfsync: i64 =
                        connection
                            .query_row("PRAGMA checkpoint_fullfsync", [], |row| row.get(0))?;
                    assert_eq!(checkpoint_fullfsync, 1);
                }
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn reopening_rebuilds_a_missing_search_index_from_event_parts() {
        let root = std::env::temp_dir().join(format!("lico-search-rebuild-{}", Uuid::new_v4()));
        path_security::ensure_private_dir(&root).unwrap();
        let store = ConversationStore::open(&root).unwrap();
        let conversation = store.create_conversation("Search", owner()).unwrap();
        let agent = store
            .add_member(
                &conversation.id,
                Principal {
                    id: "agent:search".into(),
                    kind: PrincipalKind::Agent,
                    display_name: "Search Agent".into(),
                    agent_id: Some("search".into()),
                    created_at_unix_ms: 1,
                },
                MembershipAccess::Member,
            )
            .unwrap();
        store
            .append_event(
                &conversation.id,
                Some(&agent.id),
                EventKind::Message,
                &[NewEventPart {
                    id: String::new(),
                    kind: EventPartKind::Text,
                    content: "durable search token".into(),
                }],
                None,
                None,
                true,
            )
            .unwrap();
        store
            .with_connection(|connection| {
                connection.execute_batch(
                    "DROP TABLE event_search;
                     CREATE VIRTUAL TABLE event_search USING fts5(
                       event_id UNINDEXED, conversation_id UNINDEXED, content
                     );",
                )?;
                Ok(())
            })
            .unwrap();
        drop(store);

        let reopened = ConversationStore::open(&root).unwrap();
        assert_eq!(
            reopened.search("durable search token", 10).unwrap().len(),
            1
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn creates_peer_conversation_and_pages_structured_events() {
        let store = ConversationStore::open_in_memory().unwrap();
        let conversation = store.create_conversation("Project", owner()).unwrap();
        assert!(!conversation.is_group);
        let agent = Principal {
            id: "agent:one".into(),
            kind: PrincipalKind::Agent,
            display_name: "One".into(),
            agent_id: Some("one".into()),
            created_at_unix_ms: 1,
        };
        let member = store
            .add_member(&conversation.id, agent, MembershipAccess::Member)
            .unwrap();
        let event = store
            .append_event(
                &conversation.id,
                Some(&member.id),
                EventKind::Message,
                &[NewEventPart {
                    id: String::new(),
                    kind: EventPartKind::Text,
                    content: "hello".into(),
                }],
                None,
                None,
                true,
            )
            .unwrap();
        assert_eq!(event.sequence, 2);
        assert_eq!(
            store
                .page_events(&conversation.id, None, 50)
                .unwrap()
                .events
                .into_iter()
                .find(|event| event.kind == EventKind::Message)
                .unwrap()
                .parts[0]
                .content,
            "hello"
        );
        assert_eq!(store.get(&conversation.id).unwrap().memberships.len(), 2);
    }

    /// AC-013: one posted image attachment persists as an image Event Part
    /// with its attachment metadata, survives cold recovery, and the
    /// serialized Event exposes the attachment to the Dart projection.
    #[test]
    fn image_part_persists_and_projects_attachment_metadata() {
        let root = std::env::temp_dir().join(format!("lico-image-part-{}", Uuid::new_v4()));
        path_security::ensure_private_dir(&root).unwrap();
        let attachment = ImageAttachment {
            path: "fixtures/mockup.png".into(),
            name: "mockup.png".into(),
            media_type: "image/png".into(),
            byte_size: 12,
        };
        let (event_id, conversation_id) = {
            let store = ConversationStore::open(&root).unwrap();
            let conversation = store.create_conversation("Images", owner()).unwrap();
            let (event, turns) = store
                .post_message_with_attachments(
                    &conversation.id,
                    None,
                    "see the mockup",
                    None,
                    &[],
                    std::slice::from_ref(&attachment),
                )
                .unwrap();
            assert!(turns.is_empty());
            assert_eq!(event.parts.len(), 2);
            assert_eq!(event.parts[0].kind, EventPartKind::Text);
            let image = &event.parts[1];
            assert_eq!(image.kind, EventPartKind::Image);
            assert_eq!(image.image_attachment().as_ref(), Some(&attachment));
            store.checkpoint().unwrap();
            (event.id, conversation.id)
        };

        // Cold recovery: a reopened store reads the same image part back.
        let reopened = ConversationStore::open(&root).unwrap();
        let page = reopened.page_events(&conversation_id, None, 10).unwrap();
        let stored = page
            .events
            .iter()
            .find(|event| event.id == event_id)
            .unwrap();
        assert_eq!(stored.parts.len(), 2);
        let stored_image = stored
            .parts
            .iter()
            .find(|part| part.kind == EventPartKind::Image)
            .unwrap();
        assert_eq!(stored_image.image_attachment().as_ref(), Some(&attachment));

        // The projected message exposes the attachment to Dart: the part kind
        // is "image" and its content carries the attachment metadata JSON.
        let projected = serde_json::to_value(stored).unwrap();
        let parts = projected["parts"].as_array().unwrap();
        let image_json = parts
            .iter()
            .find(|part| part["kind"] == serde_json::json!("image"))
            .unwrap();
        let content: serde_json::Value =
            serde_json::from_str(image_json["content"].as_str().unwrap()).unwrap();
        assert_eq!(content["path"], "fixtures/mockup.png");
        assert_eq!(content["name"], "mockup.png");
        assert_eq!(content["mediaType"], "image/png");
        assert_eq!(content["byteSize"], 12);
        let _ = std::fs::remove_dir_all(root);
    }

    /// Image parts are not searchable text: the FTS index must keep indexing
    /// only text and reasoning content so attachment metadata never leaks
    /// into search results.
    #[test]
    fn image_part_content_stays_out_of_search() {
        let store = ConversationStore::open_in_memory().unwrap();
        let conversation = store.create_conversation("Images", owner()).unwrap();
        store
            .post_message_with_attachments(
                &conversation.id,
                None,
                "ordinary words",
                None,
                &[],
                &[ImageAttachment {
                    path: "fixtures/lico-unique-token.png".into(),
                    name: "lico-unique-token.png".into(),
                    media_type: "image/png".into(),
                    byte_size: 1,
                }],
            )
            .unwrap();
        assert_eq!(store.search("ordinary words", 10).unwrap().len(), 1);
        assert!(store.search("lico-unique-token", 10).unwrap().is_empty());
    }

    /// An image-only post omits the text part, and its stored text reads back
    /// empty so dispatch addressing still runs for the attachments.
    #[test]
    fn image_only_post_stores_no_text_part_and_reads_back_empty() {
        let store = ConversationStore::open_in_memory().unwrap();
        let conversation = store.create_conversation("Images", owner()).unwrap();
        let attachment = ImageAttachment {
            path: "fixtures/only.png".into(),
            name: "only.png".into(),
            media_type: "image/png".into(),
            byte_size: 1,
        };
        let (event, _) = store
            .post_message_with_attachments(
                &conversation.id,
                None,
                "",
                None,
                &[],
                std::slice::from_ref(&attachment),
            )
            .unwrap();
        assert_eq!(event.parts.len(), 1);
        assert_eq!(event.parts[0].kind, EventPartKind::Image);
        assert_eq!(
            store
                .posted_event_text(&conversation.id, &event.id)
                .unwrap(),
            ""
        );
        assert_eq!(
            store
                .posted_event_part_text(&conversation.id, &event.id, &event.parts[0].id)
                .unwrap(),
            ""
        );
    }

    #[test]
    fn posted_event_part_text_reads_only_the_named_part() {
        let store = ConversationStore::open_in_memory().unwrap();
        let conversation = store.create_conversation("Parts", owner()).unwrap();
        let owner = conversation
            .memberships
            .iter()
            .find(|membership| membership.access == MembershipAccess::Owner)
            .unwrap()
            .id
            .clone();
        let event = store
            .append_event(
                &conversation.id,
                Some(&owner),
                EventKind::Message,
                &[
                    NewEventPart {
                        id: String::new(),
                        kind: EventPartKind::Text,
                        content: "FIRST-PART-ONLY".into(),
                    },
                    NewEventPart {
                        id: String::new(),
                        kind: EventPartKind::Text,
                        content: "SIBLING-PART-SECRET".into(),
                    },
                ],
                None,
                None,
                true,
            )
            .unwrap();
        assert_eq!(
            store
                .posted_event_text(&conversation.id, &event.id)
                .unwrap(),
            "FIRST-PART-ONLY"
        );
        assert_eq!(
            store
                .posted_event_part_text(&conversation.id, &event.id, &event.parts[0].id)
                .unwrap(),
            "FIRST-PART-ONLY"
        );
        assert_eq!(
            store
                .posted_event_part_text(&conversation.id, &event.id, &event.parts[1].id)
                .unwrap(),
            "SIBLING-PART-SECRET"
        );
        assert!(
            store
                .posted_event_part_text(&conversation.id, &event.id, "part:missing")
                .is_err()
        );
    }

    /// Without attachments the message text stays required.
    #[test]
    fn post_without_attachments_still_requires_text() {
        let store = ConversationStore::open_in_memory().unwrap();
        let conversation = store.create_conversation("Images", owner()).unwrap();
        for content in ["", "   "] {
            let error = store
                .post_message_with_attachments(&conversation.id, None, content, None, &[], &[])
                .expect_err("text-only posts must keep the required-text rule");
            assert_eq!(error.to_string(), "message_content_invalid");
        }
    }

    #[test]
    fn pooled_connections_are_bounded_and_reused_across_operations() {
        let store = ConversationStore::open_in_memory().unwrap();
        let conversation = store.create_conversation("Pool", owner()).unwrap();
        let agent = Principal {
            id: "agent:one".into(),
            kind: PrincipalKind::Agent,
            display_name: "One".into(),
            agent_id: Some("one".into()),
            created_at_unix_ms: 1,
        };
        let member = store
            .add_member(&conversation.id, agent, MembershipAccess::Member)
            .unwrap();
        for index in 0..3 {
            store
                .append_event(
                    &conversation.id,
                    Some(&member.id),
                    EventKind::Message,
                    &[NewEventPart {
                        id: String::new(),
                        kind: EventPartKind::Text,
                        content: format!("seed {index}"),
                    }],
                    None,
                    None,
                    true,
                )
                .unwrap();
        }
        let opens_after_warmup = store.counters().opens();
        assert!(
            opens_after_warmup <= DEFAULT_CONVERSATION_POOL_SIZE,
            "pool opened {opens_after_warmup} connections"
        );
        for _ in 0..5 {
            let _ = store.list(false).unwrap();
            let _ = store.get(&conversation.id).unwrap();
            let _ = store.page_events(&conversation.id, None, 50).unwrap();
            let _ = store.search("seed", 50).unwrap();
        }
        assert_eq!(store.counters().opens(), opens_after_warmup);
        assert!(store.counters().opens() <= DEFAULT_CONVERSATION_POOL_SIZE);
    }

    #[test]
    fn page_and_search_query_counts_do_not_grow_with_page_size() {
        let store = ConversationStore::open_in_memory().unwrap();
        let conversation = store.create_conversation("Query", owner()).unwrap();
        let agent = Principal {
            id: "agent:one".into(),
            kind: PrincipalKind::Agent,
            display_name: "One".into(),
            agent_id: Some("one".into()),
            created_at_unix_ms: 1,
        };
        let member = store
            .add_member(&conversation.id, agent, MembershipAccess::Member)
            .unwrap();
        for index in 0..120 {
            store
                .append_event(
                    &conversation.id,
                    Some(&member.id),
                    EventKind::Message,
                    &[
                        NewEventPart {
                            id: String::new(),
                            kind: EventPartKind::Text,
                            content: format!("needle {index}"),
                        },
                        NewEventPart {
                            id: String::new(),
                            kind: EventPartKind::Reasoning,
                            content: format!("reasoning {index}"),
                        },
                    ],
                    None,
                    None,
                    true,
                )
                .unwrap();
        }
        let mut page_delta: Option<usize> = None;
        for position in [
            EventPagePosition::After(0),
            EventPagePosition::Latest,
            EventPagePosition::Before(121),
        ] {
            for limit in [10usize, 20, 100] {
                let before = store.counters().queries();
                let page = store
                    .page_events_window(&conversation.id, position, limit)
                    .unwrap();
                let delta = store.counters().queries() - before;
                assert_eq!(page.events.len(), limit.min(120));
                for event in &page.events {
                    if event.kind == EventKind::Message {
                        assert_eq!(event.parts.len(), 2);
                    }
                }
                assert_eq!(delta, 4, "page of {limit} events must cost 4 statements");
                page_delta = Some(delta);
            }
        }
        let mut search_delta: Option<usize> = None;
        for limit in [10usize, 50, 100] {
            let before = store.counters().queries();
            let events = store.search("needle", limit).unwrap();
            let delta = store.counters().queries() - before;
            assert_eq!(events.len(), limit.min(120));
            assert!(events.iter().all(|event| event.parts.len() == 2));
            assert_eq!(delta, 2, "search of {limit} events must cost 2 statements");
            search_delta = Some(delta);
        }
        assert_eq!(page_delta, Some(4));
        assert_eq!(search_delta, Some(2));
    }

    #[test]
    fn creates_initial_group_memberships_atomically() {
        let store = ConversationStore::open_in_memory().unwrap();
        let agent = |id: &str| Principal {
            id: format!("agent:{id}"),
            kind: PrincipalKind::Agent,
            display_name: id.to_owned(),
            agent_id: Some(id.to_owned()),
            created_at_unix_ms: 1,
        };
        let conversation = store
            .create_conversation_with_members(
                "Group",
                owner(),
                &[(agent("one"), MembershipAccess::Member)],
            )
            .unwrap();

        assert!(conversation.is_group);
        assert_eq!(conversation.memberships.len(), 2);
        assert_eq!(conversation.event_count, 1);
        assert_eq!(store.list(false).unwrap()[0].membership_count, 2);

        assert!(
            store
                .create_conversation_with_members("Empty", owner(), &[])
                .is_err()
        );

        let failure = store.create_conversation_with_members(
            "Invalid",
            owner(),
            &[
                (agent("duplicate"), MembershipAccess::Member),
                (agent("duplicate"), MembershipAccess::Member),
            ],
        );
        assert!(failure.is_err());
        assert_eq!(store.list(false).unwrap().len(), 1);
    }

    #[test]
    fn list_summaries_carry_member_agent_ids_and_local_owner() {
        let store = ConversationStore::open_in_memory().unwrap();
        let agent = |id: &str| Principal {
            id: format!("agent:{id}"),
            kind: PrincipalKind::Agent,
            display_name: id.to_owned(),
            agent_id: Some(id.to_owned()),
            created_at_unix_ms: 1,
        };
        let group = store
            .create_conversation_with_members(
                "Mosaic",
                owner(),
                &[
                    (agent("one"), MembershipAccess::Member),
                    (agent("two"), MembershipAccess::Member),
                ],
            )
            .unwrap();
        // A direct conversation without an agent member still lists; its
        // member id list stays empty.
        store.create_conversation("Direct", owner()).unwrap();

        let summaries = store.list(false).unwrap();
        let group_summary = summaries
            .iter()
            .find(|summary| summary.id == group.id)
            .unwrap();
        assert_eq!(group_summary.member_agent_ids, ["one", "two"]);
        assert!(group_summary.has_local_owner);
        let direct = summaries
            .iter()
            .find(|summary| summary.id != group.id)
            .unwrap();
        assert!(direct.member_agent_ids.is_empty());
        assert!(direct.has_local_owner);
    }

    #[test]
    fn schema_upgrade_recovers_existing_multi_member_groups() {
        let store = ConversationStore::open_in_memory().unwrap();
        let agent = |id: &str| Principal {
            id: format!("agent:{id}"),
            kind: PrincipalKind::Agent,
            display_name: id.to_owned(),
            agent_id: Some(id.to_owned()),
            created_at_unix_ms: 1,
        };
        let conversation = store
            .create_conversation_with_members(
                "Existing group",
                owner(),
                &[
                    (agent("one"), MembershipAccess::Member),
                    (agent("two"), MembershipAccess::Member),
                ],
            )
            .unwrap();
        store
            .with_connection(|connection| {
                connection.execute(
                    "UPDATE conversations SET is_group=0 WHERE id=?1",
                    params![conversation.id],
                )?;
                initialize_schema(connection)
            })
            .unwrap();

        assert!(store.get(&conversation.id).unwrap().is_group);
    }

    #[test]
    fn structured_mentions_are_exact_deduplicated_direct_turns() {
        let store = ConversationStore::open_in_memory().unwrap();
        let conversation = store.create_conversation("Project", owner()).unwrap();
        let first = store
            .add_member(
                &conversation.id,
                Principal {
                    id: "agent:one".into(),
                    kind: PrincipalKind::Agent,
                    display_name: "One".into(),
                    agent_id: Some("one".into()),
                    created_at_unix_ms: 1,
                },
                MembershipAccess::Member,
            )
            .unwrap();
        let second = store
            .add_member(
                &conversation.id,
                Principal {
                    id: "agent:two".into(),
                    kind: PrincipalKind::Agent,
                    display_name: "Two".into(),
                    agent_id: Some("two".into()),
                    created_at_unix_ms: 1,
                },
                MembershipAccess::Member,
            )
            .unwrap();
        let event = store
            .append_event(
                &conversation.id,
                None,
                EventKind::Message,
                &[NewEventPart {
                    id: String::new(),
                    kind: EventPartKind::Text,
                    content: "Names in text are not authority".into(),
                }],
                None,
                None,
                true,
            )
            .unwrap();
        let turns = store
            .enqueue_mention_turns(
                &conversation.id,
                &event.id,
                &[first.id.clone(), first.id.clone(), second.id.clone()],
            )
            .unwrap();
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].membership_id, first.id);
        assert_eq!(turns[1].membership_id, second.id);
    }

    #[test]
    fn message_and_mentions_commit_together_and_only_pending_turns_are_claimable() {
        let store = ConversationStore::open_in_memory().unwrap();
        let conversation = store.create_conversation("Project", owner()).unwrap();
        let agent = store
            .add_member(
                &conversation.id,
                Principal {
                    id: "agent:claim".into(),
                    kind: PrincipalKind::Agent,
                    display_name: "Claim".into(),
                    agent_id: Some("claim".into()),
                    created_at_unix_ms: 1,
                },
                MembershipAccess::Member,
            )
            .unwrap();
        let owner_id = store
            .get(&conversation.id)
            .unwrap()
            .memberships
            .into_iter()
            .find(|membership| membership.principal.kind == PrincipalKind::Human)
            .unwrap()
            .id;

        let (event, turns) = store
            .post_message_with_mentions(
                &conversation.id,
                Some(&owner_id),
                "run once",
                None,
                &[agent.id],
            )
            .unwrap();
        assert_eq!(turns.len(), 1);
        assert!(
            store
                .page_events(&conversation.id, None, 50)
                .unwrap()
                .events
                .iter()
                .any(|candidate| candidate.id == event.id)
        );

        let claimed = store.claim_direct_turn(&turns[0].id).unwrap().unwrap();
        assert_eq!(claimed.turn.state, TurnState::Claimed);
        assert!(store.claim_direct_turn(&turns[0].id).unwrap().is_none());
        assert!(store.mark_direct_turn_running(&turns[0].id).unwrap());
        assert!(store.claim_direct_turn(&turns[0].id).unwrap().is_none());
        assert!(!store.mark_direct_turn_running(&turns[0].id).unwrap());
        assert_eq!(
            store.direct_turn(&turns[0].id).unwrap().state,
            TurnState::Running
        );
    }

    #[test]
    fn access_changes_preserve_an_owner_and_finalized_events_are_immutable() {
        let store = ConversationStore::open_in_memory().unwrap();
        let conversation = store.create_conversation("Project", owner()).unwrap();
        let agent = store
            .add_member(
                &conversation.id,
                Principal {
                    id: "agent:owner".into(),
                    kind: PrincipalKind::Agent,
                    display_name: "Owner Agent".into(),
                    agent_id: Some("owner".into()),
                    created_at_unix_ms: 1,
                },
                MembershipAccess::Member,
            )
            .unwrap();
        assert_eq!(
            store
                .set_member_access(&conversation.id, &agent.id, MembershipAccess::Owner)
                .unwrap()
                .access,
            MembershipAccess::Owner
        );
        let local_owner = store
            .get(&conversation.id)
            .unwrap()
            .memberships
            .into_iter()
            .find(|membership| membership.principal.id == "human:local")
            .unwrap();
        store
            .set_member_access(&conversation.id, &local_owner.id, MembershipAccess::Member)
            .unwrap();
        assert_eq!(
            store
                .set_member_access(&conversation.id, &agent.id, MembershipAccess::Member)
                .unwrap_err()
                .to_string(),
            "conversation_requires_owner"
        );

        let event = store
            .append_event(
                &conversation.id,
                Some(&agent.id),
                EventKind::Message,
                &[],
                None,
                None,
                false,
            )
            .unwrap();
        store
            .append_event_part(
                &event.id,
                NewEventPart {
                    id: String::new(),
                    kind: EventPartKind::Text,
                    content: "streamed".into(),
                },
            )
            .unwrap();
        store.finalize_event(&event.id).unwrap();
        assert_eq!(
            store
                .append_event_part(
                    &event.id,
                    NewEventPart {
                        id: String::new(),
                        kind: EventPartKind::Text,
                        content: "late".into(),
                    },
                )
                .unwrap_err()
                .to_string(),
            "event_already_finalized"
        );
    }

    #[test]
    fn event_pages_are_bounded_and_search_uses_persisted_parts() {
        let store = ConversationStore::open_in_memory().unwrap();
        let conversation = store.create_conversation("Project", owner()).unwrap();
        for index in 0..105 {
            store
                .append_event(
                    &conversation.id,
                    None,
                    EventKind::Message,
                    &[NewEventPart {
                        id: String::new(),
                        kind: EventPartKind::Text,
                        content: format!("event {index} searchable-token"),
                    }],
                    None,
                    None,
                    true,
                )
                .unwrap();
        }
        let first = store
            .page_events(&conversation.id, None, usize::MAX)
            .unwrap();
        assert_eq!(first.events.len(), MAX_EVENT_PAGE_SIZE);
        assert_eq!(first.total_count, 105);
        let second = store
            .page_events(
                &conversation.id,
                first.next_cursor.and_then(|value| value.parse().ok()),
                DEFAULT_EVENT_PAGE_SIZE,
            )
            .unwrap();
        assert_eq!(second.events.len(), 5);
        assert_eq!(store.search("searchable-token", 1).unwrap().len(), 1);
    }

    #[test]
    fn event_pages_use_retained_sequence_anchors_for_latest_older_and_tail() {
        let store = ConversationStore::open_in_memory().unwrap();
        let conversation = store.create_conversation("Sparse pages", owner()).unwrap();
        let append = || {
            store
                .append_event(
                    &conversation.id,
                    None,
                    EventKind::Message,
                    &[NewEventPart {
                        id: String::new(),
                        kind: EventPartKind::Text,
                        content: "retained complete content".into(),
                    }],
                    None,
                    None,
                    true,
                )
                .unwrap()
        };
        for _ in 0..65 {
            append();
        }
        store
            .with_connection(|connection| {
                connection.execute(
                    "DELETE FROM events WHERE conversation_id=?1 AND sequence % 3 = 0",
                    [&conversation.id],
                )?;
                Ok(())
            })
            .unwrap();
        let retained: Vec<i64> = (1..=65).filter(|sequence| sequence % 3 != 0).collect();
        let mut position = EventPagePosition::Latest;
        let mut recovered = Vec::new();
        let mut end = retained.len();
        let mut latest_cursor = None;
        loop {
            let page = store
                .page_events_window(&conversation.id, position, DEFAULT_EVENT_PAGE_SIZE)
                .unwrap();
            let sequences: Vec<_> = page.events.iter().map(|event| event.sequence).collect();
            let start = end.saturating_sub(20);
            assert_eq!(sequences, retained[start..end]);
            assert_eq!(page.total_count, retained.len() as i64);
            assert_eq!(page.next_cursor, sequences.last().map(ToString::to_string));
            assert_eq!(page.has_earlier, start > 0);
            assert!(page.events.iter().all(|event| {
                event.parts.len() == 1 && event.parts[0].content == "retained complete content"
            }));
            if latest_cursor.is_none() {
                latest_cursor = page.next_cursor.clone();
            }
            recovered.splice(0..0, sequences);
            let Some(before) = page.next_before_sequence else {
                assert!(!page.has_earlier);
                break;
            };
            assert_eq!(before, retained[start]);
            position = EventPagePosition::Before(before);
            end = start;
        }
        assert_eq!(recovered, retained);

        // A deleted anchor is still a valid exclusive sequence boundary.
        let page = store
            .page_events_window(&conversation.id, EventPagePosition::Before(36), 20)
            .unwrap();
        assert_eq!(page.events.len(), 20);
        assert_eq!(page.events.last().unwrap().sequence, 35);

        let appended = [append(), append()];
        let tail = store
            .page_events(
                &conversation.id,
                latest_cursor.and_then(|cursor| cursor.parse().ok()),
                20,
            )
            .unwrap();
        assert_eq!(
            tail.events
                .iter()
                .map(|event| &event.id)
                .collect::<Vec<_>>(),
            appended.iter().map(|event| &event.id).collect::<Vec<_>>()
        );
        assert_eq!(tail.total_count, retained.len() as i64 + 2);
        assert!(tail.has_earlier);
        assert_eq!(tail.next_before_sequence, Some(appended[0].sequence));
        let empty = store
            .page_events(&conversation.id, Some(appended[1].sequence), 20)
            .unwrap();
        assert!(empty.events.is_empty());
        assert_eq!(empty.next_cursor, None);
        assert!(!empty.has_earlier);
        assert_eq!(empty.next_before_sequence, None);
    }

    #[test]
    fn event_pages_have_no_earlier_cursor_without_retained_rows() {
        let store = ConversationStore::open_in_memory().unwrap();
        let conversation = store.create_conversation("Empty pages", owner()).unwrap();
        for position in [EventPagePosition::Latest, EventPagePosition::Before(1)] {
            let page = store
                .page_events_window(&conversation.id, position, 20)
                .unwrap();
            assert!(page.events.is_empty());
            assert_eq!(page.total_count, 0);
            assert_eq!(page.next_cursor, None);
            assert!(!page.has_earlier);
            assert_eq!(page.next_before_sequence, None);
        }
    }

    #[test]
    fn export_import_preserves_stable_ids_and_rejects_conflicts_atomically() {
        let source_root = std::env::temp_dir().join(format!("lico-export-{}", Uuid::new_v4()));
        let destination_root = std::env::temp_dir().join(format!("lico-import-{}", Uuid::new_v4()));
        path_security::ensure_private_dir(&source_root).unwrap();
        path_security::ensure_private_dir(&destination_root).unwrap();
        let source = ConversationStore::open(&source_root).unwrap();
        let conversation = source.create_conversation("Transfer", owner()).unwrap();
        let agent = source
            .add_member(
                &conversation.id,
                Principal {
                    id: "agent:transfer".into(),
                    kind: PrincipalKind::Agent,
                    display_name: "Transfer Agent".into(),
                    agent_id: Some("transfer".into()),
                    created_at_unix_ms: 1,
                },
                MembershipAccess::Member,
            )
            .unwrap();
        let event = source
            .append_event(
                &conversation.id,
                Some(&agent.id),
                EventKind::Message,
                &[NewEventPart {
                    id: String::new(),
                    kind: EventPartKind::Reasoning,
                    content: "complete structured output".into(),
                }],
                None,
                None,
                true,
            )
            .unwrap();
        let bundle = source_root.join("bundle.json");
        source
            .export_bundle(&bundle, std::slice::from_ref(&conversation.id))
            .unwrap();

        let destination = ConversationStore::open(&destination_root).unwrap();
        assert_eq!(destination.import_bundle(&bundle).unwrap()["imported"], 1);
        let imported = destination.get(&conversation.id).unwrap();
        assert_eq!(imported.memberships[1].id, agent.id);
        assert_eq!(
            destination
                .page_events(&conversation.id, None, 50)
                .unwrap()
                .events
                .into_iter()
                .find(|candidate| candidate.id == event.id)
                .unwrap()
                .id,
            event.id,
        );
        assert_eq!(
            destination.import_bundle(&bundle).unwrap()["alreadyPresent"],
            1
        );

        let mut conflicting: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&bundle).unwrap()).unwrap();
        conflicting["conversations"][0]["conversation"]["title"] = serde_json::json!("Conflicting");
        std::fs::write(&bundle, serde_json::to_vec(&conflicting).unwrap()).unwrap();
        assert_eq!(
            destination.import_bundle(&bundle).unwrap_err().to_string(),
            "conversation_import_identity_conflict"
        );
        assert_eq!(destination.get(&conversation.id).unwrap().title, "Transfer");
        let _ = std::fs::remove_dir_all(source_root);
        let _ = std::fs::remove_dir_all(destination_root);
    }

    #[test]
    fn dispatch_runtime_locations_stay_private_and_membership_scoped() {
        let root = std::env::temp_dir().join(format!("lico-dispatch-{}", Uuid::new_v4()));
        path_security::ensure_private_dir(&root).unwrap();
        let store = ConversationStore::open(&root).unwrap();
        let conversation = store.create_conversation("Dispatch", owner()).unwrap();
        let agent = store
            .add_member(
                &conversation.id,
                Principal {
                    id: "agent:dispatch".into(),
                    kind: PrincipalKind::Agent,
                    display_name: "Dispatch Agent".into(),
                    agent_id: Some("dispatch".into()),
                    created_at_unix_ms: 1,
                },
                MembershipAccess::Member,
            )
            .unwrap();
        let dispatch = store
            .create_dispatch(
                &conversation.id,
                &agent.id,
                "subagent.delegate",
                DispatchSessionMode::New,
            )
            .unwrap();
        assert!(
            store
                .latest_resumable_dispatch(&conversation.id, &agent.id)
                .unwrap()
                .is_none()
        );
        let private_location = "opaque-runtime-session-location";
        store
            .update_dispatch(
                &dispatch.id,
                DispatchState::Completed,
                Some(private_location),
                None,
            )
            .unwrap();
        assert_eq!(
            store
                .latest_resumable_dispatch(&conversation.id, &agent.id)
                .unwrap()
                .unwrap()
                .runtime_conversation_path
                .as_deref(),
            Some(private_location)
        );
        let bundle = root.join("bundle.json");
        store
            .export_bundle(&bundle, std::slice::from_ref(&conversation.id))
            .unwrap();
        assert!(
            !std::fs::read_to_string(bundle)
                .unwrap()
                .contains(private_location)
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn dispatch_record_reads_one_identity_and_reports_absent_unknowns() {
        let store = ConversationStore::open_in_memory().unwrap();
        let conversation = store.create_conversation("Workflow", owner()).unwrap();
        let agent = store
            .add_member(
                &conversation.id,
                Principal {
                    id: "agent:workflow-read".into(),
                    kind: PrincipalKind::Agent,
                    display_name: "Workflow Agent".into(),
                    agent_id: Some("workflow-read".into()),
                    created_at_unix_ms: 1,
                },
                MembershipAccess::Member,
            )
            .unwrap();
        assert!(
            store
                .dispatch_record("dispatch:workflow-missing")
                .unwrap()
                .is_none()
        );
        let dispatch = store
            .create_dispatch(
                &conversation.id,
                &agent.id,
                "assistant.workflow.dispatch",
                DispatchSessionMode::New,
            )
            .unwrap();
        assert_eq!(
            store.dispatch_record(&dispatch.id).unwrap(),
            Some(dispatch.clone())
        );
        assert!(
            store
                .latest_send_dispatch(&conversation.id, &agent.id)
                .unwrap()
                .is_none(),
            "workflow dispatch is not a send turn"
        );
        let scope = store
            .prepare_runtime_dispatch(
                "workflow-read",
                "",
                "admitted turn text",
                Some(&conversation.id),
                Some(&agent.id),
                Some("event:cause"),
                None,
            )
            .unwrap();
        let send = store
            .latest_send_dispatch(&conversation.id, &agent.id)
            .unwrap()
            .expect("send dispatch");
        assert_eq!(send.id, scope.dispatch_id);
        let event = store
            .agent_turn_event_for_dispatch(&conversation.id, &scope.dispatch_id)
            .unwrap()
            .expect("agent turn event");
        assert_eq!(
            event.author_membership_id.as_deref(),
            Some(agent.id.as_str())
        );
        assert_eq!(
            event.correlation_id.as_deref(),
            Some(scope.dispatch_id.as_str())
        );
        assert!(
            store
                .agent_turn_event_for_dispatch(&conversation.id, "dispatch:missing")
                .unwrap()
                .is_none()
        );
        store
            .update_dispatch(&dispatch.id, DispatchState::Completed, None, None)
            .unwrap();
        assert_eq!(
            store
                .dispatch_record(&dispatch.id)
                .unwrap()
                .map(|record| record.state),
            Some(DispatchState::Completed)
        );
    }

    #[test]
    fn requested_dispatch_identity_is_canonical_and_rejects_a_duplicate_open() {
        let store = ConversationStore::open_in_memory().unwrap();
        let dispatch_id = "assistant-workflow:run:RUN-001:1";
        let scope = store
            .prepare_runtime_dispatch(
                "workflow-agent",
                "native-session-workflow",
                "synthetic workflow input",
                None,
                None,
                Some("assistant.workflow.dispatch"),
                Some(dispatch_id),
            )
            .unwrap();
        assert_eq!(scope.dispatch_id, dispatch_id);
        let record = store.dispatch_record(dispatch_id).unwrap().unwrap();
        assert_eq!(record.id, dispatch_id);
        assert_eq!(record.operation, "send");
        assert_eq!(record.state, DispatchState::Accepted);
        assert!(
            store
                .prepare_runtime_dispatch(
                    "workflow-agent",
                    "native-session-workflow",
                    "synthetic workflow input",
                    None,
                    None,
                    Some("assistant.workflow.dispatch"),
                    Some(dispatch_id),
                )
                .is_err()
        );
        assert_eq!(store.dispatch_record(dispatch_id).unwrap(), Some(record));
    }

    // ---- durable reserved-group cutover (schema v2 -> v3) ----

    fn seed_v2_database(connection: &Connection, block_version_three: bool) {
        if block_version_three {
            connection
                .execute_batch(
                    "CREATE TABLE schema_meta (
                       key TEXT PRIMARY KEY, value TEXT NOT NULL, CHECK(value <> '3')
                     );",
                )
                .unwrap();
        } else {
            connection
                .execute_batch(
                    "CREATE TABLE schema_meta (
                       key TEXT PRIMARY KEY, value TEXT NOT NULL
                     );",
                )
                .unwrap();
        }
        connection
            .execute(
                "INSERT INTO schema_meta(key, value) VALUES ('version', '2')",
                [],
            )
            .unwrap();
        connection
            .execute_batch(CONVERSATION_SCHEMA_TABLES)
            .unwrap();

        for (id, kind, name, agent_id, created) in [
            ("human:local", "human", "You", None::<&str>, 100),
            ("agent:one", "agent", "One", Some("one"), 200),
            ("agent:two", "agent", "Two", Some("two"), 300),
        ] {
            connection
                .execute(
                    "INSERT INTO principals(id, kind, display_name, agent_id, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![id, kind, name, agent_id, created],
                )
                .unwrap();
        }
        for (id, title, archived, pinned, is_group, revision, created, updated) in [
            ("legacy-reserved-group", "Lico", 0, 1, 0, 7, 1000, 9000),
            ("custom-group", "Custom Group", 0, 0, 1, 3, 1000, 8000),
        ] {
            connection
                .execute(
                    "INSERT INTO conversations(id, title, archived, pinned, is_group,
                     revision, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![
                        id, title, archived, pinned, is_group, revision, created, updated
                    ],
                )
                .unwrap();
        }
        for (id, conversation_id, principal_id, access, status, joined, left) in [
            (
                "m-owner",
                "legacy-reserved-group",
                "human:local",
                "owner",
                "active",
                1000,
                None::<i64>,
            ),
            (
                "m-one",
                "legacy-reserved-group",
                "agent:one",
                "member",
                "active",
                2000,
                None,
            ),
            (
                "m-two",
                "legacy-reserved-group",
                "agent:two",
                "member",
                "left",
                3000,
                Some(9000),
            ),
            (
                "mc-owner",
                "custom-group",
                "human:local",
                "owner",
                "active",
                1000,
                None,
            ),
            (
                "mc-one",
                "custom-group",
                "agent:one",
                "member",
                "active",
                2000,
                None,
            ),
            (
                "mc-two",
                "custom-group",
                "agent:two",
                "member",
                "left",
                3000,
                Some(8000),
            ),
        ] {
            connection
                .execute(
                    "INSERT INTO memberships(id, conversation_id, principal_id, access,
                     status, joined_at, left_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        id,
                        conversation_id,
                        principal_id,
                        access,
                        status,
                        joined,
                        left
                    ],
                )
                .unwrap();
        }

        let event = |id: &str,
                     conversation_id: &str,
                     sequence: i64,
                     author: Option<&str>,
                     kind: &str,
                     created_at: i64| {
            connection
                .execute(
                    "INSERT INTO events(id, conversation_id, sequence, author_membership_id,
                     kind, causation_id, correlation_id, created_at, finalized)
                     VALUES (?1, ?2, ?3, ?4, ?5, NULL, NULL, ?6, 1)",
                    params![id, conversation_id, sequence, author, kind, created_at],
                )
                .unwrap();
        };
        let metadata_part = |event_id: &str, content: &str, created_at: i64| {
            connection
                .execute(
                    "INSERT INTO event_parts(id, event_id, ordinal, kind, content, created_at)
                     VALUES (?1, ?2, 0, 'metadata', ?3, ?4)",
                    params![format!("{event_id}-part"), event_id, content, created_at],
                )
                .unwrap();
        };
        let message_part =
            |event_id: &str, conversation_id: &str, content: &str, created_at: i64| {
                connection
                    .execute(
                        "INSERT INTO event_parts(id, event_id, ordinal, kind, content, created_at)
                         VALUES (?1, ?2, 0, 'text', ?3, ?4)",
                        params![format!("{event_id}-part"), event_id, content, created_at],
                    )
                    .unwrap();
                connection
                    .execute(
                        "INSERT INTO event_search(event_id, conversation_id, content)
                         VALUES (?1, ?2, ?3)",
                        params![event_id, conversation_id, content],
                    )
                    .unwrap();
            };

        event(
            "e1",
            "legacy-reserved-group",
            1,
            Some("m-owner"),
            "message",
            1100,
        );
        message_part("e1", "legacy-reserved-group", "first hello", 1100);
        event(
            "e2",
            "legacy-reserved-group",
            2,
            None,
            "membership-changed",
            2000,
        );
        metadata_part(
            "e2",
            r#"{"membershipId":"m-one","principalId":"agent:one","change":"joined"}"#,
            2000,
        );
        event(
            "e3",
            "legacy-reserved-group",
            3,
            None,
            "membership-changed",
            3000,
        );
        metadata_part(
            "e3",
            r#"{"membershipId":"m-two","principalId":"agent:two","change":"joined"}"#,
            3000,
        );
        event(
            "e4",
            "legacy-reserved-group",
            4,
            Some("m-owner"),
            "message",
            4000,
        );
        message_part("e4", "legacy-reserved-group", "middle message", 4000);
        event(
            "e5",
            "legacy-reserved-group",
            5,
            None,
            "membership-changed",
            9000,
        );
        metadata_part(
            "e5",
            r#"{"membershipId":"m-two","principalId":"agent:two","change":"left"}"#,
            9000,
        );
        event(
            "e6",
            "legacy-reserved-group",
            6,
            None,
            "membership-changed",
            9100,
        );
        metadata_part(
            "e6",
            r#"{"membershipId":"m-x","principalId":"agent:ghost","change":"left","reason":"retired"}"#,
            9100,
        );
        event(
            "e7",
            "legacy-reserved-group",
            7,
            None,
            "membership-changed",
            9200,
        );
        metadata_part(
            "e7",
            r#"{"membershipId":"m-two","change":"access-set","access":"member"}"#,
            9200,
        );
        event(
            "e8",
            "legacy-reserved-group",
            8,
            Some("m-one"),
            "message",
            9300,
        );
        message_part("e8", "legacy-reserved-group", "final hello", 9300);
        event(
            "e9",
            "legacy-reserved-group",
            9,
            Some("m-owner"),
            "membership-changed",
            9400,
        );
        metadata_part(
            "e9",
            r#"{"membershipId":"m-one","principalId":"agent:one","change":"joined"}"#,
            9400,
        );
        event(
            "e10",
            "legacy-reserved-group",
            10,
            None,
            "membership-changed",
            9500,
        );
        metadata_part(
            "e10",
            r#"{"membershipId":"m-one","principalId":"agent:one","change":"joined"}"#,
            9500,
        );
        connection
            .execute(
                "INSERT INTO event_parts(id, event_id, ordinal, kind, content, created_at)
                 VALUES ('e10-extra', 'e10', 1, 'text', 'preserve this near-match', 9500)",
                [],
            )
            .unwrap();
        event(
            "e11",
            "legacy-reserved-group",
            11,
            None,
            "membership-changed",
            9600,
        );
        metadata_part(
            "e11",
            r#"{"membershipId":"m-one","principalId":"agent:two","change":"joined"}"#,
            9600,
        );
        event(
            "e12",
            "legacy-reserved-group",
            12,
            None,
            "membership-changed",
            9700,
        );
        metadata_part(
            "e12",
            r#"{"membershipId":"m-owner","principalId":"human:local","change":"joined"}"#,
            9700,
        );

        event("ce1", "custom-group", 1, Some("mc-owner"), "message", 1100);
        message_part("ce1", "custom-group", "custom message", 1100);
        event("ce2", "custom-group", 2, None, "membership-changed", 2000);
        metadata_part(
            "ce2",
            r#"{"membershipId":"mc-two","principalId":"agent:two","change":"joined"}"#,
            2000,
        );

        connection
            .execute(
                "INSERT INTO migration_provenance(source_kind, source_identity, conversation_id)
                 VALUES ('group', 'lico-group-default', 'legacy-reserved-group')",
                [],
            )
            .unwrap();
    }

    fn fixture_database(root: &Path) -> std::path::PathBuf {
        root.join("client-state")
            .join("conversations")
            .join("conversations.sqlite3")
    }

    fn database_layout(connection: &Connection) -> Vec<(String, String)> {
        connection
            .prepare("SELECT name, sql FROM sqlite_schema WHERE sql IS NOT NULL ORDER BY name")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }

    fn assert_open_refuses_without_database_changes(
        root: &Path,
        metadata: &str,
        expected_error: &str,
        open: fn(&Path) -> StoreResult<ConversationStore>,
    ) {
        let database = fixture_database(root);
        std::fs::create_dir_all(database.parent().unwrap()).unwrap();
        #[cfg(unix)]
        let before_directory_permissions = {
            use std::os::unix::fs::PermissionsExt;
            std::fs::metadata(database.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
        };
        let connection = Connection::open(&database).unwrap();
        connection
            .execute_batch("PRAGMA journal_mode=DELETE;")
            .unwrap();
        connection.execute_batch(metadata).unwrap();
        let before_layout = database_layout(&connection);
        let before_journal_mode: String = connection
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .unwrap();
        let before_content: String = connection
            .query_row("SELECT value FROM retained_fixture", [], |row| row.get(0))
            .unwrap();
        drop(connection);
        let sidecars_before = ["-wal", "-shm"].map(|suffix| {
            let mut sidecar = database.as_os_str().to_os_string();
            sidecar.push(suffix);
            std::fs::symlink_metadata(Path::new(&sidecar)).is_ok()
        });
        #[cfg(unix)]
        let before_permissions = {
            use std::os::unix::fs::PermissionsExt;
            std::fs::metadata(&database).unwrap().permissions().mode()
        };

        let error = open(root).unwrap_err().to_string();
        assert!(error.contains(expected_error), "{error}");

        let connection = Connection::open(&database).unwrap();
        assert_eq!(database_layout(&connection), before_layout);
        let after_journal_mode: String = connection
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .unwrap();
        assert_eq!(after_journal_mode, before_journal_mode);
        let after_content: String = connection
            .query_row("SELECT value FROM retained_fixture", [], |row| row.get(0))
            .unwrap();
        assert_eq!(after_content, before_content);
        drop(connection);
        let sidecars_after = ["-wal", "-shm"].map(|suffix| {
            let mut sidecar = database.as_os_str().to_os_string();
            sidecar.push(suffix);
            std::fs::symlink_metadata(Path::new(&sidecar)).is_ok()
        });
        assert_eq!(sidecars_after, sidecars_before);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let after_permissions = std::fs::metadata(&database).unwrap().permissions().mode();
            assert_eq!(after_permissions, before_permissions);
            let after_directory_permissions = std::fs::metadata(database.parent().unwrap())
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(after_directory_permissions, before_directory_permissions);
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn open_rejects_unpublished_schema_before_configuring_sqlite() {
        let root = std::env::temp_dir().join(format!("lico-conv-unsupported-{}", Uuid::new_v4()));
        assert_open_refuses_without_database_changes(
            &root,
            "CREATE TABLE schema_meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO schema_meta VALUES ('version', '13');
             CREATE TABLE retained_fixture(value TEXT NOT NULL);
             INSERT INTO retained_fixture VALUES ('synthetic retained data');",
            "conversation_schema_unsupported_version",
            ConversationStore::open,
        );
    }

    #[test]
    fn open_initializes_existing_empty_or_unrelated_store_without_losing_rows() {
        for seed in [
            "",
            "CREATE TABLE schema_meta(key TEXT PRIMARY KEY, value TEXT NOT NULL); INSERT INTO schema_meta VALUES ('format', 'synthetic'); CREATE TABLE retained_fixture(value TEXT NOT NULL); INSERT INTO retained_fixture VALUES ('synthetic retained data');",
        ] {
            let root = std::env::temp_dir().join(format!("lico-conv-empty-{}", Uuid::new_v4()));
            let database = fixture_database(&root);
            std::fs::create_dir_all(database.parent().unwrap()).unwrap();
            let connection = Connection::open(&database).unwrap();
            connection.execute_batch(seed).unwrap();
            drop(connection);
            let store = ConversationStore::open(&root).unwrap();
            store.checkpoint().unwrap();
            let connection = Connection::open(&database).unwrap();
            assert_eq!(
                connection
                    .query_row(
                        "SELECT value FROM schema_meta WHERE key='version'",
                        [],
                        |row| row.get::<_, String>(0)
                    )
                    .unwrap(),
                CURRENT_SCHEMA_VERSION
            );
            if !seed.is_empty() {
                assert_eq!(
                    connection
                        .query_row("SELECT value FROM retained_fixture", [], |row| row
                            .get::<_, String>(0))
                        .unwrap(),
                    "synthetic retained data"
                );
            }
            drop(connection);
            drop(store);
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn open_requires_admission_for_published_schema_without_changing_it() {
        let root = std::env::temp_dir().join(format!("lico-conv-admission-{}", Uuid::new_v4()));
        assert_open_refuses_without_database_changes(
            &root,
            "CREATE TABLE schema_meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO schema_meta VALUES ('version', '12');
             CREATE TABLE retained_fixture(value TEXT NOT NULL);
             INSERT INTO retained_fixture VALUES ('synthetic retained data');",
            "conversation_schema_migration_required",
            ConversationStore::open,
        );
    }

    #[test]
    fn open_preserves_an_existing_wal_when_rejecting_unsupported_schema() {
        let root = std::env::temp_dir().join(format!("lico-conv-wal-{}", Uuid::new_v4()));
        let database = fixture_database(&root);
        std::fs::create_dir_all(database.parent().unwrap()).unwrap();
        let connection = Connection::open(&database).unwrap();
        let journal_mode: String = connection
            .query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))
            .unwrap();
        assert_eq!(journal_mode, "wal");
        connection
            .execute_batch(
                "CREATE TABLE schema_meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
                 INSERT INTO schema_meta VALUES ('version', '13');
                 CREATE TABLE retained_fixture(value TEXT NOT NULL);
                 INSERT INTO retained_fixture VALUES ('synthetic retained data');",
            )
            .unwrap();

        let mut wal_path = database.as_os_str().to_os_string();
        wal_path.push("-wal");
        let wal_path = PathBuf::from(wal_path);
        let mut shm_path = database.as_os_str().to_os_string();
        shm_path.push("-shm");
        let shm_path = PathBuf::from(shm_path);
        let database_before = std::fs::read(&database).unwrap();
        let wal_before = std::fs::read(&wal_path).unwrap();
        assert!(shm_path.exists());

        let error = ConversationStore::open(&root).unwrap_err().to_string();
        assert!(error.contains("conversation_schema_unsupported_version"));
        assert_eq!(std::fs::read(&database).unwrap(), database_before);
        assert_eq!(std::fs::read(&wal_path).unwrap(), wal_before);
        assert!(shm_path.exists());
        assert_eq!(
            connection
                .query_row(
                    "SELECT value FROM schema_meta WHERE key='version'",
                    [],
                    |row| row.get::<_, String>(0)
                )
                .unwrap(),
            "13"
        );
        assert_eq!(
            connection
                .query_row("SELECT value FROM retained_fixture", [], |row| row
                    .get::<_, String>(0))
                .unwrap(),
            "synthetic retained data"
        );
        drop(connection);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn open_rejects_incomplete_current_schema_without_configuring_sqlite() {
        let root = std::env::temp_dir().join(format!("lico-conv-incomplete-{}", Uuid::new_v4()));
        assert_open_refuses_without_database_changes(
            &root,
            "CREATE TABLE schema_meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO schema_meta VALUES ('version', '19');
             CREATE TABLE retained_fixture(value TEXT NOT NULL);
             INSERT INTO retained_fixture VALUES ('synthetic retained data');",
            "conversation_schema_incomplete",
            ConversationStore::open,
        );
    }

    #[test]
    fn open_rejects_missing_current_runtime_column_without_changes() {
        let root =
            std::env::temp_dir().join(format!("lico-conv-missing-column-{}", Uuid::new_v4()));
        let database = fixture_database(&root);
        std::fs::create_dir_all(database.parent().unwrap()).unwrap();
        let mut connection = Connection::open(&database).unwrap();
        initialize_schema(&mut connection).unwrap();
        connection
            .execute_batch(
                "PRAGMA journal_mode=DELETE;
                 CREATE TABLE retained_fixture(value TEXT NOT NULL);
                 INSERT INTO retained_fixture VALUES ('synthetic retained data');
                 ALTER TABLE events DROP COLUMN finalized;",
            )
            .unwrap();
        let before_layout = database_layout(&connection);
        let before_journal_mode: String = connection
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .unwrap();
        assert_eq!(before_journal_mode, "delete");
        drop(connection);

        let database_before = std::fs::read(&database).unwrap();
        let sidecars_before = ["-wal", "-shm"].map(|suffix| {
            let mut sidecar = database.as_os_str().to_os_string();
            sidecar.push(suffix);
            std::fs::symlink_metadata(Path::new(&sidecar)).is_ok()
        });
        #[cfg(unix)]
        let (before_file_permissions, before_directory_permissions) = {
            use std::os::unix::fs::PermissionsExt;
            (
                std::fs::metadata(&database).unwrap().permissions().mode(),
                std::fs::metadata(database.parent().unwrap())
                    .unwrap()
                    .permissions()
                    .mode(),
            )
        };

        let error = ConversationStore::open(&root).unwrap_err().to_string();
        assert!(error.contains("conversation_schema_incomplete"), "{error}");
        assert_eq!(std::fs::read(&database).unwrap(), database_before);
        let sidecars_after = ["-wal", "-shm"].map(|suffix| {
            let mut sidecar = database.as_os_str().to_os_string();
            sidecar.push(suffix);
            std::fs::symlink_metadata(Path::new(&sidecar)).is_ok()
        });
        assert_eq!(sidecars_after, sidecars_before);
        let connection =
            Connection::open_with_flags(&database, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .unwrap();
        assert_eq!(database_layout(&connection), before_layout);
        assert_eq!(
            connection
                .query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0))
                .unwrap(),
            "delete"
        );
        assert_eq!(
            connection
                .query_row("SELECT value FROM retained_fixture", [], |row| {
                    row.get::<_, String>(0)
                })
                .unwrap(),
            "synthetic retained data"
        );
        drop(connection);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&database).unwrap().permissions().mode(),
                before_file_permissions
            );
            assert_eq!(
                std::fs::metadata(database.parent().unwrap())
                    .unwrap()
                    .permissions()
                    .mode(),
                before_directory_permissions
            );
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn migration_open_rejects_unpublished_schema_without_changes() {
        let root = std::env::temp_dir().join(format!("lico-conv-migration-{}", Uuid::new_v4()));
        assert_open_refuses_without_database_changes(
            &root,
            "CREATE TABLE schema_meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO schema_meta VALUES ('version', '17');
             CREATE TABLE retained_fixture(value TEXT NOT NULL);
             INSERT INTO retained_fixture VALUES ('synthetic retained data');",
            "conversation_schema_unsupported_version",
            ConversationStore::open_for_migration,
        );
    }

    #[test]
    fn open_preserves_orphaned_sqlite_sidecars_without_creating_a_database() {
        for suffix in ["-wal", "-shm"] {
            let root = std::env::temp_dir().join(format!(
                "lico-conv-orphan-{}-{}",
                suffix.trim_start_matches('-'),
                Uuid::new_v4()
            ));
            let database = fixture_database(&root);
            std::fs::create_dir_all(database.parent().unwrap()).unwrap();
            let mut sidecar = database.as_os_str().to_os_string();
            sidecar.push(suffix);
            let sidecar = PathBuf::from(sidecar);
            std::fs::write(&sidecar, b"synthetic retained sidecar").unwrap();
            #[cfg(unix)]
            let before_directory_permissions = {
                use std::os::unix::fs::PermissionsExt;
                std::fs::metadata(database.parent().unwrap())
                    .unwrap()
                    .permissions()
                    .mode()
            };

            assert!(ConversationStore::open(&root).is_err());
            assert!(!database.exists());
            assert_eq!(
                std::fs::read(&sidecar).unwrap(),
                b"synthetic retained sidecar"
            );
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(
                    std::fs::metadata(database.parent().unwrap())
                        .unwrap()
                        .permissions()
                        .mode(),
                    before_directory_permissions
                );
            }
            let _ = std::fs::remove_dir_all(&root);
        }
    }

    fn open_fixture_connection(root: &Path) -> rusqlite::Connection {
        let connection = rusqlite::Connection::open(fixture_database(root)).unwrap();
        configure_connection(&connection).unwrap();
        connection
    }

    fn snapshot_group(root: &Path, conversation_id: &str) -> String {
        let connection = open_fixture_connection(root);
        let mut out = String::new();
        let queries: Vec<(&str, &str)> = vec![
            (
                "conversations",
                "SELECT id, title, archived, pinned, is_group, revision, created_at, updated_at
                 FROM conversations WHERE id=?1",
            ),
            (
                "memberships",
                "SELECT id, conversation_id, principal_id, access, status, joined_at, left_at
                 FROM memberships WHERE conversation_id=?1 ORDER BY id",
            ),
            (
                "events",
                "SELECT id, conversation_id, sequence, author_membership_id, kind, causation_id,
                 correlation_id, created_at, finalized
                 FROM events WHERE conversation_id=?1 ORDER BY sequence, id",
            ),
            (
                "event_parts",
                "SELECT id, event_id, ordinal, kind, content, created_at FROM event_parts
                 WHERE event_id IN (SELECT id FROM events WHERE conversation_id=?1)
                 ORDER BY event_id, ordinal, id",
            ),
            (
                "event_search",
                "SELECT event_id, conversation_id, content FROM event_search
                 WHERE conversation_id=?1 ORDER BY event_id",
            ),
        ];
        for (table, sql) in queries {
            let mut statement = connection.prepare(sql).unwrap();
            let mut rows = statement.query(params![conversation_id]).unwrap();
            while let Some(row) = rows.next().unwrap() {
                let mut values = Vec::new();
                for index in 0..row.as_ref().column_count() {
                    values.push(format!(
                        "{:?}",
                        row.get::<_, rusqlite::types::Value>(index).unwrap()
                    ));
                }
                out.push_str(&format!(
                    "{table}|{}
",
                    values.join("|")
                ));
            }
        }
        out
    }

    fn snapshot_database(root: &Path) -> String {
        let connection = open_fixture_connection(root);
        let mut out = String::new();
        let tables = [
            "principals",
            "conversations",
            "memberships",
            "events",
            "event_parts",
            "event_search",
            "source_links",
            "runtime_bindings",
            "conversation_dispatches",
            "subagent_dispatch_claims",
            "subagent_mcp_inbound",
            "migration_provenance",
            "schema_meta",
        ];
        for table in tables {
            let mut statement = connection
                .prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))
                .unwrap();
            let mut rows = statement.query([]).unwrap();
            while let Some(row) = rows.next().unwrap() {
                let mut values = Vec::new();
                for index in 0..row.as_ref().column_count() {
                    values.push(format!(
                        "{:?}",
                        row.get::<_, rusqlite::types::Value>(index).unwrap()
                    ));
                }
                out.push_str(&format!(
                    "{table}|{}
",
                    values.join("|")
                ));
            }
        }
        out
    }

    fn schema_version(root: &Path) -> String {
        let connection = open_fixture_connection(root);
        connection
            .query_row(
                "SELECT value FROM schema_meta WHERE key='version'",
                [],
                |row| row.get(0),
            )
            .unwrap()
    }

    #[test]
    fn migrates_v2_reserved_group_and_preserves_everything_else() {
        let root = std::env::temp_dir().join(format!("lico-conv-v2-{}", Uuid::new_v4()));
        std::fs::create_dir_all(fixture_database(&root).parent().unwrap()).unwrap();
        let fixture = open_fixture_connection(&root);
        seed_v2_database(&fixture, false);
        fixture.close().unwrap();

        let custom_before = snapshot_group(&root, "custom-group");
        assert!(ConversationStore::open(&root).is_err());
        let store = ConversationStore::open_for_migration(&root).unwrap();
        assert_eq!(schema_version(&root), CURRENT_SCHEMA_VERSION);

        let conversation = store.get("legacy-reserved-group").unwrap();
        assert_eq!(conversation.title, DEFAULT_LOCAL_AGENT_GROUP_TITLE);
        assert!(conversation.pinned);
        assert!(conversation.is_group);
        assert_eq!(conversation.event_count, 9);
        assert_eq!(conversation.memberships.len(), 3);
        let mut by_principal = std::collections::HashMap::new();
        for membership in &conversation.memberships {
            by_principal.insert(membership.principal.id.as_str(), membership);
        }
        assert_eq!(by_principal.len(), 3);
        for principal_id in ["human:local", "agent:one", "agent:two"] {
            let membership = by_principal[principal_id];
            assert_eq!(membership.status, MembershipStatus::Active);
            assert_eq!(membership.left_at_unix_ms, None);
        }

        let remaining: Vec<(String, String, i64)> = store
            .with_connection(|connection| {
                let mut statement = connection
                    .prepare(
                        "SELECT id, kind, sequence FROM events
                         WHERE conversation_id=?1 ORDER BY sequence ASC",
                    )
                    .unwrap();
                let rows = statement
                    .query_map(params!["legacy-reserved-group"], |row| {
                        Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                    })
                    .unwrap();
                rows.collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(Into::into)
            })
            .unwrap();
        assert_eq!(
            remaining,
            vec![
                ("e1".to_owned(), "message".to_owned(), 1),
                ("e4".to_owned(), "message".to_owned(), 2),
                ("e6".to_owned(), "membership-changed".to_owned(), 3),
                ("e7".to_owned(), "membership-changed".to_owned(), 4),
                ("e8".to_owned(), "message".to_owned(), 5),
                ("e9".to_owned(), "membership-changed".to_owned(), 6),
                ("e10".to_owned(), "membership-changed".to_owned(), 7),
                ("e11".to_owned(), "membership-changed".to_owned(), 8),
                ("e12".to_owned(), "membership-changed".to_owned(), 9),
            ]
        );

        let search: Vec<(String, String)> = store
            .with_connection(|connection| {
                let mut statement = connection
                    .prepare(
                        "SELECT event_id, content FROM event_search
                         WHERE conversation_id=?1 ORDER BY rowid",
                    )
                    .unwrap();
                let rows = statement
                    .query_map(params!["legacy-reserved-group"], |row| {
                        Ok((row.get(0)?, row.get(1)?))
                    })
                    .unwrap();
                rows.collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(Into::into)
            })
            .unwrap();
        assert_eq!(
            search,
            vec![
                ("e1".to_owned(), "first hello".to_owned()),
                ("e4".to_owned(), "middle message".to_owned()),
                ("e8".to_owned(), "final hello".to_owned()),
            ]
        );

        assert_eq!(snapshot_group(&root, "custom-group"), custom_before);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn reopening_current_schema_and_legacy_import_closure_are_no_ops() {
        let root = std::env::temp_dir().join(format!("lico-conv-v4-{}", Uuid::new_v4()));
        std::fs::create_dir_all(fixture_database(&root).parent().unwrap()).unwrap();
        let fixture = open_fixture_connection(&root);
        seed_v2_database(&fixture, false);
        fixture.close().unwrap();

        assert!(ConversationStore::open(&root).is_err());
        let store = ConversationStore::open_for_migration(&root).unwrap();
        let after_first_open = snapshot_database(&root);
        drop(store);
        let reopened = ConversationStore::open(&root).unwrap();
        assert_eq!(snapshot_database(&root), after_first_open);
        reopened
            .normalize_reserved_default_group_after_legacy_import()
            .unwrap();
        assert_eq!(snapshot_database(&root), after_first_open);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn failed_v3_migration_rolls_back_and_retry_succeeds() {
        let root = std::env::temp_dir().join(format!("lico-conv-rollback-{}", Uuid::new_v4()));
        std::fs::create_dir_all(fixture_database(&root).parent().unwrap()).unwrap();
        let fixture = open_fixture_connection(&root);
        seed_v2_database(&fixture, true);
        fixture.close().unwrap();

        assert!(ConversationStore::open_for_migration(&root).is_err());
        assert_eq!(schema_version(&root), "2");
        let check = open_fixture_connection(&root);
        let event_count: i64 = check
            .query_row(
                "SELECT COUNT(*) FROM events WHERE conversation_id='legacy-reserved-group'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(event_count, 12);
        let m_two: String = check
            .query_row(
                "SELECT status FROM memberships WHERE id='m-two'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(m_two, "left");
        let automatic_survived: i64 = check
            .query_row(
                "SELECT COUNT(*) FROM events WHERE id IN ('e2','e3','e5')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(automatic_survived, 3);
        check
            .execute_batch(
                "DROP TABLE schema_meta;
                 CREATE TABLE schema_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
                 INSERT INTO schema_meta(key, value) VALUES ('version', '2');",
            )
            .unwrap();
        drop(check);

        let store = ConversationStore::open_for_migration(&root).unwrap();
        assert_eq!(schema_version(&root), CURRENT_SCHEMA_VERSION);
        let conversation = store.get("legacy-reserved-group").unwrap();
        assert_eq!(conversation.event_count, 9);
        for membership in conversation.memberships {
            assert_eq!(membership.status, MembershipStatus::Active);
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn published_store_upgrade_creates_dispatch_delivery_storage() {
        let root =
            std::env::temp_dir().join(format!("lico-conv-published-delivery-{}", Uuid::new_v4()));
        std::fs::create_dir_all(fixture_database(&root).parent().unwrap()).unwrap();
        let store = ConversationStore::open_for_migration(&root).unwrap();
        assert_eq!(schema_version(&root), CURRENT_SCHEMA_VERSION);
        drop(store);

        let fixture = open_fixture_connection(&root);
        fixture
            .execute_batch(
                "DROP TABLE subagent_dispatch_deliveries;
                 UPDATE schema_meta SET value='12' WHERE key='version';",
            )
            .unwrap();
        fixture.close().unwrap();

        assert!(ConversationStore::open(&root).is_err());
        let store = ConversationStore::open_for_migration(&root).unwrap();
        store
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT COUNT(*) FROM subagent_dispatch_deliveries",
                        [],
                        |row| row.get::<_, i64>(0),
                    )
                    .unwrap();
                Ok(())
            })
            .unwrap();
        drop(store);
        ConversationStore::open(&root).unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn migrates_v5_conversations_to_durable_strategy_selection() {
        let root = std::env::temp_dir().join(format!("lico-conv-v5-{}", Uuid::new_v4()));
        std::fs::create_dir_all(fixture_database(&root).parent().unwrap()).unwrap();
        let fixture = open_fixture_connection(&root);
        fixture
            .execute_batch(&released_source::supported_conversation_schema(5))
            .unwrap();
        fixture
            .execute_batch(
                "INSERT INTO schema_meta(key, value) VALUES ('version', '5');
                 INSERT INTO conversations(id,title,created_at,updated_at)
                 VALUES ('retained-conversation','Synthetic retained title',1,1);",
            )
            .unwrap();
        fixture.close().unwrap();

        assert!(ConversationStore::open(&root).is_err());
        let store = ConversationStore::open_for_migration(&root).unwrap();
        assert_eq!(schema_version(&root), CURRENT_SCHEMA_VERSION);
        let has_strategy_revision = store
            .with_connection(|connection| {
                let mut statement = connection.prepare("PRAGMA table_info(conversations)")?;
                Ok(statement
                    .query_map([], |row| row.get::<_, String>(1))?
                    .collect::<rusqlite::Result<Vec<_>>>()?
                    .iter()
                    .any(|column| column == "strategy_revision"))
            })
            .unwrap();
        assert!(has_strategy_revision);
        assert_eq!(
            store.get("retained-conversation").unwrap().title,
            "Synthetic retained title"
        );

        drop(store);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn fresh_install_stays_empty_and_closure_is_a_no_op() {
        let root = std::env::temp_dir().join(format!("lico-conv-fresh-{}", Uuid::new_v4()));
        let store = ConversationStore::open(&root).unwrap();
        assert!(store.list(false).unwrap().is_empty());
        assert!(store.get(DEFAULT_LOCAL_AGENT_GROUP_ID).is_err());
        store
            .normalize_reserved_default_group_after_legacy_import()
            .unwrap();
        assert!(store.list(false).unwrap().is_empty());
        assert!(store.get(DEFAULT_LOCAL_AGENT_GROUP_ID).is_err());
        assert_eq!(schema_version(&root), CURRENT_SCHEMA_VERSION);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn default_local_group_adopts_legacy_identity_without_losing_content() {
        let store = ConversationStore::open_in_memory().unwrap();
        let legacy = store
            .create_group_with_id("legacy-local-group", "Lico", owner())
            .unwrap();
        store
            .source_link(&legacy.id, "group", DEFAULT_LOCAL_AGENT_GROUP_ID)
            .unwrap();
        let owner = legacy.memberships[0].id.clone();
        store
            .append_event(
                &legacy.id,
                Some(&owner),
                EventKind::Message,
                &[NewEventPart {
                    id: String::new(),
                    kind: EventPartKind::Text,
                    content: "kept".into(),
                }],
                None,
                None,
                true,
            )
            .unwrap();

        let local = store.ensure_default_local_group().unwrap();

        assert_eq!(local.id, DEFAULT_LOCAL_AGENT_GROUP_ID);
        assert_eq!(local.title, DEFAULT_LOCAL_AGENT_GROUP_TITLE);
        assert_eq!(local.event_count, 1);
        assert_eq!(local.memberships.len(), 1);
        assert_eq!(store.list(false).unwrap().len(), 1);
        assert!(store.get("legacy-local-group").is_err());
        assert_eq!(
            store
                .page_events(DEFAULT_LOCAL_AGENT_GROUP_ID, None, 10)
                .unwrap()
                .events[0]
                .parts[0]
                .content,
            "kept"
        );
    }

    #[test]
    fn runtime_message_chunks_become_visible_text_parts() {
        let store = ConversationStore::open_in_memory().unwrap();
        let conversation = store.create_conversation("Project", owner()).unwrap();
        let agent = store
            .add_member(
                &conversation.id,
                Principal {
                    id: "agent:one".into(),
                    kind: PrincipalKind::Agent,
                    display_name: "One".into(),
                    agent_id: Some("one".into()),
                    created_at_unix_ms: 1,
                },
                MembershipAccess::Member,
            )
            .unwrap();
        let scope = store
            .prepare_runtime_dispatch(
                "one",
                "",
                "hello",
                Some(&conversation.id),
                Some(&agent.id),
                Some("event:user"),
                None,
            )
            .unwrap();
        store
            .append_runtime_frame(
                &scope,
                1,
                &serde_json::json!({
                    "event": "agent.message.chunk",
                    "sessionId": "session-1",
                    "turnId": "turn-1",
                    "payload": {"messageUnit": "1", "text": "Hel"}
                }),
            )
            .unwrap();
        store
            .append_runtime_frame(
                &scope,
                2,
                &serde_json::json!({
                    "event": "agent.message.chunk",
                    "sessionId": "session-1",
                    "turnId": "turn-1",
                    "payload": {"messageUnit": "1", "text": "l"}
                }),
            )
            .unwrap();
        store
            .append_runtime_frame(
                &scope,
                3,
                &serde_json::json!({
                    "event": "agent.message.completed",
                    "sessionId": "session-1",
                    "turnId": "turn-1",
                    "payload": {"messageUnit": "1", "text": "Hello"}
                }),
            )
            .unwrap();
        store
            .append_runtime_frame(
                &scope,
                4,
                &serde_json::json!({
                    "event": "agent.message.chunk",
                    "sessionId": "session-1",
                    "turnId": "turn-1",
                    "payload": {"messageUnit": "2", "text": "Wor"}
                }),
            )
            .unwrap();
        store
            .append_runtime_frame(
                &scope,
                5,
                &serde_json::json!({
                    "event": "agent.message.completed",
                    "sessionId": "session-1",
                    "turnId": "turn-1",
                    "payload": {"messageUnit": "2", "text": "World"}
                }),
            )
            .unwrap();
        store
            .finish_runtime_dispatch(
                &scope,
                &serde_json::json!({"output": "World"}),
                DispatchState::Completed,
                None,
            )
            .unwrap();
        let event = store
            .page_events(&conversation.id, None, 50)
            .unwrap()
            .events
            .into_iter()
            .find(|candidate| candidate.id == scope.event_id)
            .unwrap();
        let text = event
            .parts
            .iter()
            .filter(|part| part.kind == EventPartKind::Text)
            .map(|part| part.content.as_str())
            .collect::<String>();
        assert_eq!(text, "HelloWorld");
        // Canonical Event Parts stay intact: each chunk keeps its own part
        // identity and ordinal instead of being folded into one aggregate
        // part at readback time.
        let text_parts: Vec<_> = event
            .parts
            .iter()
            .filter(|part| part.kind == EventPartKind::Text)
            .collect();
        assert_eq!(text_parts.len(), 5);
        assert_eq!(text_parts[0].content, "Hel");
        assert_eq!(text_parts[1].content, "l");
        assert_eq!(text_parts[2].content, "o");
        assert_eq!(text_parts[3].content, "Wor");
        assert_eq!(text_parts[4].content, "ld");
        assert!(text_parts[0].ordinal < text_parts[1].ordinal);
        assert_ne!(text_parts[0].id, text_parts[1].id);
        assert_eq!(
            event
                .parts
                .iter()
                .filter(|part| part.content == r#"{"messageUnit":"1"}"#)
                .count(),
            1
        );
        assert_eq!(
            event
                .parts
                .iter()
                .filter(|part| part.content == r#"{"messageUnit":"2"}"#)
                .count(),
            1
        );
    }

    #[test]
    fn user_message_projection_does_not_mix_into_agent_event() {
        let store = ConversationStore::open_in_memory().unwrap();
        let conversation = store.create_conversation("Project", owner()).unwrap();
        let agent = store
            .add_member(
                &conversation.id,
                Principal {
                    id: "agent:one".into(),
                    kind: PrincipalKind::Agent,
                    display_name: "One".into(),
                    agent_id: Some("one".into()),
                    created_at_unix_ms: 1,
                },
                MembershipAccess::Member,
            )
            .unwrap();
        let human = conversation.memberships[0].id.clone();
        let user_event = store
            .post_message_with_attachments(
                &conversation.id,
                Some(&human),
                "follow up",
                None,
                &[],
                &[],
            )
            .unwrap()
            .0;
        let scope = store
            .prepare_runtime_dispatch(
                "one",
                "",
                "follow up",
                Some(&conversation.id),
                Some(&agent.id),
                Some(&user_event.id),
                None,
            )
            .unwrap();
        store
            .append_runtime_frame(
                &scope,
                1,
                &serde_json::json!({
                    "event": "agent.message.chunk",
                    "sessionId": "session-1",
                    "turnId": "turn-1",
                    "payload": {"messageUnit": "1", "text": "Reply"}
                }),
            )
            .unwrap();
        store
            .append_runtime_frame(
                &scope,
                2,
                &serde_json::json!({
                    "event": crate::projection::USER_MESSAGE_EVENT_KIND,
                    "sessionId": "session-1",
                    "turnId": "turn-1",
                    "payload": {
                        "text": "follow up",
                        "role": "user",
                        "lifecyclePrefix": ["submitted"]
                    }
                }),
            )
            .unwrap();
        store
            .finish_runtime_dispatch(
                &scope,
                &serde_json::json!({"output": "Reply"}),
                DispatchState::Completed,
                None,
            )
            .unwrap();

        let events = store
            .page_events(&conversation.id, None, 50)
            .unwrap()
            .events;
        let user = events
            .iter()
            .find(|event| event.id == user_event.id)
            .unwrap();
        let assistant = events
            .iter()
            .find(|event| event.id == scope.event_id)
            .unwrap();
        assert_eq!(user.author_membership_id.as_deref(), Some(human.as_str()));
        assert!(user.finalized);
        assert_eq!(user.parts[0].kind, EventPartKind::Text);
        assert_eq!(user.parts[0].content, "follow up");
        assert_eq!(
            assistant.author_membership_id.as_deref(),
            Some(agent.id.as_str())
        );
        assert!(assistant.finalized);
        let assistant_text: String = assistant
            .parts
            .iter()
            .filter(|part| part.kind == EventPartKind::Text)
            .map(|part| part.content.as_str())
            .collect();
        assert_eq!(assistant_text, "Reply");
        assert!(assistant.parts.iter().all(|part| {
            !part
                .content
                .contains(crate::projection::USER_MESSAGE_EVENT_KIND)
                && part.content != "follow up"
        }));
    }

    #[test]
    fn rejoining_member_reuses_the_left_membership_id() {
        let store = ConversationStore::open_in_memory().unwrap();
        let conversation = store.create_conversation("Project", owner()).unwrap();
        let principal = Principal {
            id: "agent:one".into(),
            kind: PrincipalKind::Agent,
            display_name: "One".into(),
            agent_id: Some("one".into()),
            created_at_unix_ms: 1,
        };
        let first = store
            .add_member(
                &conversation.id,
                principal.clone(),
                MembershipAccess::Member,
            )
            .unwrap();
        store.leave_member(&conversation.id, &first.id).unwrap();
        let second = store
            .add_member(&conversation.id, principal, MembershipAccess::Member)
            .unwrap();
        assert_eq!(second.id, first.id);
        assert_eq!(second.status, MembershipStatus::Active);
        assert_eq!(second.left_at_unix_ms, None);
        let matching = store
            .get(&conversation.id)
            .unwrap()
            .memberships
            .into_iter()
            .filter(|membership| membership.principal.id == "agent:one")
            .collect::<Vec<_>>();
        assert_eq!(matching.len(), 1);
        assert_eq!(matching[0].id, first.id);
    }

    #[test]
    fn v12_collapses_duplicate_left_and_active_memberships() {
        let root = std::env::temp_dir().join(format!("lico-conv-v12-{}", Uuid::new_v4()));
        path_security::ensure_private_dir(&root).unwrap();
        let store = ConversationStore::open(&root).unwrap();
        let conversation = store.create_conversation("Project", owner()).unwrap();
        let conversation_id = conversation.id.clone();
        let agent = store
            .add_member(
                &conversation_id,
                Principal {
                    id: "agent:one".into(),
                    kind: PrincipalKind::Agent,
                    display_name: "One".into(),
                    agent_id: Some("one".into()),
                    created_at_unix_ms: 1,
                },
                MembershipAccess::Member,
            )
            .unwrap();
        let leftover_id = format!("membership:{}", Uuid::new_v4());
        store
            .with_connection(|connection| {
                connection.execute("UPDATE schema_meta SET value='11' WHERE key='version'", [])?;
                connection.execute_batch(
                    "DROP INDEX IF EXISTS memberships_principal_unique;
                     CREATE UNIQUE INDEX IF NOT EXISTS memberships_active_unique
                       ON memberships(conversation_id, principal_id)
                       WHERE status='active';",
                )?;
                connection.execute(
                    "INSERT INTO memberships(
                       id, conversation_id, principal_id, access, status, joined_at, left_at
                     ) VALUES (?1, ?2, 'agent:one', 'member', 'left', 1, 2)",
                    params![leftover_id, conversation_id],
                )?;
                Ok(())
            })
            .unwrap();
        drop(store);

        assert!(ConversationStore::open(&root).is_err());
        let migrated = ConversationStore::open_for_migration(&root).unwrap();
        assert_eq!(schema_version(&root), CURRENT_SCHEMA_VERSION);
        let matching = migrated
            .get(&conversation_id)
            .unwrap()
            .memberships
            .into_iter()
            .filter(|membership| membership.principal.id == "agent:one")
            .collect::<Vec<_>>();
        assert_eq!(matching.len(), 1);
        assert_eq!(matching[0].id, agent.id);
        assert_eq!(matching[0].status, MembershipStatus::Active);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn completed_snapshot_falls_back_to_terminal_output_without_chunks() {
        let store = ConversationStore::open_in_memory().unwrap();
        let conversation = store.create_conversation("Project", owner()).unwrap();
        let agent = store
            .add_member(
                &conversation.id,
                Principal {
                    id: "agent:one".into(),
                    kind: PrincipalKind::Agent,
                    display_name: "One".into(),
                    agent_id: Some("one".into()),
                    created_at_unix_ms: 1,
                },
                MembershipAccess::Member,
            )
            .unwrap();
        let scope = store
            .prepare_runtime_dispatch(
                "one",
                "",
                "hello",
                Some(&conversation.id),
                Some(&agent.id),
                Some("event:user"),
                None,
            )
            .unwrap();
        store
            .append_runtime_frame(
                &scope,
                1,
                &serde_json::json!({
                    "event": "agent.message.completed",
                    "sessionId": "session-1",
                    "turnId": "turn-1",
                    "payload": {"text": "Complete"}
                }),
            )
            .unwrap();
        store
            .finish_runtime_dispatch(
                &scope,
                &serde_json::json!({}),
                DispatchState::Completed,
                None,
            )
            .unwrap();

        let event = store
            .page_events(&conversation.id, None, 50)
            .unwrap()
            .events
            .into_iter()
            .find(|candidate| candidate.id == scope.event_id)
            .unwrap();
        let text_parts: Vec<_> = event
            .parts
            .iter()
            .filter(|part| part.kind == EventPartKind::Text)
            .collect();
        assert_eq!(text_parts.len(), 1);
        assert_eq!(text_parts[0].content, "Complete");
    }

    #[test]
    fn failed_runtime_terminal_keeps_turn_status_and_skips_generic_failed_frame() {
        let store = ConversationStore::open_in_memory().unwrap();
        let conversation = store.create_conversation("Project", owner()).unwrap();
        let agent = store
            .add_member(
                &conversation.id,
                Principal {
                    id: "agent:one".into(),
                    kind: PrincipalKind::Agent,
                    display_name: "One".into(),
                    agent_id: Some("one".into()),
                    created_at_unix_ms: 1,
                },
                MembershipAccess::Member,
            )
            .unwrap();
        let scope = store
            .prepare_runtime_dispatch(
                "one",
                "",
                "hello",
                Some(&conversation.id),
                Some(&agent.id),
                Some("event:user"),
                None,
            )
            .unwrap();
        store
            .append_runtime_frame(
                &scope,
                1,
                &serde_json::json!({
                    "event": "agent.turn.accepted",
                    "sessionId": "session-1",
                    "turnId": "turn-1",
                    "payload": {
                        "lifecyclePrefix": ["submitted", "accepted"]
                    }
                }),
            )
            .unwrap();
        store
            .append_runtime_frame(
                &scope,
                2,
                &serde_json::json!({
                    "event": "dispatch.turn.failed",
                    "sessionId": "session-1",
                    "turnId": "turn-1",
                    "payload": {
                        "turnStatus": "failed/UsageLimitExceeded",
                        "code": "codex_usage_limit_exceeded"
                    }
                }),
            )
            .unwrap();
        store
            .finish_runtime_dispatch(
                &scope,
                &serde_json::json!({
                    "ok": false,
                    "turnStatus": "failed/UsageLimitExceeded",
                    "error": {
                        "code": "codex_usage_limit_exceeded",
                        "stage": "turn/completed",
                        "component": "native_cli",
                        "retryable": false,
                        "recovery": "select_available_model_or_wait_for_quota_reset",
                        "rootCause": "quota",
                        "turnStatus": "failed/UsageLimitExceeded",
                        "message": "turn-fixture/secret.txt is unreadable"
                    }
                }),
                DispatchState::Failed,
                Some("codex_usage_limit_exceeded"),
            )
            .unwrap();
        let event = store
            .page_events(&conversation.id, None, 50)
            .unwrap()
            .events
            .into_iter()
            .find(|candidate| candidate.id == scope.event_id)
            .unwrap();
        let diagnostics: Vec<_> = event
            .parts
            .iter()
            .filter(|part| part.kind == EventPartKind::Diagnostic)
            .map(|part| part.content.as_str())
            .collect();
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].contains("codex_usage_limit_exceeded"));
        assert!(diagnostics[0].contains("turn/completed"));
        assert!(diagnostics[0].contains("failed/UsageLimitExceeded"));
        assert!(diagnostics[0].contains("native_cli"));
        assert!(diagnostics[0].contains("\"retryable\":false"));
        assert!(diagnostics[0].contains("select_available_model_or_wait_for_quota_reset"));
        assert!(diagnostics[0].contains("\"rootCause\":\"quota\""));
        assert!(!diagnostics[0].contains("secret.txt"));
        assert!(!diagnostics[0].contains("turn-fixture"));
        assert!(!diagnostics.contains(&"runtime diagnostic"));
        assert!(event.parts.iter().any(|part| {
            part.kind == EventPartKind::Metadata && part.content.contains("failed")
        }));
        assert!(event.parts.iter().any(|part| {
            part.kind == EventPartKind::Metadata && part.content.contains("accepted")
        }));
    }

    #[test]
    fn terminal_diagnostic_root_cause_is_allow_listed() {
        let carried = runtime_terminal_diagnostic(
            &serde_json::json!({
                "error": {
                    "code": "antigravity_hook_receipt_missing",
                    "stage": "session/new",
                    "rootCause": "env_mismatch",
                    "recovery": "subagent_env_mismatch: LicoUp-launched CLI environment differs from user terminal"
                }
            }),
            None,
        );
        assert!(carried.contains("\"rootCause\":\"env_mismatch\""));
        assert!(carried.contains("subagent_env_mismatch"));

        let dropped = runtime_terminal_diagnostic(
            &serde_json::json!({
                "error": {
                    "code": "agent_conversation_dispatch_failed",
                    "rootCause": "unreviewed-root-cause-text",
                    "recovery": "unreviewed-recovery-text"
                }
            }),
            None,
        );
        assert!(!dropped.contains("rootCause"));
        assert!(!dropped.contains("unreviewed"));
    }

    #[test]
    fn permission_denied_frame_projects_typed_diagnostic_json() {
        let store = ConversationStore::open_in_memory().unwrap();
        let conversation = store.create_conversation("Project", owner()).unwrap();
        let agent = store
            .add_member(
                &conversation.id,
                Principal {
                    id: "agent:one".into(),
                    kind: PrincipalKind::Agent,
                    display_name: "One".into(),
                    agent_id: Some("one".into()),
                    created_at_unix_ms: 1,
                },
                MembershipAccess::Member,
            )
            .unwrap();
        let scope = store
            .prepare_runtime_dispatch(
                "one",
                "",
                "hello",
                Some(&conversation.id),
                Some(&agent.id),
                Some("event:user"),
                None,
            )
            .unwrap();
        store
            .append_runtime_frame(
                &scope,
                1,
                &serde_json::json!({
                    "event": "permission.denied",
                    "sessionId": "session-1",
                    "turnId": "turn-1",
                    "payload": {"text": "permission denied"}
                }),
            )
            .unwrap();
        let event = store
            .page_events(&conversation.id, None, 50)
            .unwrap()
            .events
            .into_iter()
            .find(|candidate| candidate.id == scope.event_id)
            .unwrap();
        let diagnostics: Vec<_> = event
            .parts
            .iter()
            .filter(|part| part.kind == EventPartKind::Diagnostic)
            .map(|part| part.content.as_str())
            .collect();
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].contains("permission_denied"));
        assert!(!diagnostics.contains(&"permission denied"));
        assert!(!diagnostics.contains(&"runtime diagnostic"));
    }

    #[test]
    fn three_member_tool_error_stays_tool_result_and_skips_terminal_diagnostic() {
        let store = ConversationStore::open_in_memory().unwrap();
        let conversation = store
            .create_conversation_with_members(
                "Mesh",
                owner(),
                &[
                    (agent("cursor", "cursor"), MembershipAccess::Member),
                    (agent("codex", "codex"), MembershipAccess::Member),
                    (
                        agent("antigravity", "antigravity"),
                        MembershipAccess::Member,
                    ),
                ],
            )
            .unwrap();
        assert_eq!(conversation.memberships.len(), 4);
        let caller = conversation
            .memberships
            .iter()
            .find(|membership| membership.principal.agent_id.as_deref() == Some("cursor"))
            .unwrap();
        let scope = store
            .prepare_runtime_dispatch(
                "cursor",
                "",
                "delegate",
                Some(&conversation.id),
                Some(&caller.id),
                Some("event:user"),
                None,
            )
            .unwrap();
        store
            .append_runtime_frame(
                &scope,
                1,
                &serde_json::json!({
                    "event": "agent.turn.processing",
                    "sessionId": "session-1",
                    "turnId": "turn-1",
                    "payload": {
                        "evidenceKind": "tool",
                        "toolName": "lico_subagent_delegate"
                    }
                }),
            )
            .unwrap();
        store
            .append_runtime_frame(
                &scope,
                2,
                &serde_json::json!({
                    "event": "agent.tool.result",
                    "sessionId": "session-1",
                    "turnId": "turn-1",
                    "payload": {
                        "text": "caller_membership_not_authorized",
                        "toolName": "lico_subagent_delegate",
                        "status": "error"
                    }
                }),
            )
            .unwrap();
        store
            .finish_runtime_dispatch(
                &scope,
                &serde_json::json!({
                    "ok": false,
                    "error": {
                        "code": "cursor_cli_turn_failed",
                        "stage": "turn/completed"
                    }
                }),
                DispatchState::Failed,
                Some("cursor_cli_turn_failed"),
            )
            .unwrap();
        let event = store
            .page_events(&conversation.id, None, 50)
            .unwrap()
            .events
            .into_iter()
            .find(|candidate| candidate.id == scope.event_id)
            .unwrap();
        assert!(event.parts.iter().any(|part| {
            part.kind == EventPartKind::ToolCall && part.content == "lico_subagent_delegate"
        }));
        assert!(event.parts.iter().any(|part| {
            part.kind == EventPartKind::ToolResult
                && part.content == "caller_membership_not_authorized"
        }));
        assert!(
            !event
                .parts
                .iter()
                .any(|part| part.kind == EventPartKind::Diagnostic)
        );
        assert!(event.parts.iter().any(|part| {
            part.kind == EventPartKind::Metadata && part.content.contains("failed")
        }));
    }

    fn agent(agent_id: &str, display_name: &str) -> Principal {
        Principal {
            id: agent_id.into(),
            kind: PrincipalKind::Agent,
            display_name: display_name.into(),
            agent_id: Some(agent_id.into()),
            created_at_unix_ms: 1,
        }
    }

    #[test]
    fn assistant_designation_and_profile_intent_are_persistent_and_idempotent() {
        let store = ConversationStore::open_in_memory().unwrap();
        let conversation = store
            .create_conversation_with_members(
                "Group",
                owner(),
                &[
                    (agent("agent:one", "One"), MembershipAccess::Member),
                    (agent("agent:two", "Two"), MembershipAccess::Member),
                ],
            )
            .unwrap();
        let agent_ids = conversation
            .memberships
            .iter()
            .filter(|membership| membership.principal.kind == PrincipalKind::Agent)
            .map(|membership| membership.id.clone())
            .collect::<Vec<_>>();
        let owner_membership_id = conversation
            .memberships
            .iter()
            .find(|membership| {
                membership.principal.kind == PrincipalKind::Human
                    && membership.access == MembershipAccess::Owner
            })
            .unwrap()
            .id
            .clone();
        assert_eq!(agent_ids.len(), 2);
        for membership_id in &agent_ids {
            let profile = store.membership_profile(membership_id).unwrap().unwrap();
            assert_eq!(profile.revision, 0);
        }

        store
            .set_conversation_assistant(
                &conversation.id,
                &owner_membership_id,
                conversation.revision,
                Some(&agent_ids[0]),
            )
            .unwrap();
        let conversation = store.get(&conversation.id).unwrap();
        assert_eq!(
            conversation.assistant_membership_id.as_deref(),
            Some(agent_ids[0].as_str())
        );
        let revision_after_designation = conversation.revision;
        store
            .set_conversation_assistant(
                &conversation.id,
                &owner_membership_id,
                conversation.revision - 1,
                Some(&agent_ids[0]),
            )
            .unwrap();
        assert_eq!(
            store.get(&conversation.id).unwrap().revision,
            revision_after_designation
        );
        store
            .set_conversation_assistant(
                &conversation.id,
                &owner_membership_id,
                revision_after_designation,
                None,
            )
            .unwrap();
        assert!(
            store
                .get(&conversation.id)
                .unwrap()
                .assistant_membership_id
                .is_none()
        );

        let update = ProfileIntentUpdate {
            required_capabilities: vec!["workspace".into()],
            preferred_model: Some("model-a".into()),
            preferred_reasoning_effort: Some("high".into()),
            preferred_environment: Some("local".into()),
            ..Default::default()
        };
        let profile_revision = store
            .membership_profile(&agent_ids[0])
            .unwrap()
            .unwrap()
            .revision;
        let intent = store
            .set_membership_profile(
                &conversation.id,
                &agent_ids[0],
                &owner_membership_id,
                profile_revision,
                &update,
            )
            .unwrap();
        let replayed = store
            .set_membership_profile(
                &conversation.id,
                &agent_ids[0],
                &owner_membership_id,
                profile_revision,
                &update,
            )
            .unwrap();
        assert_eq!(intent, replayed);
        assert_eq!(intent.revision, profile_revision + 1);
        assert_eq!(
            store.membership_profile(&agent_ids[0]).unwrap().unwrap(),
            intent
        );
        let stale = ProfileIntentUpdate {
            preferred_model: Some("model-b".into()),
            ..Default::default()
        };
        assert_eq!(
            store
                .set_membership_profile(
                    &conversation.id,
                    &agent_ids[0],
                    &owner_membership_id,
                    profile_revision,
                    &stale,
                )
                .unwrap_err()
                .to_string(),
            "profile_revision_stale"
        );
        let profiles = store.membership_profiles(&conversation.id).unwrap();
        assert_eq!(profiles.len(), 2);
        assert!(
            profiles
                .iter()
                .any(|(_, profile)| profile.revision == profile_revision + 1)
        );
        assert!(profiles.iter().any(|(_, profile)| profile.revision == 0));
    }

    #[test]
    fn migrates_v6_conversations_to_assistant_and_profile_state_without_assigning_assistants() {
        let root = std::env::temp_dir().join(format!("lico-conv-v6-assistant-{}", Uuid::new_v4()));
        std::fs::create_dir_all(fixture_database(&root).parent().unwrap()).unwrap();
        let fixture = open_fixture_connection(&root);
        fixture
            .execute_batch(&released_source::supported_conversation_schema(6))
            .unwrap();
        fixture
            .execute_batch(
                "INSERT INTO schema_meta(key, value) VALUES ('version', '6');
                 INSERT INTO principals(id, kind, display_name, agent_id, created_at)
                   VALUES ('agent:one', 'agent', 'One', 'one', 1),
                          ('human:local', 'human', 'You', NULL, 1);
                 INSERT INTO conversations(id, title, archived, pinned, is_group, revision, created_at, updated_at)
                   VALUES ('legacy-group', 'Legacy', 0, 0, 1, 0, 1, 1);
                 INSERT INTO memberships(id, conversation_id, principal_id, access, status, joined_at)
                   VALUES ('m-human', 'legacy-group', 'human:local', 'owner', 'active', 1),
                          ('m-agent', 'legacy-group', 'agent:one', 'member', 'active', 1);",
            )
            .unwrap();
        fixture.close().unwrap();

        assert!(ConversationStore::open(&root).is_err());
        let store = ConversationStore::open_for_migration(&root).unwrap();
        assert_eq!(schema_version(&root), CURRENT_SCHEMA_VERSION);
        let conversation = store.get("legacy-group").unwrap();
        assert!(conversation.assistant_membership_id.is_none());
        let profiles = store.membership_profiles("legacy-group").unwrap();
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].1.revision, 0);
        assert_eq!(profiles[0].1.required_capabilities.len(), 0);

        drop(store);
        let reopened = ConversationStore::open(&root).unwrap();
        assert_eq!(schema_version(&root), CURRENT_SCHEMA_VERSION);
        assert_eq!(
            reopened.membership_profiles("legacy-group").unwrap().len(),
            1
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
