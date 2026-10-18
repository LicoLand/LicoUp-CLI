import { cpSync, existsSync, mkdirSync, readdirSync, statSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import process from "node:process";

function defaultCacheRoot(env = process.env) {
  if (process.platform === "darwin") {
    return path.join(os.homedir(), "Library", "Caches", "LicoMesh", "client-toolchain");
  }
  if (process.platform === "win32") {
    return path.join(
      env.LOCALAPPDATA || path.join(os.homedir(), "AppData", "Local"),
      "LicoMesh",
      "ClientToolchain"
    );
  }
  return path.join(env.XDG_CACHE_HOME || path.join(os.homedir(), ".cache"), "licomesh", "client-toolchain");
}

export function clientCacheRoot(env = process.env) {
  return path.resolve(env.LICO_CLIENT_CACHE_ROOT || defaultCacheRoot(env));
}

export function clientPubCacheRoot(env = process.env) {
  return path.resolve(env.LICO_CLIENT_PUB_CACHE || env.PUB_CACHE || path.join(clientCacheRoot(env), "pub-cache"));
}

export function clientGradleUserHome(env = process.env) {
  return path.resolve(
    env.LICO_CLIENT_GRADLE_USER_HOME || env.GRADLE_USER_HOME || path.join(clientCacheRoot(env), "gradle-user-home")
  );
}

export function defaultSystemGradleUserHome() {
  return path.resolve(path.join(os.homedir(), ".gradle"));
}

function sourceGradleUserHomes(targetGradleHome, env = process.env) {
  const roots = [
    env.GRADLE_USER_HOME ? path.resolve(env.GRADLE_USER_HOME) : null,
    defaultSystemGradleUserHome()
  ].filter(Boolean);
  return [...new Set(roots)].filter((root) => root !== path.resolve(targetGradleHome));
}

function copyMissingTree(sourcePath, targetPath) {
  const sourceStats = statSync(sourcePath);
  if (sourceStats.isDirectory()) {
    mkdirSync(targetPath, { recursive: true });
    for (const entry of readdirSync(sourcePath, { withFileTypes: true })) {
      copyMissingTree(path.join(sourcePath, entry.name), path.join(targetPath, entry.name));
    }
    return;
  }
  if (existsSync(targetPath)) {
    const targetStats = statSync(targetPath);
    if (targetStats.size === sourceStats.size) {
      return;
    }
  } else {
    mkdirSync(path.dirname(targetPath), { recursive: true });
  }
  cpSync(sourcePath, targetPath, {
    dereference: false,
    force: true
  });
}

function seedGradleSubtree(sourceRoots, targetGradleHome, relativePath, label, log) {
  const markerPath = path.join(targetGradleHome, ".lico-seeded", `${relativePath.replaceAll(/[\\/]/g, "__")}.merge-v2.marker`);
  if (existsSync(markerPath)) {
    return false;
  }
  for (const sourceRoot of sourceRoots) {
    const sourcePath = path.join(sourceRoot, relativePath);
    if (!existsSync(sourcePath)) {
      continue;
    }
    const targetPath = path.join(targetGradleHome, relativePath);
    log?.(`[client-toolchain] Seeding Gradle ${label} into isolated cache.`);
    copyMissingTree(sourcePath, targetPath);
    mkdirSync(path.dirname(markerPath), { recursive: true });
    writeFileSync(markerPath, "seeded\n", "utf8");
    return true;
  }
  return false;
}

export function seedClientGradleHome(env = process.env, options = {}) {
  const targetGradleHome = clientGradleUserHome(env);
  mkdirSync(targetGradleHome, { recursive: true });
  const sourceRoots = sourceGradleUserHomes(targetGradleHome, env);
  return [
    seedGradleSubtree(sourceRoots, targetGradleHome, "wrapper/dists", "wrapper distributions", options.log),
    seedGradleSubtree(sourceRoots, targetGradleHome, "caches/modules-2", "dependency modules", options.log)
  ].filter(Boolean).length;
}

export function clientAndroidProjectCacheRoot(env = process.env) {
  return path.resolve(
    env.LICO_CLIENT_ANDROID_PROJECT_CACHE || path.join(clientCacheRoot(env), "android-project-cache")
  );
}

export function defaultSystemPubCacheRoot(env = process.env) {
  if (process.platform === "win32") {
    return path.resolve(env.LOCALAPPDATA || path.join(os.homedir(), "AppData", "Local"), "Pub", "Cache");
  }
  return path.resolve(os.homedir(), ".pub-cache");
}

const LOOPBACK_PROXY_EXEMPTIONS = Object.freeze(["localhost", "127.0.0.1", "::1"]);

function mergeLoopbackProxyExemptions(current) {
  const parts = String(current ?? "")
    .split(/[\s,]+/u)
    .map((part) => part.trim())
    .filter(Boolean);
  const seen = new Set(parts.map((part) => part.toLowerCase()));
  for (const extra of LOOPBACK_PROXY_EXEMPTIONS) {
    if (!seen.has(extra.toLowerCase())) {
      parts.push(extra);
      seen.add(extra.toLowerCase());
    }
  }
  return parts.join(",");
}

export function withTestProcessLoopbackProxyExemption(env = process.env) {
  const nextEnv = { ...env };
  const upper = nextEnv.NO_PROXY;
  const lower = nextEnv.no_proxy;
  if (upper != null && lower == null) {
    const merged = mergeLoopbackProxyExemptions(upper);
    nextEnv.NO_PROXY = merged;
    nextEnv.no_proxy = merged;
  } else if (lower != null && upper == null) {
    const merged = mergeLoopbackProxyExemptions(lower);
    nextEnv.NO_PROXY = merged;
    nextEnv.no_proxy = merged;
  } else {
    nextEnv.NO_PROXY = mergeLoopbackProxyExemptions(upper);
    nextEnv.no_proxy = mergeLoopbackProxyExemptions(lower);
  }
  return nextEnv;
}

export function withClientToolchainEnv(env = process.env, options = {}) {
  const nextEnv = {
    ...withTestProcessLoopbackProxyExemption(env),
    PUB_CACHE: path.resolve(options.pubCache || clientPubCacheRoot(env)),
    GRADLE_USER_HOME: path.resolve(options.gradleUserHome || clientGradleUserHome(env))
  };
  if (options.pubHostedUrl) {
    nextEnv.PUB_HOSTED_URL = options.pubHostedUrl;
  } else if (env.LICO_CLIENT_PUB_HOSTED_URL) {
    nextEnv.PUB_HOSTED_URL = env.LICO_CLIENT_PUB_HOSTED_URL;
  }
  if (env.LICO_FLUTTER_STORAGE_BASE_URL) {
    nextEnv.FLUTTER_STORAGE_BASE_URL = env.LICO_FLUTTER_STORAGE_BASE_URL;
  }
  return nextEnv;
}
