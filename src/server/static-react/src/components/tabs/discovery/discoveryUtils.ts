import type { Schema } from '../../../types/schema'

interface SchemaLike {
  name?: string
  descriptive_name?: string
  field_interest_categories?: Record<string, string>
  fields?: Record<string, { field_type?: string } | undefined>
}

/** Derive a category from schema field_interest_categories.
 *  Returns the most common interest category across the schema's fields,
 *  falling back to schema name if no interest categories are assigned. */
function inferCategory(schema: Schema | SchemaLike): string {
  const s = schema as SchemaLike
  if (s.field_interest_categories) {
    const counts: Record<string, number> = {}
    for (const cat of Object.values(s.field_interest_categories)) {
      counts[cat] = (counts[cat] || 0) + 1
    }
    const sorted = Object.entries(counts).sort((a, b) => b[1] - a[1])
    if (sorted.length > 0) return sorted[0][0]
  }

  return s.descriptive_name || s.name?.replace(/([A-Z])/g, ' $1').trim().toLowerCase() || 'general'
}

/** Group schemas by their inferred category. */
export function groupByCategory<T extends Schema | SchemaLike>(schemas: T[]): Record<string, T[]> {
  const groups: Record<string, T[]> = {}
  for (const s of schemas) {
    const cat = inferCategory(s)
    if (!groups[cat]) groups[cat] = []
    groups[cat].push(s)
  }
  return groups
}

/** Count fields across schemas in a category. */
export function fieldCount(schemas: Array<Schema | SchemaLike>): number {
  let count = 0
  for (const s of schemas) {
    const fields = (s as SchemaLike).fields
    if (fields) count += Object.keys(fields).length
  }
  return count
}

interface PreviewItem {
  schema: string
  field: string
  type: string
}

/** Build a preview of what will be shared for a set of schemas. */
export function buildPreviewItems(schemas: Array<Schema | SchemaLike>): PreviewItem[] {
  const items: PreviewItem[] = []
  for (const s of schemas) {
    const fields = (s as SchemaLike).fields
    if (!fields) continue
    for (const [fieldName, fieldDef] of Object.entries(fields)) {
      const type = fieldDef?.field_type || 'unknown'
      items.push({ schema: (s as SchemaLike).name ?? '', field: fieldName, type })
    }
  }
  return items
}

export function isLocalModeError(msg: string | null | undefined): boolean {
  return Boolean(msg) && (
    msg!.includes('503') ||
    msg!.includes('DISCOVERY_MASTER_KEY') ||
    msg!.includes('Service Unavailable') ||
    msg!.includes('Discovery not available') ||
    msg!.includes('Register with Exemem')
  )
}
