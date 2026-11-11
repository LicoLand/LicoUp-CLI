#!/usr/bin/env node
// The nightly -> stable -> release promotion policy: temporary branches merge
// into nightly, a dated nightly cutoff promotes into stable, and stable
// promotes into release. Release tags are cut from release.
import fs from "node:fs";
import path from "node:path";
import process from "node:process";
import { execFileSync } from "node:child_process";
import { pathToFileURL } from "node:url";

export const LONG_LIVED_BRANCHES = Object.freeze(["nightly", "stable", "release"]);
export const CUTOFF_PATTERN = /^nightly-cutoff\/\d{4}-\d{2}-\d{2}$/u;
const LONG_LIVED = new Set(LONG_LIVED_BRANCHES);
const RETIRED = new Set(["main", "master"]);
const ZERO_OID = "0".repeat(40);
const DIRECT_UPSTREAM = Object.freeze({ release: "stable" });

function identity(payload) {
  return {
    base: payload?.repository?.full_name || "",
    head: payload?.pull_request?.head?.repo?.full_name || ""
  };
}

function sameRepository(payload) {
  const repositories = identity(payload);
  return repositories.base.length > 0
    && repositories.head.length > 0
    && repositories.base === repositories.head;
}

export function evaluateBranchFlow({
  eventName = "",
  refName = "",
  baseRef = "",
  headRef = "",
  payload = {}
} = {}) {
  if (eventName === "push") {
    return LONG_LIVED.has(refName)
      ? { ok: true, code: "protected-push-event" }
      : { ok: false, code: "unexpected-push-ref" };
  }
  if (eventName !== "pull_request" && eventName !== "pull_request_target") {
    return { ok: true, code: "event-not-governed" };
  }

  const base = baseRef || payload.pull_request?.base?.ref || "";
  const head = headRef || payload.pull_request?.head?.ref || "";
  if (RETIRED.has(base)) return { ok: false, code: "retired-base" };
  if (!LONG_LIVED.has(base)) return { ok: true, code: "base-not-governed" };
  if (!sameRepository(payload)) return { ok: false, code: "cross-repository-promotion" };
  if (base === "nightly") {
    return !LONG_LIVED.has(head) && !RETIRED.has(head) && head.length > 0
      ? { ok: true, code: "temporary-to-nightly" }
      : { ok: false, code: "nightly-source-invalid" };
  }
  if (base === "stable") return CUTOFF_PATTERN.test(head)
    ? { ok: true, code: "cutoff-to-stable" }
    : { ok: false, code: "stable-source-invalid" };
  const required = DIRECT_UPSTREAM[base];
  return head === required
    ? { ok: true, code: `${required}-to-${base}` }
    : { ok: false, code: `${base}-source-invalid` };
}

function git(args) {
  return execFileSync("git", args, {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"]
  }).trim();
}

function parents(commit) {
  return git(["show", "-s", "--format=%P", commit]).split(/\s+/u).filter(Boolean);
}

function resolveBranch(branch) {
  for (const ref of [`refs/remotes/origin/${branch}`, `refs/heads/${branch}`]) {
    try {
      return git(["rev-parse", "--verify", `${ref}^{commit}`]);
    } catch {
      // Try the canonical local fallback.
    }
  }
  return "";
}

function isAncestor(ancestor, descendant) {
  try {
    execFileSync("git", ["merge-base", "--is-ancestor", ancestor, descendant], { stdio: "ignore" });
    return true;
  } catch {
    return false;
  }
}

function commitTree(commit) {
  return git(["rev-parse", `${commit}^{tree}`]);
}

function resolveCutoffTips() {
  try {
    return git(["for-each-ref", "--format=%(objectname)", "refs/remotes/origin/nightly-cutoff/"])
      .split(/\s+/u).filter(Boolean);
  } catch { return []; }
}

export function verifyProtectedPushTopology({
  branch,
  before,
  after,
  commitParents = parents,
  branchTip = resolveBranch,
  ancestor = isAncestor,
  cutoffTips = [],
  candidateTree = commitTree
} = {}) {
  if (!LONG_LIVED.has(branch)) return { ok: false, code: "protected-branch-invalid" };
  if (!before || !after || before === ZERO_OID || after === ZERO_OID) {
    return { ok: false, code: "protected-branch-bootstrap-forbidden" };
  }
  const afterParents = commitParents(after);
  if (afterParents.length !== 2 || afterParents[0] !== before) {
    return { ok: false, code: "protected-branch-not-single-merge" };
  }
  const mergedHead = afterParents[1];
  if (branch === "nightly") {
    for (const protectedBranch of LONG_LIVED_BRANCHES) {
      if (protectedBranch === branch) continue;
      const tip = branchTip(protectedBranch);
      if (tip && ancestor(mergedHead, tip)) {
        return { ok: false, code: "nightly-source-protected" };
      }
    }
    return { ok: true, code: "temporary-merge-advanced-nightly" };
  }
  if (branch === "stable") {
    if (!Array.isArray(cutoffTips) || !cutoffTips.includes(mergedHead)) return { ok: false, code: "promotion-source-cutoff-mismatch" };
    return candidateTree(after) === candidateTree(mergedHead)
      ? { ok: true, code: "cutoff-merge-advanced-stable" }
      : { ok: false, code: "promotion-tree-mismatch" };
  }
  const upstream = DIRECT_UPSTREAM[branch];
  const tip = branchTip(upstream);
  if (!tip) return { ok: false, code: "promotion-source-missing" };
  return mergedHead === tip && candidateTree(after) === candidateTree(mergedHead)
    ? { ok: true, code: `${upstream}-merge-advanced-${branch}` }
    : { ok: false, code: "promotion-source-tip-mismatch" };
}

