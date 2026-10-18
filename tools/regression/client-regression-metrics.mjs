export function unavailableProcessTreeMetrics() {
  const unavailable = (reason) => Object.freeze({ status: "unavailable", reason });
  return Object.freeze({
    directCpuMs: unavailable("native_process_supervisor_unavailable"),
    descendantCpuMs: unavailable("native_process_supervisor_unavailable"),
    peakResidentBytes: unavailable("native_process_supervisor_unavailable"),
  });
}

export function defaultProcessTreeMetricsAdapter() {
  // Node exposes resource usage for the current process, not accurate direct
  // and descendant usage for an arbitrary spawned process tree. Do not infer
  // these values from polling, which misses short-lived descendants.
  return Object.freeze({
    async measure() {
      return unavailableProcessTreeMetrics();
    },
  });
}
