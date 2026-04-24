/** Map raw model IDs to user-friendly display names. */
const MODEL_DISPLAY_NAMES: Record<string, string> = {
  'claude-sonnet-4-20250514': 'Claude Sonnet 4',
  'claude-haiku-4-5-20251001': 'Claude Haiku 4.5',
}

/**
 * Convert a raw model ID (e.g. "claude-sonnet-4-20250514") to a friendly
 * display name (e.g. "Claude Sonnet 4"). Returns the raw ID unchanged if
 * no mapping exists.
 */
export function friendlyModelName(modelId: string | null | undefined): string | null {
  if (!modelId) return null
  return MODEL_DISPLAY_NAMES[modelId] || modelId
}
