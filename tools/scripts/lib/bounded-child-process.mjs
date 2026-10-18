function hasExited(child) {
  return child.exitCode !== null || child.signalCode !== null;
}

function waitForExit(child, timeoutMs) {
  if (hasExited(child)) return Promise.resolve(true);
  return new Promise((resolve) => {
    let settled = false;
    let timer;
    const finish = (value) => {
      if (settled) return;
      settled = true;
      if (timer) clearTimeout(timer);
      child.off("exit", onExit);
      resolve(value);
    };
    const onExit = () => finish(true);
    child.once("exit", onExit);
    timer = setTimeout(() => finish(hasExited(child)), timeoutMs);
    if (hasExited(child)) finish(true);
  });
}

export async function stopChildProcess(child, {
  gracefulTimeoutMs,
  forceTimeoutMs = 1_000,
} = {}) {
  if (!child || hasExited(child)) return true;
  if (!Number.isSafeInteger(gracefulTimeoutMs) || gracefulTimeoutMs <= 0 ||
    !Number.isSafeInteger(forceTimeoutMs) || forceTimeoutMs <= 0) {
    throw new Error("bounded child-process timeout is invalid");
  }

  const gracefulExit = waitForExit(child, gracefulTimeoutMs);
  child.kill("SIGTERM");
  if (await gracefulExit) return true;
  if (hasExited(child)) return true;

  const forcedExit = waitForExit(child, forceTimeoutMs);
  child.kill("SIGKILL");
  return await forcedExit || hasExited(child);
}
