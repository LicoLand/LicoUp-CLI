//! Observed availability: what a live source on this machine reported, when it
//! reported it, and what the catalogue never claims in its absence.
//!
//! An observation is evidence, not identity and not authority. It records the
//! time it was taken so a consumer can age it, and it is only ever produced by
//! a probe a composed port answered. A model the catalogue declares but this
//! host never observed has unknown availability; a model nothing declares and
//! nothing observed is unknown identity *and* unknown availability; neither is
//! ever projected as available, and no entry is synthesized from a name, a
//! price row or an intelligence score.

use crate::port::ModelCatalogPort;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

/// One model a live source reported on this machine.
///
/// The fields are the facts the observing source declared. The catalogue does
/// not complete them: an observation that names no provider stays without one,
/// and no provider is inferred from the name.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObservedModel {
    pub name: String,
    pub provider_id: Option<String>,
    pub provider: Option<String>,
    pub sources: Vec<String>,
}

/// Whether, and when, this host observed one model name.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ObservedAvailability {
    /// A live source reported the name at this time, from these sources.
    Observed {
        at_unix_ms: u64,
        sources: Vec<String>,
    },
    /// No live source has reported the name. This is not a negative fact about
    /// the model and it is never projected as available.
    Unknown,
}

impl ObservedAvailability {
    pub fn is_observed(&self) -> bool {
        matches!(self, Self::Observed { .. })
    }

    pub fn observed_at_unix_ms(&self) -> Option<u64> {
        match self {
            Self::Observed { at_unix_ms, .. } => Some(*at_unix_ms),
            Self::Unknown => None,
        }
    }
}

/// The current wall clock in milliseconds since the Unix epoch.
///
/// A clock that cannot report a time before the epoch yields zero rather than a
/// fabricated time; the observation still counts as observed, because the
/// catalogue reports the moment it was taken and never invents availability.
pub fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

/// One immutable observation of a target's models, taken at one time against
/// one owner generation.
#[derive(Clone, Debug)]
pub struct ObservedCatalog {
    pub target: String,
    pub generation: Option<String>,
    pub observed_at_unix_ms: u64,
    pub models: Vec<ObservedModel>,
}

/// Immutable observed snapshots, keyed by target and owner generation.
///
/// A snapshot is reused only while the owner publishes the same generation. An
/// owner that publishes none never has its observations cached, so an
/// unversioned host re-probes instead of serving an attachment to stale
/// availability.
#[derive(Default)]
pub struct ObservedCatalogCache {
    entries: Mutex<BTreeMap<String, CachedObservation>>,
}

struct CachedObservation {
    generation: String,
    snapshot: Arc<ObservedCatalog>,
}

impl ObservedCatalogCache {
    /// Probe one target now and cache the result under the current generation.
    pub fn observe(
        &self,
        port: &ModelCatalogPort,
        target: &str,
        params: &Value,
    ) -> anyhow::Result<Arc<ObservedCatalog>> {
        self.observe_at(port, target, params, now_unix_ms())
    }

    /// The same observation with an explicit clock, so a caller can state the
    /// time it is recording rather than read it twice.
    pub fn observe_at(
        &self,
        port: &ModelCatalogPort,
        target: &str,
        params: &Value,
        at_unix_ms: u64,
    ) -> anyhow::Result<Arc<ObservedCatalog>> {
        let generation = (port.source_generation)();
        if let Some(generation) = generation.as_deref()
            && let Some(cached) = self.cached(target, generation)
        {
            return Ok(cached);
        }
        let models = (port.observe_target_models)(target, params)?;
        let snapshot = Arc::new(ObservedCatalog {
            target: target.to_owned(),
            generation: generation.clone(),
            observed_at_unix_ms: at_unix_ms,
            models,
        });
        if let Some(generation) = generation {
            let mut entries = self
                .entries
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            entries.insert(
                target.to_owned(),
                CachedObservation {
                    generation,
                    snapshot: Arc::clone(&snapshot),
                },
            );
        }
        Ok(snapshot)
    }