function sameRepositoryPayload(base, head) {
  return {
    repository: { full_name: "example/repository" },
    pull_request: {
      base: { ref: base },
      head: { ref: head, repo: { full_name: "example/repository" } }
    }
  };
}

export function runSelfTest() {
  const policyCases = [
    ["temporary to nightly", true, "nightly", "agent/security-review"],
    ["cutoff to stable", true, "stable", "nightly-cutoff/2026-09-05"],
    ["stable to release", true, "release", "stable"],
    ["stable to nightly", false, "nightly", "stable"],
    ["temporary to stable", false, "stable", "agent/security-review"],
    ["nightly to release", false, "release", "nightly"],
    ["temporary to retired main", false, "main", "agent/security-review"],
    ["retired main to nightly", false, "nightly", "main"]
  ];
  for (const [label, expected, base, head] of policyCases) {
    const result = evaluateBranchFlow({
      eventName: "pull_request",
      baseRef: base,
      headRef: head,
      payload: sameRepositoryPayload(base, head)
    });
    if (result.ok !== expected) throw new Error(`policy fixture failed: ${label}`);
  }
  const missingIdentity = evaluateBranchFlow({
    eventName: "pull_request",
    baseRef: "nightly",
    headRef: "agent/security-review",
    payload: {}
  });
  if (missingIdentity.ok) throw new Error("policy fixture failed: missing repository identity");
  const crossRepository = sameRepositoryPayload("nightly", "agent/security-review");
  crossRepository.pull_request.head.repo.full_name = "fork/repository";
  if (evaluateBranchFlow({
    eventName: "pull_request",
    baseRef: "nightly",
    headRef: "agent/security-review",
    payload: crossRepository
  }).ok) throw new Error("policy fixture failed: cross-repository source");
  const tips = { nightly: "nightly-tip", stable: "stable-tip", release: "release-tip" };
  const topologyCases = [
    ["temporary merge", true, "nightly", ["old-nightly", "feature-tip"]],
    ["cutoff promotion", true, "stable", ["old-stable", "cutoff-tip"]],
    ["stable promotion", true, "release", ["old-release", "stable-tip"]],
    ["direct commit", false, "nightly", ["old-nightly"]],
    ["wrong stable source", false, "stable", ["old-stable", "feature-tip"]],
    ["wrong release source", false, "release", ["old-release", "nightly-tip"]]
  ];
  for (const [label, expected, branch, commitParents] of topologyCases) {
    const result = verifyProtectedPushTopology({
      branch,
      before: commitParents[0],
      after: "after",
      commitParents: () => commitParents,
      branchTip: (name) => tips[name] || "",
      ancestor: (candidate, tip) => candidate === tip,
      cutoffTips: ["cutoff-tip"],
      candidateTree: value => value === "after" || value === "cutoff-tip" || value === "stable-tip" ? "candidate-tree" : value,
    });
    if (result.ok !== expected) throw new Error(`topology fixture failed: ${label}`);
  }
  return { fixtures: policyCases.length + topologyCases.length + 2 };
}

function readPayload(file) {
  if (!file) return {};
  try {
    return JSON.parse(fs.readFileSync(file, "utf8"));
  } catch {
    return {};
  }
}

function verifyCurrentEvent() {
  const payload = readPayload(process.env.GITHUB_EVENT_PATH);
  const result = evaluateBranchFlow({
    eventName: process.env.GITHUB_EVENT_NAME || "",
    refName: process.env.GITHUB_REF_NAME || "",
    baseRef: process.env.GITHUB_BASE_REF || "",
    headRef: process.env.GITHUB_HEAD_REF || "",
    payload
  });
  console[result.ok ? "log" : "error"](`[branch-flow] ${result.code}`);
  if (!result.ok) {
    process.exitCode = 1;
    return;
  }
  if (process.env.GITHUB_EVENT_NAME === "push") {
    const topology = verifyProtectedPushTopology({
      branch: process.env.GITHUB_REF_NAME || "",
      before: payload.before || "",
      after: payload.after || process.env.GITHUB_SHA || "",
      cutoffTips: resolveCutoffTips()
    });
    console[topology.ok ? "log" : "error"](`[branch-flow] ${topology.code}`);
    if (!topology.ok) process.exitCode = 1;
  }
}

function main(argv) {
  if (argv.length === 0) return verifyCurrentEvent();
  if (argv.length === 1 && argv[0] === "--self-test") {
    const result = runSelfTest();
    console.log(`[branch-flow] ${result.fixtures} fixtures passed.`);
    return;
  }
  throw Object.assign(new Error("invalid invocation"), { code: "invalid-invocation" });
}

const invoked = process.argv[1] ? pathToFileURL(path.resolve(process.argv[1])).href : "";
if (invoked === import.meta.url) {
  try {
    main(process.argv.slice(2));
  } catch (error) {
    console.error(`[branch-flow] ${error?.code || "verification-failed"}`);
    process.exitCode = 1;
  }
}
