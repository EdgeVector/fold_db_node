/**
 * Unwrap FoldDB typed field values like { String: "foo" } to plain primitives.
 * Query results return typed wrappers; this extracts the inner value.
 */
export function unwrapFieldValue(val: unknown): unknown {
  if (val == null) return val;
  if (typeof val !== 'object') return val;
  const obj = val as Record<string, unknown>;
  const keys = Object.keys(obj);
  if (keys.length === 1) return obj[keys[0]];
  return val;
}
