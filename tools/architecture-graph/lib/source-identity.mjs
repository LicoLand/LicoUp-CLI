import fs from "node:fs";
import path from "node:path";
import { spawnSync } from "node:child_process";

import { assertRepoRelativePath, digestOf, GraphError, looksLikeRepoPath, requireFact, sha256Hex } from "./canonical.mjs";

/**
 * Source-file identity and revision resolution (C08: "source-file identity plus
 * explicit revision compose into one resolved typed graph").
 *
 * Two different declarations exist and they must not be conflated:
 *   - a module `path` is a logical boundary projection, not a write lock;
 *   - a task `write_scope` is the only declared source ownership used for
 *     development conflict checks.
 * Both are resolved to a content identity here so a claim, a work item and an
 * evidence receipt can name the same source state.
 */

export const ENTRY_KINDS = Object.freeze(["file", "directory", "absent"]);

export const SOURCE_LIST_METHOD = "git-ls-files-cached-and-untracked-excluding-ignored";

function gitFileList(repoRoot, directory) {
  const result = spawnSync(
    "git",
    ["ls-files", "--cached", "--others", "--exclude-standard", "-z", "--", directory],
    { cwd: repoRoot, encoding: "buffer", maxBuffer: 64 * 1024 * 1024, shell: false },
  );
  if (result.error || result.status !== 0) {
    throw new GraphError(
      `unable to enumerate repository sources for ${directory}; this tool resolves identity from the repository, not from a filesystem walk`,
    );
  }
  return result.stdout
    .toString("utf8")
    .split("\0")
    .filter(Boolean)
    .sort();
}

// Never follow a source symlink, including an intermediate directory. A source
// identity must not hash private files outside the checkout (or special files).
// Inspect components without reading link targets, so errors remain portable.
function sourceStat(repoRoot, relative) {
  assertRepoRelativePath(relative, "source path");
  let current = repoRoot;
  let stat;
  for (const segment of relative.split("/")) {
    current = path.join(current, segment);
    stat = fs.lstatSync(current, { throwIfNoEntry: false });
    if (!stat) return null;
    requireFact(!stat.isSymbolicLink(), "source identity refuses symbolic links");
    requireFact(stat.isFile() || stat.isDirectory(), "source identity requires regular files or directories");
  }
  return stat;
}

function digestDirectory(repoRoot, directory, listSources) {
  const entries = listSources(repoRoot, directory);
  const lines = [];
  let bytes = 0;
  for (const relative of entries) {
    requireFact(relative.startsWith(`${directory}/`), "source listing leaves its declared directory");
    const absolute = path.join(repoRoot, relative);
    const stat = sourceStat(repoRoot, relative);
    if (!stat || !stat.isFile()) continue;
    const content = fs.readFileSync(absolute);
    bytes += content.length;
    lines.push(`${relative}\0${sha256Hex(content)}`);
  }
  lines.sort();
  return {
    entry_kind: "directory",
    sha256: lines.length > 0 ? sha256Hex(lines.join("\n")) : null,
    file_count: lines.length,
    bytes,
  };
}

function resolveOne({ repoRoot, declaredPath, listSources }) {
  const globForm = declaredPath.endsWith("/**");
  const directoryForm = declaredPath.endsWith("/");
  const bare = declaredPath.replace(/\/\*\*$/u, "").replace(/\/+$/u, "");
  const absolute = path.resolve(repoRoot, bare);
  const relativeToRoot = path.relative(repoRoot, absolute);
  requireFact(
    relativeToRoot.length > 0 && !relativeToRoot.startsWith("..") && !path.isAbsolute(relativeToRoot),
    `declared source leaves the repository: ${declaredPath}`,
  );
  const stat = sourceStat(repoRoot, bare);
  const isDirectory = stat?.isDirectory();
  if (!stat) return { entry_kind: "absent", sha256: null, file_count: 0, bytes: 0 };
  if (isDirectory || directoryForm || globForm) {
    requireFact(isDirectory, `declared directory is not a directory: ${declaredPath}`);
    return digestDirectory(repoRoot, bare, listSources);
  }
  const content = fs.readFileSync(absolute);
  return { entry_kind: "file", sha256: sha256Hex(content), file_count: 1, bytes: content.length };
}

