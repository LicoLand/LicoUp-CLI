use super::parameters::param_u64;
use anyhow::{Result, anyhow};
use serde_json::Value;
use std::sync::atomic::{AtomicUsize, Ordering};

const MAX_TARGET_SCAN_CONCURRENCY: usize = 8;
const DEFAULT_TARGET_SCAN_CONCURRENCY: usize = 4;

pub(super) fn target_scan_concurrency(params: &Value, task_count: usize) -> usize {
    if task_count == 0 {
        return 0;
    }
    let requested = param_u64(params, "targetScanConcurrency")
        .map(|value| usize::try_from(value).unwrap_or(MAX_TARGET_SCAN_CONCURRENCY))
        .unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(|value| value.get())
                .unwrap_or(DEFAULT_TARGET_SCAN_CONCURRENCY)
                .max(2)
        });
    requested.clamp(1, MAX_TARGET_SCAN_CONCURRENCY.min(task_count))
}

/// Run target probes with bounded parallelism while returning results in the
/// original catalog order, independent of completion timing.
pub(super) fn run_bounded_target_probes<T, R, F>(
    probes: Vec<T>,
    concurrency: usize,
    probe: F,
) -> Result<Vec<R>>
where
    T: Clone + Send + Sync,
    R: Send,
    F: Fn(T) -> Result<R> + Sync,
{
    if probes.is_empty() {
        return Ok(Vec::new());
    }
    let worker_count = concurrency.clamp(1, probes.len());
    let (result_queue, result_worker) =
        licoup_foundation::core::task_queue::bounded::<(usize, Result<R>)>(worker_count)
            .map_err(|error| anyhow!(error))?;
    let next_probe = AtomicUsize::new(0);
    let probe_count = probes.len();

    let slots = std::thread::scope(|scope| -> Result<Vec<Option<Result<R>>>> {
        let mut handles = Vec::with_capacity(worker_count);
        for _ in 0..worker_count {
            let result_queue = result_queue.clone();
            let probes = &probes;
            let probe = &probe;
            let next_probe = &next_probe;
            handles.push(scope.spawn(move || {
                loop {
                    let index = next_probe.fetch_add(1, Ordering::Relaxed);
                    if index >= probes.len() {
                        break;
                    }
                    let result = probe(probes[index].clone());
                    if result_queue.submit((index, result)).is_err() {
                        break;
                    }
                }
            }));
        }
        drop(result_queue);

        let mut slots = (0..probe_count).map(|_| None).collect::<Vec<_>>();
        let mut disconnected = false;
        for _ in 0..probe_count {
            match result_worker.recv() {
                Ok((index, result)) => slots[index] = Some(result),
                Err(_) => {
                    disconnected = true;
                    break;
                }
            }
        }
        let mut worker_panicked = false;
        for handle in handles {
            if handle.join().is_err() {
                worker_panicked = true;
            }
        }
        if worker_panicked {
            return Err(anyhow!("target probe worker failed"));
        }
        if disconnected {
            return Err(anyhow!("target probe result queue disconnected"));
        }
        Ok(slots)
    })?;

    slots
        .into_iter()
        .enumerate()
        .map(|(index, result)| {
            result.ok_or_else(|| anyhow!("target probe result {index} is missing"))?
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn target_probe_pool_is_bounded_and_returns_catalog_order() {
        #[derive(Clone)]
        struct ProbeFixture {
            catalog_index: usize,
            delay_ms: u64,
        }

        let active = std::sync::Arc::new(AtomicUsize::new(0));
        let peak = std::sync::Arc::new(AtomicUsize::new(0));
        let probes = (0..7)
            .map(|catalog_index| ProbeFixture {
                catalog_index,
                delay_ms: if catalog_index == 0 { 35 } else { 5 },
            })
            .collect::<Vec<_>>();
        let active_for_probe = active.clone();
        let peak_for_probe = peak.clone();

        let ordered = run_bounded_target_probes(probes, 3, move |fixture| {
            let now_active = active_for_probe.fetch_add(1, Ordering::SeqCst) + 1;
            peak_for_probe.fetch_max(now_active, Ordering::SeqCst);
            std::thread::sleep(std::time::Duration::from_millis(fixture.delay_ms));
            active_for_probe.fetch_sub(1, Ordering::SeqCst);
            Ok(fixture.catalog_index)
        })
        .unwrap();

        assert_eq!(ordered, (0..7).collect::<Vec<_>>());
        assert_eq!(active.load(Ordering::SeqCst), 0);
        assert!((2..=3).contains(&peak.load(Ordering::SeqCst)));
    }

    #[test]
    fn target_scan_concurrency_is_explicitly_bounded() {
        assert_eq!(
            target_scan_concurrency(&json!({"targetScanConcurrency": 1}), 13),
            1
        );
        assert_eq!(
            target_scan_concurrency(&json!({"targetScanConcurrency": 99}), 13),
            MAX_TARGET_SCAN_CONCURRENCY
        );
        assert_eq!(target_scan_concurrency(&json!({}), 0), 0);
    }
}
