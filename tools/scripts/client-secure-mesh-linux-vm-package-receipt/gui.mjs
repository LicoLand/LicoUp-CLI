import { spawn, spawnSync } from "node:child_process";
import process from "node:process";
import { stopChildProcess } from "../lib/bounded-child-process.mjs";
import { assert, waitFor } from "./util.mjs";

export async function runGuiSession(ctx, flutterClient, installedBundle, portableRoot) {
  for (const tool of ["Xvfb", "xdotool"]) {
    const check = spawnSync("bash", ["-lc", `command -v ${tool}`], { encoding: "utf8" });
    assert(check.status === 0, "Linux GUI session tool is unavailable");
  }
  const display = `:${200 + (process.pid % 500)}`;
  const env = {
    ...process.env,
    DISPLAY: display,
    GDK_BACKEND: "x11",
    GDK_GL: "software",
    LIBGL_ALWAYS_SOFTWARE: "1",
    NO_AT_BRIDGE: "1",
    LICOUP_HOME: portableRoot
  };
  const xvfb = spawn("Xvfb", [
    display,
    "-screen",
    "0",
    "1280x800x24",
    "-ac",
    "+extension",
    "GLX",
    "+render",
    "-noreset",
    "-nolisten",
    "tcp"
  ], { stdio: "ignore" });
  let app;
  let stderrBytes = 0;
  let stderrOverflow = false;
  try {
    ctx.verificationPhase = "gui_display";
    await waitFor(() => {
      assert(xvfb.exitCode === null, "Linux virtual display exited before readiness");
      const probe = spawnSync("xdotool", ["getdisplaygeometry"], {
        env,
        stdio: "ignore"
      });
      return probe.status === 0;
    }, 5_000, "virtual display readiness");
    ctx.verificationPhase = "gui_process";
    app = spawn(flutterClient, ["--enable-software-rendering"], {
      cwd: installedBundle,
      env,
      stdio: ["ignore", "ignore", "pipe"]
    });
    app.stderr.on("data", (chunk) => {
      stderrBytes = Math.min(64 * 1024 + 1, stderrBytes + Buffer.byteLength(chunk));
      if (stderrBytes > 64 * 1024) {
        stderrOverflow = true;
        app.kill("SIGTERM");
      }
    });
    ctx.verificationPhase = "gui_window";
    const windowId = await waitFor(() => {
      assert(app.exitCode === null, "Installed Linux desktop client exited before readiness");
      const search = spawnSync("xdotool", [
        "search",
        "--onlyvisible",
        "--pid",
        String(app.pid),
        "--name",
        ".*"
      ], {
        env,
        encoding: "utf8"
      });
      return search.status === 0
        ? String(search.stdout || "").trim().split(/\s+/u).find(Boolean) || ""
        : "";
    }, 30_000, "installed Linux desktop window");
    ctx.verificationPhase = "gui_interaction";
    const interaction = spawnSync("xdotool", ["key", "--window", windowId, "Tab"], {
      env,
      stdio: "ignore"
    });
    assert(interaction.status === 0 && app.exitCode === null,
      "Installed Linux desktop interaction smoke failed");
    ctx.verificationPhase = "gui_shutdown";
    const boundedShutdown = await stopChildProcess(app, { gracefulTimeoutMs: 5_000 });
    app = null;
    assert(boundedShutdown, "Installed Linux desktop client did not stop within the bound");
    ctx.verificationPhase = "gui_stderr";
    assert(stderrOverflow === false && stderrBytes <= 64 * 1024,
      "Installed Linux desktop stderr exceeded the bounded buffer");
    return {
      clientStarted: true,
      visibleWindow: true,
      interactionSmoke: true,
      boundedShutdown
    };
  } finally {
    if (app) await stopChildProcess(app, { gracefulTimeoutMs: 2_000 });
    await stopChildProcess(xvfb, { gracefulTimeoutMs: 2_000 });
  }
}
