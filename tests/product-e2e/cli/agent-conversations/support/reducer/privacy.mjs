import { SENSITIVE_KEY_FRAGMENTS } from "./constants.mjs";
import { fail } from "./errors.mjs";
import { isPlainObject, normalizedKey } from "./json.mjs";

export function assertNoSensitiveFields(value) {
  const pending = [value];
  while (pending.length > 0) {
    const current = pending.pop();
    if (Array.isArray(current)) {
      pending.push(...current);
      continue;
    }
    if (!isPlainObject(current)) {
      continue;
    }
    for (const [key, nested] of Object.entries(current)) {
      const candidate = normalizedKey(key);
      if (SENSITIVE_KEY_FRAGMENTS.some((fragment) => candidate.includes(fragment))) {
        fail("sensitive_evidence_field_rejected");
      }
      pending.push(nested);
    }
  }
}
