import { readFileSync, readdirSync, realpathSync } from "node:fs";
import { spawnSync } from "node:child_process";
import path from "node:path";

const CARGO_PROCESS_NAMES = new Set(["cargo", "cargo.exe", "rustc", "rustc.exe"]);

function pathIsInside(directory, candidate) {
  const relative = path.relative(directory, candidate);
  return relative === "" ||
    (relative !== ".." && !relative.startsWith(`..${path.sep}`) && !path.isAbsolute(relative));
}

function processEntriesLinux() {
  const entries = [];
  for (const entry of readdirSync("/proc", { withFileTypes: true })) {
    if (!entry.isDirectory() || !/^\d+$/u.test(entry.name)) continue;
    try {
      const processRoot = `/proc/${entry.name}`;
      const executable = realpathSync(`${processRoot}/exe`);
      entries.push({
        pid: Number(entry.name),
        path: executable,
        paths: [executable],
        cwd: realpathSync(`${processRoot}/cwd`),
        command: readFileSync(`${processRoot}/comm`, "utf8").trim(),
        arguments: readFileSync(`${processRoot}/cmdline`, "utf8")
          .split("\0")
          .filter(Boolean),
      });
    } catch (error) {
      if (error?.code === "ENOENT") continue;
      throw error;
    }
  }
  return entries;
}

export function parseMacOSProcessSnapshot(lsof, processList) {
  if (lsof.error || lsof.status !== 0 || String(lsof.stderr || "").trim() ||
      processList.error || processList.status !== 0 || String(processList.stderr || "").trim()) {
    return null;
  }
  const argumentsByPid = new Map();
  for (const line of processList.stdout.split(/\r?\n/u)) {
    const match = /^\s*(\d+)\s+(.*)$/u.exec(line);
    if (match) argumentsByPid.set(Number(match[1]), match[2]);
  }
  const entries = new Map();
  let pid = null;
  let command = "";
  let descriptor = "";
  for (const line of lsof.stdout.split(/\r?\n/u)) {
    if (line.startsWith("p")) {
      const parsed = Number(line.slice(1));
      pid = Number.isSafeInteger(parsed) ? parsed : null;
      command = "";
      descriptor = "";
    } else if (line.startsWith("c")) {
      command = line.slice(1);
    } else if (line.startsWith("f")) {
      descriptor = line.slice(1);
    } else if (line.startsWith("n") && pid) {
      const entry = entries.get(pid) || { pid, command, paths: [] };
      if (descriptor === "cwd") entry.cwd = line.slice(1);
      else if (descriptor === "txt" && !entry.paths.includes(line.slice(1))) {
        entry.paths.push(line.slice(1));
      }
      entries.set(pid, entry);
    }
  }
  return [...entries.values()]
    .filter((entry) => entry.paths.length > 0)
    .map((entry) => ({
      ...entry,
      path: entry.paths[0],
      arguments: argumentsByPid.get(entry.pid) || "",
    }));
}

function processEntriesMacOS() {
  const lsof = spawnSync("lsof", ["-nP", "-Fpcfn", "-d", "txt,cwd"], {
    encoding: "utf8",
    windowsHide: true,
    maxBuffer: 16 * 1024 * 1024,
  });
  const processList = spawnSync("ps", ["-axo", "pid=,command="], {
    encoding: "utf8",
    windowsHide: true,
    maxBuffer: 16 * 1024 * 1024,
  });
  return parseMacOSProcessSnapshot(lsof, processList);
}

function processEntriesWindows() {
  const command = [
    "$ErrorActionPreference='Stop';",
    "Get-CimInstance Win32_Process |",
    "Select-Object @{Name='pid';Expression={$_.ProcessId}},@{Name='command';Expression={$_.Name}},",
    "@{Name='path';Expression={$_.ExecutablePath}},@{Name='arguments';Expression={$_.CommandLine}} |",
    "ConvertTo-Json -Compress",
  ].join(" ");
  const result = spawnSync("powershell.exe", [
    "-NoLogo",
    "-NoProfile",
    "-NonInteractive",
    "-Command",
    command,
  ], {
    encoding: "utf8",
    windowsHide: true,
    maxBuffer: 16 * 1024 * 1024,
  });
  if (result.error || result.status !== 0) return null;
  try {
    const parsed = JSON.parse(result.stdout || "[]");
    return (Array.isArray(parsed) ? parsed : [parsed])
      .filter((entry) => Number.isSafeInteger(Number(entry.pid)) &&
        (entry.path || CARGO_PROCESS_NAMES.has(String(entry.command || "").toLowerCase())))
      .map((entry) => ({
        pid: Number(entry.pid),
        path: entry.path ? String(entry.path) : "",
        paths: entry.path ? [String(entry.path)] : [],
        command: String(entry.command || ""),
        arguments: String(entry.arguments || ""),
      }));
  } catch {
    return null;
  }
}

export function inspectRunningExecutables() {
  try {
    if (process.platform === "linux") return { available: true, entries: processEntriesLinux() };
    if (process.platform === "darwin") {
      const entries = processEntriesMacOS();
      return entries ? { available: true, entries } : { available: false, entries: [] };
    }
    if (process.platform === "win32") {
      const entries = processEntriesWindows();
      return entries ? { available: true, entries } : { available: false, entries: [] };
    }
    return { available: false, entries: [] };
  } catch {
    return { available: false, entries: [] };
  }
}

export function targetProcessUsage(targetPath, snapshot, repoRoot) {
  const target = path.resolve(targetPath);
  const workspace = path.resolve(repoRoot || path.dirname(target));
  if (!snapshot.available) {
    return { available: false, binaryPids: [], buildPids: [] };
  }
  const binaryPids = new Set();
  const buildPids = new Set();
  const windows = process.platform === "win32";
  for (const entry of snapshot.entries) {
    const directory = windows ? target.toLowerCase() : target;
    const executables = Array.isArray(entry.paths)
      ? entry.paths
      : entry.path
        ? [entry.path]
        : [];
    if (executables.some((executablePath) => {
      const executable = path.resolve(executablePath);
      const candidate = windows ? executable.toLowerCase() : executable;
      return candidate && pathIsInside(directory, candidate);
    })) {
      binaryPids.add(entry.pid);
      continue;
    }
    const command = path.basename(entry.command || entry.path || executables[0] || "").toLowerCase();
    if (!CARGO_PROCESS_NAMES.has(command)) continue;
    if (entry.cwd) {
      const workingDirectory = windows ? entry.cwd.toLowerCase() : entry.cwd;
      const repository = windows ? workspace.toLowerCase() : workspace;
      if (pathIsInside(repository, workingDirectory)) buildPids.add(entry.pid);
    }
    const commandLine = Array.isArray(entry.arguments)
      ? entry.arguments.join(" ")
      : String(entry.arguments || "");
    const normalizedCommandLine = windows ? commandLine.toLowerCase() : commandLine;
    if (normalizedCommandLine.includes(directory)) {
      buildPids.add(entry.pid);
      continue;
    }
    if (windows) buildPids.add(entry.pid);
  }
  return {
    available: true,
    binaryPids: [...binaryPids],
    buildPids: [...buildPids],
  };
}
