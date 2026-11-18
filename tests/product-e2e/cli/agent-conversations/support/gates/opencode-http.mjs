import { spawnSync } from "node:child_process";
import { AcceptanceError, requireFact } from "../parity/errors.mjs";

function extractAssistantText(response) {
  const chunks = [];
  const parts = Array.isArray(response?.parts) ? response.parts : [];
  for (const part of parts) {
    if (part?.type === "text" && typeof part.text === "string") chunks.push(part.text);
  }
  if (chunks.length === 0 && Array.isArray(response)) {
    for (const item of response) {
      if (item?.info?.role === "assistant") chunks.push(extractAssistantText(item));
    }
  }
  return chunks.join("").trim();
}

async function readJson(response, code) {
  let payload;
  try {
    payload = await response.json();
  } catch {
    throw new AcceptanceError(code);
  }
  return payload;
}

export function openCodeWorkspaceUrl(attachUrl, segments, directory) {
  const url = new URL(attachUrl);
  url.pathname = segments.map((segment) => `/${encodeURIComponent(segment)}`).join("");
  url.searchParams.set("directory", directory);
  return url.toString();
}

export function ensureOpenCodeServeAttachUrl(sidecar, binary, timeoutMs) {
  const ensure = spawnSync(
    sidecar,
    ["opencode-serve", "ensure", "--executable", binary],
    {
      encoding: "utf8",
      maxBuffer: 256 * 1024,
      timeout: Math.min(timeoutMs, 90_000),
    },
  );
  if (ensure.error?.code === "ETIMEDOUT") {
    throw new AcceptanceError("opencode_serve_ensure_timeout");
  }
  let payload = null;
  try {
    payload = JSON.parse(String(ensure.stdout || "").trim());
  } catch {
    throw new AcceptanceError("opencode_serve_ensure_invalid_json");
  }
  requireFact(ensure.status === 0 && payload?.ok === true, "opencode_serve_ensure_failed");
  requireFact(payload?.running === true && payload?.healthy === true, "opencode_serve_unhealthy");
  const attachUrl = String(payload?.attachUrl || "").replace(/\/$/u, "");
  requireFact(/^https?:\/\/127\.0\.0\.1:\d+$/u.test(attachUrl), "opencode_serve_attach_url_invalid");
  return attachUrl;
}

export async function nativeOpenCodeHttpTurn(context, requestedSessionId, prompt) {
  const attachUrl = context.serveAttachUrl;
  requireFact(typeof attachUrl === "string" && attachUrl.length > 0, "opencode_serve_attach_missing");
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), context.timeoutMs);
  try {
    let sessionId = requestedSessionId;
    if (sessionId) {
      const probe = await fetch(openCodeWorkspaceUrl(
        attachUrl,
        ["session", sessionId],
        context.cwd,
      ), {
        signal: controller.signal,
      });
      requireFact(probe.ok, "acp_native_session_not_found");
      const probed = await readJson(probe, "opencode_serve_invalid_json");
      requireFact(typeof probed?.id === "string" && probed.id === sessionId, "native_session_identity_mismatch");
    } else {
      const created = await fetch(openCodeWorkspaceUrl(
        attachUrl,
        ["session"],
        context.cwd,
      ), {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({}),
        signal: controller.signal,
      });
      requireFact(created.ok, "opencode_serve_session_create_failed");
      const payload = await readJson(created, "opencode_serve_invalid_json");
      sessionId = String(payload?.id || "");
      requireFact(sessionId.length > 0, "native_session_id_missing");
    }
    context.observedSessions?.add(sessionId);
    const message = await fetch(
      openCodeWorkspaceUrl(
        attachUrl,
        ["session", sessionId, "message"],
        context.cwd,
      ),
      {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ parts: [{ type: "text", text: prompt }] }),
        signal: controller.signal,
      },
    );
    requireFact(message.ok, "opencode_serve_message_failed");
    const response = await readJson(message, "opencode_serve_invalid_json");
    const output = extractAssistantText(response);
    requireFact(output.length > 0, "native_final_message_missing");
    return {
      sessionId,
      output,
      historyNotifications: [],
      settings: {
        cwd: context.cwd,
        model: null,
        reasoningEffort: null,
        mode: null,
        runtimeAgent: null,
        allowAll: null,
      },
      protocolVersion: 1,
      permissionRequests: 0,
      unsupportedRequests: 0,
      boundedOutput: true,
    };
  } catch (error) {
    if (error instanceof AcceptanceError) throw error;
    if (error?.name === "AbortError") throw new AcceptanceError("opencode_serve_timeout");
    throw new AcceptanceError("opencode_serve_request_failed");
  } finally {
    clearTimeout(timer);
  }
}

export async function nativeOpenCodeHttpReadback(context, sessionId) {
  const attachUrl = context.serveAttachUrl;
  requireFact(typeof attachUrl === "string" && attachUrl.length > 0, "opencode_serve_attach_missing");
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), context.timeoutMs);
  try {
    const response = await fetch(openCodeWorkspaceUrl(
      attachUrl,
      ["session", sessionId],
      context.cwd,
    ), {
      signal: controller.signal,
    });
    requireFact(response.ok, "readback_session_identity_mismatch");
    const payload = await readJson(response, "opencode_serve_invalid_json");
    requireFact(payload?.id === undefined || payload.id === sessionId, "readback_session_identity_mismatch");
    const text = extractAssistantText(payload)
      || (Array.isArray(payload?.messages)
        ? payload.messages.map((row) => extractAssistantText(row)).join("\n")
        : "");
    return {
      text,
      settings: {
        cwd: context.cwd,
        model: null,
        reasoningEffort: null,
        mode: null,
        runtimeAgent: null,
        allowAll: null,
      },
      boundedOutput: true,
    };
  } catch (error) {
    if (error instanceof AcceptanceError) throw error;
    if (error?.name === "AbortError") throw new AcceptanceError("opencode_serve_timeout");
    throw new AcceptanceError("opencode_serve_request_failed");
  } finally {
    clearTimeout(timer);
  }
}
