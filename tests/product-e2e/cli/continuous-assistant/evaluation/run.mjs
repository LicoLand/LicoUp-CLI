import { invokeEvaluationTarget } from "./cargo-target.mjs";
import { admitLiveEvaluation } from "./live-admission.mjs";

export function runOfflineEvaluation() {
  const admission = admitLiveEvaluation({ mode: "offline" });
  if (!admission.admitted || admission.externalCalls !== 0) {
    throw new Error("offline evaluation driver must admit with zero external calls");
  }
  const rust = invokeEvaluationTarget();
  return {
    mode: "offline",
    admission,
    rust,
  };
}

export function runLiveAdmissionAttempt(request = {}) {
  const admission = admitLiveEvaluation({ ...request, mode: "live" });
  return {
    mode: "live",
    rustInvoked: false,
    admission,
  };
}

const invokedAsCli = process.argv[1] && process.argv[1].endsWith("run.mjs");
if (invokedAsCli) {
  const args = process.argv.slice(2);
  const mode = args.includes("--mode") ? args[args.indexOf("--mode") + 1] : "offline";
  if (mode === "live") {
    const attempt = runLiveAdmissionAttempt();
    process.stdout.write(`${JSON.stringify(attempt)}\n`);
    process.exit(attempt.admission.admitted ? 0 : 2);
  }
  const offline = runOfflineEvaluation();
  process.stdout.write(`${JSON.stringify({
    mode: offline.mode,
    target: offline.rust.target,
    summary: offline.rust.summary,
    oracles: offline.rust.oracles,
    tests: offline.rust.tests,
    status: offline.rust.status,
  })}\n`);
  process.exit(offline.rust.status === 0 ? 0 : offline.rust.status ?? 1);
}