    fn cached(&self, target: &str, generation: &str) -> Option<Arc<ObservedCatalog>> {
        let entries = self
            .entries
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        entries
            .get(target)
            .filter(|cached| cached.generation == generation)
            .map(|cached| Arc::clone(&cached.snapshot))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::port::{CredentialState, ModelCatalogPort};
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static EVIDENCE_PROBES: AtomicUsize = AtomicUsize::new(0);
    static GENERATION_PROBES: AtomicUsize = AtomicUsize::new(0);
    static UNGOVERNED_PROBES: AtomicUsize = AtomicUsize::new(0);

    fn observed(name: &str, source: &str) -> ObservedModel {
        ObservedModel {
            name: name.to_owned(),
            provider_id: None,
            provider: None,
            sources: vec![source.to_owned()],
        }
    }

    fn counting_probe(_target: &str, _params: &Value) -> anyhow::Result<Vec<ObservedModel>> {
        GENERATION_PROBES.fetch_add(1, Ordering::SeqCst);
        Ok(vec![observed("gpt-5.6-sol", "codex-app-server")])
    }

    fn evidence_probe(_target: &str, _params: &Value) -> anyhow::Result<Vec<ObservedModel>> {
        EVIDENCE_PROBES.fetch_add(1, Ordering::SeqCst);
        Ok(vec![observed("gpt-5.6-sol", "codex-app-server")])
    }

    fn ungoverned_probe(_target: &str, _params: &Value) -> anyhow::Result<Vec<ObservedModel>> {
        UNGOVERNED_PROBES.fetch_add(1, Ordering::SeqCst);
        Ok(vec![observed("gpt-5.6-sol", "codex-app-server")])
    }

    fn labels() -> Vec<String> {
        Vec::new()
    }

    fn credential(_provider_id: &str) -> CredentialState {
        CredentialState::Unknown
    }

    fn fixed_generation() -> Option<String> {
        Some("generation-1".to_owned())
    }

    fn port_with(
        probe: crate::port::ObserveTargetModels,
        generation: crate::port::CatalogSourceGeneration,
    ) -> ModelCatalogPort {
        ModelCatalogPort {
            agent_wrapper_labels: labels,
            observe_target_models: probe,
            provider_credential: credential,
            source_generation: generation,
        }
    }

    #[test]
    fn an_unanswered_probe_reports_no_observation_rather_than_an_empty_catalog() {
        let cache = ObservedCatalogCache::default();
        let port = ModelCatalogPort::unavailable();
        assert!(cache.observe_at(&port, "codex", &json!({}), 7).is_err());
    }

    #[test]
    fn an_observation_records_its_source_and_time() {
        let cache = ObservedCatalogCache::default();
        let port = port_with(evidence_probe, fixed_generation);
        let snapshot = cache
            .observe_at(&port, "codex", &json!({}), 1_726_000_000_000)
            .expect("the probe answers");
        assert_eq!(snapshot.observed_at_unix_ms, 1_726_000_000_000);
        assert_eq!(snapshot.target, "codex");
        assert_eq!(
            snapshot.models,
            vec![observed("gpt-5.6-sol", "codex-app-server")]
        );
    }

    #[test]
    fn one_generation_reuses_its_snapshot_and_a_new_generation_reprobes() {
        fn generation_one() -> Option<String> {
            Some("generation-1".to_owned())
        }
        fn generation_two() -> Option<String> {
            Some("generation-2".to_owned())
        }
        let cache = ObservedCatalogCache::default();
        let first = port_with(counting_probe, generation_one);
        let reused = cache.observe_at(&first, "codex", &json!({}), 1).unwrap();
        let again = cache.observe_at(&first, "codex", &json!({}), 2).unwrap();
        assert!(Arc::ptr_eq(&reused, &again));
        assert_eq!(GENERATION_PROBES.load(Ordering::SeqCst), 1);

        let next = port_with(counting_probe, generation_two);
        let reprobed = cache.observe_at(&next, "codex", &json!({}), 3).unwrap();
        assert!(!Arc::ptr_eq(&reused, &reprobed));
        assert_eq!(reprobed.observed_at_unix_ms, 3);
        assert_eq!(GENERATION_PROBES.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn an_owner_without_a_generation_is_never_cached() {
        fn no_generation() -> Option<String> {
            None
        }
        let cache = ObservedCatalogCache::default();
        let port = port_with(ungoverned_probe, no_generation);
        cache.observe_at(&port, "codex", &json!({}), 1).unwrap();
        cache.observe_at(&port, "codex", &json!({}), 2).unwrap();
        assert_eq!(UNGOVERNED_PROBES.load(Ordering::SeqCst), 2);
    }
}
