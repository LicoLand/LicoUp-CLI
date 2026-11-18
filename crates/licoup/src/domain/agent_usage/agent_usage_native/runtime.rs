//! Bounded SQLite connection ownership for native usage caches.
//!
//! Long-running report and scan commands reuse a small process-wide pool:
//! two configured connections per active usage scope, retaining the four
//! most recently used scopes. Leases are short-lived and never span file
//! discovery, guard hashing, or parsing.

use super::cache::open_cache_database;
use anyhow::Result;
use rusqlite::Connection;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex, MutexGuard};

pub(super) const ACTIVE_SCOPE_POOLS: usize = 4;
pub(super) const CONNECTIONS_PER_SCOPE: usize = 2;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct LeaseStats {
    pub(super) opened: u64,
    pub(super) leases: u64,
}

pub struct CacheRuntime {
    inner: Mutex<RuntimeState>,
    scope_available: Condvar,
}

struct RuntimeState {
    pools: BTreeMap<PoolKey, RootPool>,
}

/// A logical scan scope is not sufficient pool identity: callers can use the
/// same history roots and time window with an isolated client-state root. The
/// database path therefore participates in identity so connections can never
/// cross that isolation boundary.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct PoolKey {
    scope_key: String,
    database_path: PathBuf,
}

impl PoolKey {
    fn new(scope_key: &str, database_path: &Path) -> Self {
        Self {
            scope_key: scope_key.to_owned(),
            database_path: database_path.to_owned(),
        }
    }
}

struct RootPool {
    connections: Vec<Connection>,
    last_used_ms: u64,
    active_refreshes: usize,
}

impl CacheRuntime {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(RuntimeState {
                pools: BTreeMap::new(),
            }),
            scope_available: Condvar::new(),
        }
    }

    pub(super) fn begin_refresh(
        &self,
        scope_key: &str,
        database_path: &Path,
        now_ms: u64,
    ) -> Result<RefreshScope<'_>> {
        let pool_key = PoolKey::new(scope_key, database_path);
        let mut state = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("native usage cache runtime lock poisoned"))?;
        loop {
            if state.pools.contains_key(&pool_key) {
                break;
            }
            if state.pools.len() < ACTIVE_SCOPE_POOLS {
                state.pools.insert(
                    pool_key.clone(),
                    RootPool {
                        connections: Vec::new(),
                        last_used_ms: 0,
                        active_refreshes: 0,
                    },
                );
                break;
            }
            let evict = state
                .pools
                .iter()
                .filter(|(_, pool)| pool.active_refreshes == 0)
                .min_by_key(|(_, pool)| pool.last_used_ms)
                .map(|(key, _)| key.clone());
            if let Some(key) = evict {
                state.pools.remove(&key);
                state.pools.insert(
                    pool_key.clone(),
                    RootPool {
                        connections: Vec::new(),
                        last_used_ms: 0,
                        active_refreshes: 0,
                    },
                );
                break;
            }
            state = self
                .scope_available
                .wait(state)
                .map_err(|_| anyhow::anyhow!("native usage cache runtime lock poisoned"))?;
        }

        let pool = state
            .pools
            .get_mut(&pool_key)
            .expect("native usage refresh scope exists");
        pool.last_used_ms = now_ms;
        let mut opened = 0_u64;
        while pool.connections.len() < CONNECTIONS_PER_SCOPE {
            pool.connections.push(open_cache_database(database_path)?);
            opened = opened.saturating_add(1);
        }
        pool.active_refreshes = pool.active_refreshes.saturating_add(1);
        Ok(RefreshScope {
            runtime: self,
            pool_key,
            opened,
        })
    }

    pub(super) fn lease(
        &self,
        scope_key: &str,
        database_path: &Path,
        now_ms: u64,
    ) -> Result<CacheLease<'_>> {
        let pool_key = PoolKey::new(scope_key, database_path);
        let mut state = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("native usage cache runtime lock poisoned"))?;
        if !state.pools.contains_key(&pool_key) && state.pools.len() >= ACTIVE_SCOPE_POOLS {
            let evict = state
                .pools
                .iter()
                .filter(|(_, pool)| pool.active_refreshes == 0)
                .min_by_key(|(_, pool)| pool.last_used_ms)
                .map(|(key, _)| key.clone());
            if let Some(key) = evict {
                state.pools.remove(&key);
            } else {
                anyhow::bail!("native usage cache scopes are busy");
            }
        }
        let pool = state
            .pools
            .entry(pool_key.clone())
            .or_insert_with(|| RootPool {
                connections: Vec::new(),
                last_used_ms: 0,
                active_refreshes: 0,
            });
        pool.last_used_ms = now_ms;
        let mut opened = 0_u64;
        while pool.connections.len() < CONNECTIONS_PER_SCOPE {
            pool.connections.push(open_cache_database(database_path)?);
            opened = opened.saturating_add(1);
        }
        Ok(CacheLease {
            state,
            pool_key,
            opened,
            leases: 1,
        })
    }
}

pub(super) struct RefreshScope<'a> {
    runtime: &'a CacheRuntime,
    pool_key: PoolKey,
    opened: u64,
}

