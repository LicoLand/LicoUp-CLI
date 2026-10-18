export function normalizeSourceCheckFiles(check, normalizeSourceRef, label) {
  const sourceRefs = Array.isArray(check.files) ? check.files : [check.file];
  const files = [...new Set(sourceRefs.map((sourceRef, sourceIndex) =>
    normalizeSourceRef(sourceRef, `${label} file ${sourceIndex + 1}`)))];
  if (files.length === 0) {
    throw new Error(`${label} must define files`);
  }
  return files;
}

export async function readSourceCheckBundle(check, readText) {
  const files = check.files || [check.file];
  const source = (await Promise.all(files.map((file) => readText(file)))).join("\n");
  return { files, source };
}
