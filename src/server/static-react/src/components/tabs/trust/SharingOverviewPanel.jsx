import { getSharingPosture } from '../../../api/clients/trustClient'

export default function SharingOverviewPanel({ posture, setPosture, setError, onResult }) {
  if (!posture) {
    return (
      <div className="text-center py-8">
        <div className="w-6 h-6 border-2 border-border border-t-primary rounded-full animate-spin mx-auto" />
      </div>
    )
  }

  return (
    <div>
      <div className="space-y-4">
        <div className="grid grid-cols-2 gap-4">
          <div className="border border-border rounded-lg p-4 bg-surface text-center">
            <div className="text-2xl font-bold text-primary">{posture.total_policy_fields}</div>
            <div className="text-xs text-secondary">Protected fields</div>
          </div>
          <div className="border border-border rounded-lg p-4 bg-surface text-center">
            <div className="text-2xl font-bold text-gruvbox-yellow">{posture.total_unprotected_fields}</div>
            <div className="text-xs text-secondary">Unprotected fields</div>
          </div>
        </div>
        {posture.total_unprotected_fields > 0 && (
          <div className="bg-gruvbox-yellow/15 border border-gruvbox-yellow/30 rounded-lg p-4">
            <p className="text-sm text-gruvbox-yellow font-medium mb-1">
              {posture.total_unprotected_fields} fields have no access policy
            </p>
            <p className="text-xs text-secondary mb-3">
              Anyone you trust in any domain can see these fields. Apply default policies based on data classification to protect them.
            </p>
            <button
              className="btn btn-sm"
              onClick={async () => {
                try {
                  const resp = await fetch('/api/sharing/apply-defaults', { method: 'POST' })
                  if (!resp.ok) throw new Error(`HTTP ${resp.status}`)
                  const data = await resp.json()
                  if (data.ok !== false) {
                    const d = data.data || data
                    if (onResult) onResult({ success: true, data: { message: `Applied policies to ${d.fields_updated} fields across ${d.schemas_updated} schemas` } })
                    getSharingPosture().then(r => { if (r.success && r.data) setPosture(r.data) }).catch(() => {})
                  } else {
                    setError(data.error || 'Failed to apply default policies')
                  }
                } catch (err) {
                  setError(err?.message || 'Failed to apply default policies')
                }
              }}
            >
              Apply Default Policies
            </button>
          </div>
        )}
        {posture.domains.length > 0 && (
          <div className="border border-border rounded-lg p-4 bg-surface">
            <h3 className="text-sm font-medium text-primary mb-3">Trust Domains</h3>
            <div className="space-y-2">
              {posture.domains.map((domain) => (
                <div key={domain} className="flex items-center justify-between text-sm">
                  <span className="text-primary">{domain}</span>
                  <div className="flex gap-3 text-xs text-secondary">
                    <span>{posture.schemas_per_domain[domain] || 0} schemas</span>
                    <span>{posture.contacts_per_domain[domain] || 0} contacts</span>
                  </div>
                </div>
              ))}
            </div>
          </div>
        )}
        {posture.domains.length === 0 && (
          <div className="text-center py-8 border border-border rounded-lg">
            <p className="text-secondary">No trust domains configured yet.</p>
            <p className="text-xs text-tertiary mt-1">Assign roles to contacts to create trust domains.</p>
          </div>
        )}
      </div>
    </div>
  )
}
