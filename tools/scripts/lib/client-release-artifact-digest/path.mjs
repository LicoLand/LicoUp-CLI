import { realpathSync } from "node:fs";
import path from "node:path";
import { isWithin, requireValue, stableLstat } from "./helpers.mjs";

export function resolveContainedExistingPath(allowedRoot, candidatePath, {
  expectedKind = "any",
} = {}) {
  const rootPath = path.resolve(allowedRoot);
  const rootInfo = stableLstat(rootPath);
  requireValue(rootInfo?.isDirectory() === true && rootInfo.isSymbolicLink() === false,
    "allowed path root is not a stable directory");
  const rootReal = realpathSync(rootPath);
  const candidate = path.resolve(candidatePath);
  requireValue(isWithin(rootPath, candidate), "path escapes its allowed root");
  const relative = path.relative(rootPath, candidate);
  let current = rootPath;
  for (const component of relative.split(path.sep).filter(Boolean)) {
    current = path.join(current, component);
    const info = stableLstat(current);
    requireValue(info !== undefined, "required contained path is missing");
    requireValue(info.isSymbolicLink() === false, "contained path traverses a symbolic link");
  }
  const candidateReal = realpathSync(candidate);
  requireValue(isWithin(rootReal, candidateReal), "contained path resolves outside its allowed root");
  const finalInfo = stableLstat(candidate);
  requireValue(finalInfo !== undefined && finalInfo.isSymbolicLink() === false,
    "contained path is not stable");
  if (expectedKind === "file") {
    requireValue(finalInfo.isFile(), "contained path is not a regular file");
  } else if (expectedKind === "directory") {
    requireValue(finalInfo.isDirectory(), "contained path is not a directory");
  }
  return candidateReal;
}
