export type AuditActionPayload = {
  TrustGrant?: { user_id: string; tier: number | string }
  TrustRevoke?: { user_id: string }
  Read?: { schema_name: string }
  Write?: { schema_name: string }
  AccessDenied?: { schema_name: string }
} | null | undefined

export const truncateKey = (key: string | null | undefined): string => {
  if (!key) return ''
  if (key.length <= 20) return key
  return `${key.slice(0, 10)}...${key.slice(-10)}`
}

export const formatTimestamp = (isoString: string): string => {
  try { return new Date(isoString).toLocaleString() }
  catch { return isoString }
}

export const directionBadge = (direction: string | undefined) => {
  switch (direction) {
    case 'mutual': return <span className="badge badge-success text-xs">mutual</span>
    case 'outgoing': return <span className="badge badge-info text-xs">you trust them</span>
    case 'incoming': return <span className="badge badge-warning text-xs">they trust you</span>
    default: return null
  }
}

export const formatAuditAction = (action: AuditActionPayload): string => {
  if (!action) return 'Unknown'
  if (action.TrustGrant) return `Grant trust to ${truncateKey(action.TrustGrant.user_id)} at tier ${action.TrustGrant.tier}`
  if (action.TrustRevoke) return `Revoke trust for ${truncateKey(action.TrustRevoke.user_id)}`
  if (action.Read) return `Read ${action.Read.schema_name}`
  if (action.Write) return `Write ${action.Write.schema_name}`
  if (action.AccessDenied) return `Access denied: ${action.AccessDenied.schema_name}`
  return JSON.stringify(action)
}
