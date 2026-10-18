export function parseJsonOutput(output) {
  const text = String(output || "");
  const start = text.indexOf("{");
  if (start < 0) throw new Error("signed helper did not return a JSON result");
  return JSON.parse(text.slice(start));
}
