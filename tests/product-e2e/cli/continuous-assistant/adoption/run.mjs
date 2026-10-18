import { invokeAdoptionTarget, TARGET } from "./cargo-target.mjs";

export function runAdoptionProofs() {
  const rust = invokeAdoptionTarget();
  return {
    target: TARGET,
    rust,
  };
}

const invokedAsCli = process.argv[1] && process.argv[1].endsWith("run.mjs");
if (invokedAsCli) {
  const result = runAdoptionProofs();
  process.stdout.write(`${JSON.stringify({
    target: result.rust.target,
    summary: result.rust.summary,
    oracles: result.rust.oracles,
    tests: result.rust.tests,
    status: result.rust.status,
  })}\n`);
  process.exit(result.rust.status === 0 ? 0 : result.rust.status ?? 1);
}
