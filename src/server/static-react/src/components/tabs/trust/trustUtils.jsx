export const truncateKey = (key) => {
  if (!key) return ''
  if (key.length <= 20) return key
  return `${key.slice(0, 10)}...${key.slice(-10)}`
}

export const formatTimestamp = (isoString) => {
  try { return new Date(isoString).toLocaleString() }
  catch { return isoString }
}

export const directionBadge = (direction) => {
  switch (direction) {
    case 'mutual': return <span className="badge badge-success text-xs">mutual</span>
    case 'outgoing': return <span className="badge badge-info text-xs">you trust them</span>
    case 'incoming': return <span className="badge badge-warning text-xs">they trust you</span>
    default: return null
  }
}

export const formatAuditAction = (action) => {
  if (!action) return 'Unknown'
  if (action.TrustGrant) return `Grant trust to ${truncateKey(action.TrustGrant.user_id)} at tier ${action.TrustGrant.tier}`
  if (action.TrustRevoke) return `Revoke trust for ${truncateKey(action.TrustRevoke.user_id)}`
  if (action.Read) return `Read ${action.Read.schema_name}`
  if (action.Write) return `Write ${action.Write.schema_name}`
  if (action.AccessDenied) return `Access denied: ${action.AccessDenied.schema_name}`
  return JSON.stringify(action)
}
