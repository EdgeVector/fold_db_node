import { formatTimestamp, formatAuditAction } from './trustUtils'

export default function AuditLogPanel({ auditEvents }) {
  return (
    <div>
      {auditEvents.length === 0 && (
        <div className="text-center py-12 border border-border rounded-lg">
          <p className="text-secondary text-lg mb-2">No audit events</p>
          <p className="text-tertiary text-sm">
            Trust operations will appear here as they occur.
          </p>
        </div>
      )}

      {auditEvents.length > 0 && (
        <div className="space-y-2">
          {auditEvents.map((event, idx) => (
            <div
              key={event.id || idx}
              className="border border-border rounded-lg p-3 bg-surface"
            >
              <div className="flex items-start justify-between gap-3">
                <div className="flex-1 min-w-0">
                  <p className="text-sm text-primary">
                    {formatAuditAction(event.action)}
                  </p>
                  <span className="text-xs text-tertiary">
                    {formatTimestamp(event.timestamp)}
                  </span>
                </div>
                <span className={`badge text-xs ${event.decision_granted ? 'badge-success' : 'badge-warning'}`}>
                  {event.decision_granted ? 'granted' : 'denied'}
                </span>
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  )
}
