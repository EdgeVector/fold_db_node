/**
 * Unwrap FoldDB typed field values like { String: "foo" } to plain primitives.
 * Query results return typed wrappers; this extracts the inner value.
 */
export function unwrapFieldValue(val) {
  if (val == null) return val;
  if (typeof val !== 'object') return val;
  const keys = Object.keys(val);
  if (keys.length === 1) return val[keys[0]];
  return val;
}
