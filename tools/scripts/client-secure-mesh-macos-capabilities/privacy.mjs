export function containsPrivateValue(value) {
  if (typeof value === "string") {
    return (
      /(?:^|["'\s])\/(?:Users|home)\//u.test(value) ||
      /^[A-Za-z]:\\/u.test(value) ||
      /-----BEGIN [A-Z ]*PRIVATE KEY-----/u.test(value) ||
      /\b(?:password|passphrase|secret value|device serial)\s*[:=]/iu.test(value)
    );
  }
  if (Array.isArray(value)) return value.some(containsPrivateValue);
  if (value && typeof value === "object") {
    return Object.values(value).some(containsPrivateValue);
  }
  return false;
}
