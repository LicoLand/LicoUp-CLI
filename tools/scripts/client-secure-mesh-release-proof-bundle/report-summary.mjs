import { buildReleaseProofSummaryCore } from "./report-summary-core.mjs";
import { buildReleaseProofSummaryPhysical } from "./report-summary-physical.mjs";

export function buildReleaseProofSummary(input) {
  return {
    ...buildReleaseProofSummaryCore(input),
    ...buildReleaseProofSummaryPhysical(input),
  };
}
