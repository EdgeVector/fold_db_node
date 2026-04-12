/** Derive a category from schema field_interest_categories.
 *  Returns the most common interest category across the schema's fields,
 *  falling back to schema name if no interest categories are assigned. */
export function inferCategory(schema) {
  if (schema.field_interest_categories) {
    const counts = {}
    for (const cat of Object.values(schema.field_interest_categories)) {
      counts[cat] = (counts[cat] || 0) + 1
    }
    const sorted = Object.entries(counts).sort((a, b) => b[1] - a[1])
    if (sorted.length > 0) return sorted[0][0]
  }

  return schema.descriptive_name || schema.name?.replace(/([A-Z])/g, ' $1').trim().toLowerCase() || 'general'
}

/** Group schemas by their inferred category. */
export function groupByCategory(schemas) {
  const groups = {}
  for (const s of schemas) {
    const cat = inferCategory(s)
    if (!groups[cat]) groups[cat] = []
    groups[cat].push(s)
  }
  return groups
}

/** Count fields across schemas in a category. */
export function fieldCount(schemas) {
  let count = 0
  for (const s of schemas) {
    if (s.fields) count += Object.keys(s.fields).length
  }
  return count
}

/** Build a preview of what will be shared for a set of schemas. */
export function buildPreviewItems(schemas) {
  const items = []
  for (const s of schemas) {
    if (!s.fields) continue
    for (const [fieldName, fieldDef] of Object.entries(s.fields)) {
      const type = fieldDef?.field_type || 'unknown'
      items.push({ schema: s.name, field: fieldName, type })
    }
  }
  return items
}

/** Map numeric trust tier to human-readable label. */
export const TRUST_TIER_LABELS = {
  0: 'Public',
  1: 'Outer',
  2: 'Trusted',
  3: 'Inner',
  4: 'Owner',
}

export function trustTierLabel(tier) {
  return TRUST_TIER_LABELS[tier] ?? `Tier ${tier}`
}

export function isLocalModeError(msg) {
  return msg && (msg.includes('503') || msg.includes('DISCOVERY_MASTER_KEY') || msg.includes('Service Unavailable') || msg.includes('Discovery not available') || msg.includes('Register with Exemem'))
}
