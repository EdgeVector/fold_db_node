/**
 * Shared schema utilities used across multiple components.
 *
 * Consolidates duplicated logic for:
 * - Schema display names
 * - Field name extraction
 * - Set toggling
 * - Error message conversion
 * - Query-by-key record fetching
 */

/**
 * Get the human-readable display name for a schema.
 * Prefers descriptive_name, falls back to a short-hash label when the raw
 * `name` is an identity hash (64 hex chars) so user-facing lists never
 * render a 64-char hex string as a schema title.
 */
export function getSchemaDisplayName(schema) {
  const descriptive = schema?.descriptive_name
  if (typeof descriptive === 'string' && descriptive.trim()) return descriptive
  const name = schema?.name || ''
  if (isIdentityHash(name)) return `Schema ${name.slice(0, 8)}`
  return name
}

/** @returns true when `s` is a 64-char lowercase hex identity hash. */
export function isIdentityHash(s) {
  return typeof s === 'string' && s.length === 64 && /^[0-9a-f]+$/.test(s)
}

/**
 * Build a schema options array for SelectField dropdowns.
 * When orgNames map is provided, org schemas show their org name in the label.
 * @param {Object[]} schemas - Array of schema objects
 * @param {Object} [orgNames] - Map of org_hash → org_name
 * @returns {{ value: string, label: string }[]}
 */
export function buildSchemaOptions(schemas, orgNames) {
  return (schemas || [])
    .map(schema => {
      let label = getSchemaDisplayName(schema)
      if (orgNames && schema.org_hash && orgNames[schema.org_hash]) {
        label = `${label}  [${orgNames[schema.org_hash]}]`
      }
      return { value: schema.name, label }
    })
    .sort((a, b) => a.label.localeCompare(b.label))
}

/**
 * Extract field names from a schema object.
 * Handles both array-of-strings and object-with-field-keys formats.
 */
export function getFieldNames(schemaObj) {
  if (!schemaObj) return []
  const f = schemaObj.fields || schemaObj.transform_fields
  if (Array.isArray(f)) return f.slice()
  if (f && typeof f === 'object') return Object.keys(f)
  return []
}

/**
 * Toggle a value in a Set (immutable — returns a new Set).
 */
export function toggleSetItem(set, item) {
  const next = new Set(set)
  if (next.has(item)) next.delete(item)
  else next.add(item)
  return next
}

/**
 * Convert an unknown error value to a string message.
 */
export function toErrorMessage(error) {
  if (error instanceof Error) return error.message
  return String(error)
}

/**
 * Truncate a hash string for display.
 * @param {string} name - The hash/name string
 * @param {number} [threshold=16] - Show full string if shorter than this
 * @param {number} [keep=12] - Number of leading characters to keep
 * @returns {string}
 */
export function truncateHash(name, threshold = 16, keep = 12) {
  if (typeof name !== 'string') return name
  if (name.length > threshold) return name.slice(0, keep) + '...'
  return name
}
