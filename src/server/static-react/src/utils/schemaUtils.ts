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

export interface SchemaLike {
  name?: string;
  descriptive_name?: string;
  org_hash?: string;
  system?: boolean;
  fields?: unknown;
  transform_fields?: unknown;
}

/**
 * Get the human-readable display name for a schema.
 * Prefers descriptive_name, falls back to a short-hash label when the raw
 * `name` is an identity hash (64 hex chars) so user-facing lists never
 * render a 64-char hex string as a schema title.
 */
export function getSchemaDisplayName(schema: SchemaLike | null | undefined): string {
  const descriptive = schema?.descriptive_name
  if (typeof descriptive === 'string' && descriptive.trim()) return descriptive
  const name = schema?.name || ''
  if (isIdentityHash(name)) return `Schema ${name.slice(0, 8)}`
  return name
}

/** Returns true when `s` is a 64-char lowercase hex identity hash. */
export function isIdentityHash(s: unknown): boolean {
  return typeof s === 'string' && s.length === 64 && /^[0-9a-f]+$/.test(s)
}

export interface SchemaOption {
  value: string;
  label: string;
}

/**
 * Build a schema options array for SelectField dropdowns.
 * When orgNames map is provided, org schemas show their org name in the label.
 */
export function buildSchemaOptions(
  schemas: SchemaLike[] | null | undefined,
  orgNames?: Record<string, string>,
): SchemaOption[] {
  return (schemas || [])
    .map(schema => {
      let label = getSchemaDisplayName(schema)
      if (orgNames && schema.org_hash && orgNames[schema.org_hash]) {
        label = `${label}  [${orgNames[schema.org_hash]}]`
      }
      return { value: schema.name ?? '', label }
    })
    .sort((a, b) => a.label.localeCompare(b.label))
}

/**
 * Extract field names from a schema object.
 * Handles both array-of-strings and object-with-field-keys formats.
 */
export function getFieldNames(schemaObj: SchemaLike | null | undefined): string[] {
  if (!schemaObj) return []
  const f = schemaObj.fields || schemaObj.transform_fields
  if (Array.isArray(f)) return f.slice()
  if (f && typeof f === 'object') return Object.keys(f)
  return []
}

/** Toggle a value in a Set (immutable — returns a new Set). */
export function toggleSetItem<T>(set: Iterable<T>, item: T): Set<T> {
  const next = new Set(set)
  if (next.has(item)) next.delete(item)
  else next.add(item)
  return next
}

/** Convert an unknown error value to a string message. */
export function toErrorMessage(error: unknown): string {
  if (error instanceof Error) return error.message
  return String(error)
}

/**
 * Names of infrastructure schemas seeded by the schema service.
 * Used as a fallback when the backend doesn't pass `system: true` on the
 * schema envelope — once `SchemaEnvelope.system` is threaded through the
 * `/schemas` HTTP response, `isSystemSchema` will prefer it and this
 * allow-list becomes a safety net for older responses.
 */
export const SYSTEM_SCHEMA_NAMES: Set<string> = new Set([
  'edge',
  'edge_by_fingerprint',
  'fingerprint',
  'identity',
  'persona',
])

/**
 * Whether a schema is a system/built-in schema (vs. user-proposed).
 * Prefers the backend-provided `system` flag; falls back to a known-name
 * set for older responses that predate the SchemaEnvelope cascade.
 */
export function isSystemSchema(schema: SchemaLike | null | undefined): boolean {
  if (!schema) return false
  if (typeof schema.system === 'boolean') return schema.system
  return SYSTEM_SCHEMA_NAMES.has(schema.name ?? '')
}

/** Truncate a hash string for display. */
export function truncateHash(name: string, threshold = 16, keep = 12): string {
  if (typeof name !== 'string') return name
  if (name.length > threshold) return name.slice(0, keep) + '...'
  return name
}
