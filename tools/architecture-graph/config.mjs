/**
 * Explicit graph source configuration.
 *
 * Both sources are named as repository-relative paths so the tool runs in any
 * collaborator checkout. The architecture inventory is private-plan data today
 * and the public projection lives under docs/architecture/; nothing here depends
 * on a machine-local absolute path, an environment variable or a home directory.
 */

export const DEFAULT_PROJECT_DOCUMENT = "docs/plans/v7/graph/project.json";
export const DEFAULT_RENDER_DIRECTORY = "docs/plans/v7/graph/generated";
export const DEFAULT_PUBLIC_MAP = "docs/architecture/architecture-map.json";

export const GRAPH_SOURCE_CONTRACT = Object.freeze({
  project: DEFAULT_PROJECT_DOCUMENT,
  architecture: "the architecture inventory referenced by project.json",
  execution: "the private execution inventory referenced by project.json",
  distribution: "the deployment inventory referenced by project.json when present",
  schemas: "a *.schema.json document beside each graph document",
  public_projection: DEFAULT_PUBLIC_MAP,
});