impl RefreshScope<'_> {
    pub(super) fn opened_connections(&self) -> u64 {
        self.opened
    }
}

impl Drop for RefreshScope<'_> {
    fn drop(&mut self) {
        let Ok(mut state) = self.runtime.inner.lock() else {
            return;
        };
        if let Some(pool) = state.pools.get_mut(&self.pool_key) {
            pool.active_refreshes = pool.active_refreshes.saturating_sub(1);
        }
        drop(state);
        self.runtime.scope_available.notify_one();
    }
}

pub(super) struct CacheLease<'a> {
    state: MutexGuard<'a, RuntimeState>,
    pool_key: PoolKey,
    opened: u64,
    leases: u64,
}

impl CacheLease<'_> {
    pub(super) fn connection(&mut self, index: usize) -> &mut Connection {
        let pool = self
            .state
            .pools
            .get_mut(&self.pool_key)
            .expect("leased usage cache pool exists");
        let bound = pool.connections.len().saturating_sub(1);
        &mut pool.connections[index.min(bound)]
    }

    pub(super) fn stats(&self) -> LeaseStats {
        LeaseStats {
            opened: self.opened,
            leases: self.leases,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static TEMP_RUNTIME_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    fn temp_database() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "lico-native-usage-runtime-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            TEMP_RUNTIME_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        root.join(super::super::cache::CACHE_FILE_NAME)
    }

    #[test]
    fn runtime_reuses_two_connections_and_evicts_least_recent_scope() {
        let runtime = CacheRuntime::new();
        let first = temp_database();
        let second = temp_database();
        let third = temp_database();
        let fourth = temp_database();
        let fifth = temp_database();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis()
            .min(u64::MAX as u128) as u64;

        let lease = runtime.lease("scope-a", &first, now).unwrap();
        assert_eq!(lease.stats().opened, 2);
        assert_eq!(lease.stats().leases, 1);
        drop(lease);

        let lease = runtime.lease("scope-a", &first, now + 1).unwrap();
        assert_eq!(lease.stats().opened, 0);
        drop(lease);

        for (index, (scope, path)) in [
            ("scope-b", &second),
            ("scope-c", &third),
            ("scope-d", &fourth),
            ("scope-e", &fifth),
        ]
        .into_iter()
        .enumerate()
        {
            let lease = runtime.lease(scope, path, now + 2 + index as u64).unwrap();
            assert_eq!(lease.stats().opened, 2);
            drop(lease);
        }

        let lease = runtime.lease("scope-a", &first, now + 10).unwrap();
        assert_eq!(lease.stats().opened, 2);
        drop(lease);
        for path in [first, second, third, fourth, fifth] {
            let _ = fs::remove_file(&path);
            let _ = fs::remove_file(format!("{}-wal", path.to_string_lossy()));
            let _ = fs::remove_file(format!("{}-shm", path.to_string_lossy()));
        }
    }

    #[test]
    fn active_refresh_scope_survives_lease_free_parsing_window() {
        let runtime = CacheRuntime::new();
        let paths = (0..5).map(|_| temp_database()).collect::<Vec<_>>();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis()
            .min(u64::MAX as u128) as u64;

        let refresh = runtime.begin_refresh("scope-a", &paths[0], now).unwrap();
        assert_eq!(refresh.opened_connections(), 2);
        for (index, scope) in ["scope-b", "scope-c", "scope-d", "scope-e"]
            .into_iter()
            .enumerate()
        {
            let lease = runtime
                .lease(scope, &paths[index + 1], now + index as u64 + 1)
                .unwrap();
            assert_eq!(lease.stats().opened, 2);
            drop(lease);
        }

        let lease = runtime.lease("scope-a", &paths[0], now + 10).unwrap();
        assert_eq!(lease.stats().opened, 0);
        drop(lease);
        drop(refresh);
        for path in paths {
            let _ = fs::remove_file(&path);
            let _ = fs::remove_file(format!("{}-wal", path.to_string_lossy()));
            let _ = fs::remove_file(format!("{}-shm", path.to_string_lossy()));
        }
    }

    #[test]
    fn identical_logical_scopes_keep_distinct_database_pools() {
        let runtime = CacheRuntime::new();
        let first = temp_database();
        let second = temp_database();

        let lease = runtime.lease("shared-scope", &first, 1).unwrap();
        assert_eq!(lease.stats().opened, CONNECTIONS_PER_SCOPE as u64);
        drop(lease);

        let lease = runtime.lease("shared-scope", &second, 2).unwrap();
        assert_eq!(lease.stats().opened, CONNECTIONS_PER_SCOPE as u64);
        drop(lease);

        let lease = runtime.lease("shared-scope", &first, 3).unwrap();
        assert_eq!(lease.stats().opened, 0);
        drop(lease);
        assert_eq!(runtime.inner.lock().unwrap().pools.len(), 2);

        for path in [first, second] {
            let _ = fs::remove_file(&path);
            let _ = fs::remove_file(format!("{}-wal", path.to_string_lossy()));
            let _ = fs::remove_file(format!("{}-shm", path.to_string_lossy()));
        }
    }
}