/**
 * Collect every source identity declared by the graphs. Module paths that are
 * prose ("cross-cutting contract" style) stay in `logical` instead of being
 * silently treated as repository paths.
 */
export function collectDeclaredIdentities(graph) {
  const declared = new Map();
  const logical = [];
  const add = (declaredPath, owner) => {
    if (!declared.has(declaredPath)) declared.set(declaredPath, []);
    const owners = declared.get(declaredPath);
    const key = `${owner.kind}:${owner.id}`;
    if (!owners.some((entry) => `${entry.kind}:${entry.id}` === key)) owners.push(owner);
  };

  for (const module of graph.modules.values()) {
    if (looksLikeRepoPath(module.path)) add(module.path, { kind: "module", id: module.id });
    else logical.push({ path: module.path, module: module.id, reason: "module path is a logical projection, not a repository path" });
  }
  for (const task of graph.tasks.values()) {
    for (const scope of task.write_scopes) add(scope, { kind: "task", id: task.id });
  }
  return {
    declared,
    logical,
    writeLockOwners: new Map([...graph.tasks.values()].flatMap((task) => task.write_scopes.map((scope) => [scope, task.id]))),
  };
}

export function resolveSourceIdentities({
  graph,
  repoRoot,
  revision,
  listSources = gitFileList,
}) {
  const declaredRevision = graph.architecture.baseline_commit;
  const resolvedRevision = revision ?? declaredRevision;
  requireFact(/^[0-9a-f]{40}$/u.test(resolvedRevision), "source revision must be a full 40-character Git SHA");
  const { declared, logical, writeLockOwners } = collectDeclaredIdentities(graph);

  const identities = [...declared.keys()].sort().map((declaredPath) => {
    const owners = [...declared.get(declaredPath)].sort((a, b) => `${a.kind}${a.id}`.localeCompare(`${b.kind}${b.id}`));
    const content = resolveOne({ repoRoot, declaredPath, listSources });
    const moduleOwners = owners.filter((owner) => owner.kind === "module").map((owner) => owner.id);
    const taskOwners = owners.filter((owner) => owner.kind === "task").map((owner) => owner.id);
    const writeLockOwner = writeLockOwners.get(declaredPath);
    return {
      path: declaredPath,
      normalized_path: declaredPath.replace(/\/\*\*$/u, "").replace(/\/+$/u, ""),
      form: declaredPath.endsWith("/**") ? "prefix" : declaredPath.endsWith("/") ? "directory" : "path",
      entry_kind: content.entry_kind,
      sha256: content.sha256,
      file_count: content.file_count,
      bytes: content.bytes,
      revision: resolvedRevision,
      declared_at_revision: declaredRevision,
      declared_by: owners,
      owner_modules: moduleOwners,
      owner_tasks: taskOwners,
      source_list_method: content.entry_kind === "directory" ? SOURCE_LIST_METHOD : "content-read",
      write_lock: writeLockOwner
        ? { kind: "task-write-scope", lock: true, owner: writeLockOwner, note: "Development conflict checks use task write scopes only." }
        : { kind: "module-projection", lock: false, note: "A module path is a logical boundary projection and is not a write lock." },
    };
  });

  const byPath = Object.fromEntries(identities.map((identity) => [identity.path, identity.sha256]));
  const gaps = [
    ...identities
      .filter((identity) => identity.entry_kind === "absent")
      .map((identity) => ({ kind: "unrealized-source-path", path: identity.path, declared_by: identity.declared_by })),
    ...logical.map((entry) => ({ kind: "logical-module-path", path: entry.path, declared_by: [{ kind: "module", id: entry.module }] })),
  ];

  return {
    kind: "licoup-source-identities.v1",
    revision: {
      declared: declaredRevision,
      resolved: resolvedRevision,
      drift: declaredRevision !== resolvedRevision,
    },
    source_list_method: SOURCE_LIST_METHOD,
    identity_digest: digestOf(byPath),
    identities,
    logical_declarations: logical,
    gaps,
    note: "Identity is resolved from the checked-out worktree. It names the source state a claim was made against; it does not prove the worktree equals any committed revision.",
  };
}
