import { truncateKey, formatTimestamp, directionBadge } from './trustUtils'

export default function ContactsPanel({
  identityLoading,
  identityCard,
  setActiveSection,
  contactsLoading,
  contacts,
  selectedContact,
  setSelectedContact,
  setAuditResult,
  handleAudit,
  handleRevoke,
  revoking,
  availableRoles,
  assigningRole,
  handleAssignRole,
  handleRemoveRole,
  auditLoading,
  auditResult,
}) {
  return (
    <>
      {/* Identity card warning */}
      {!identityLoading && !identityCard && (
        <div className="bg-gruvbox-yellow/15 border border-gruvbox-yellow/30 rounded-lg p-4 mb-4">
          <p className="text-sm text-gruvbox-yellow">
            Set your display name in the <button className="underline font-medium" onClick={() => setActiveSection('identity')}>My Identity</button> tab
            before sharing trust invites.
          </p>
        </div>
      )}

      {contactsLoading && (
        <div className="text-center py-12">
          <div className="w-6 h-6 border-2 border-border border-t-primary rounded-full animate-spin mx-auto mb-3" />
          <p className="text-secondary text-sm">Loading contacts...</p>
        </div>
      )}

      {!contactsLoading && contacts.length === 0 && (
        <div className="text-center py-12 border border-border rounded-lg">
          <p className="text-secondary text-lg mb-2">No trusted contacts</p>
          <p className="text-tertiary text-sm mb-4">
            Share a trust invite or accept one to add your first contact.
          </p>
          <button className="btn" onClick={() => setActiveSection('invite')}>
            Add Contact
          </button>
        </div>
      )}

      {!contactsLoading && contacts.length > 0 && (
        <div className="space-y-2">
          {contacts.map((contact) => (
            <div
              key={contact.public_key}
              className="border border-border rounded-lg p-4 bg-surface"
            >
              <div className="flex items-start justify-between gap-4">
                <div className="flex-1 min-w-0">
                  <div className="flex items-center gap-2 mb-1">
                    <span className="text-sm font-medium text-primary">
                      {contact.display_name}
                    </span>
                    {directionBadge(contact.direction)}
                  </div>
                  {/* Role badges */}
                  {contact.roles && Object.keys(contact.roles).length > 0 && (
                    <div className="flex flex-wrap gap-1 mb-1">
                      {Object.entries(contact.roles).map(([domain, role]) => (
                        <span key={domain} className="badge badge-info text-xs">
                          {role} ({domain})
                        </span>
                      ))}
                    </div>
                  )}
                  {contact.contact_hint && (
                    <p className="text-xs text-secondary mb-1">{contact.contact_hint}</p>
                  )}
                  <div className="flex items-center gap-3 text-xs text-tertiary">
                    <code title={contact.public_key}>{truncateKey(contact.public_key)}</code>
                    <span>Connected {formatTimestamp(contact.connected_at)}</span>
                  </div>
                </div>
                <div className="flex gap-1 flex-shrink-0">
                  <button
                    className="btn btn-sm"
                    onClick={() => {
                      if (selectedContact === contact.public_key) {
                        setSelectedContact(null)
                        setAuditResult(null)
                      } else {
                        handleAudit(contact.public_key)
                      }
                    }}
                  >
                    {selectedContact === contact.public_key ? 'Close' : 'Manage'}
                  </button>
                  <button
                    className="btn btn-sm text-gruvbox-red border-gruvbox-red/30 hover:bg-gruvbox-red/10"
                    onClick={() => handleRevoke(contact.public_key)}
                    disabled={revoking === contact.public_key}
                  >
                    {revoking === contact.public_key ? '...' : 'Revoke'}
                  </button>
                </div>
              </div>

              {/* Expanded contact detail: role assignment + audit */}
              {selectedContact === contact.public_key && (
                <div className="mt-3 pt-3 border-t border-border">
                  {/* Role assignment */}
                  <h4 className="text-xs font-medium text-secondary mb-2">Assign Roles</h4>
                  <div className="flex flex-wrap gap-2 mb-3">
                    {Object.values(availableRoles).map((role) => {
                      const isActive = contact.roles?.[role.domain] === role.name
                      return (
                        <button
                          key={role.name}
                          className={`text-xs px-2 py-1 rounded border transition-colors ${
                            isActive
                              ? 'bg-gruvbox-blue/20 border-gruvbox-blue text-gruvbox-blue'
                              : 'border-border text-secondary hover:border-gruvbox-blue hover:text-primary'
                          }`}
                          onClick={() => {
                            if (isActive) {
                              handleRemoveRole(contact.public_key, role.domain)
                            } else {
                              handleAssignRole(contact.public_key, role.name)
                            }
                          }}
                          disabled={assigningRole}
                          title={`${role.description} (${role.domain} domain, tier ${role.tier})`}
                        >
                          {role.name.replace(/_/g, ' ')}
                        </button>
                      )
                    })}
                  </div>

                  {/* Sharing audit */}
                  <h4 className="text-xs font-medium text-secondary mb-2">
                    What {contact.display_name} can see
                  </h4>
                  {auditLoading && (
                    <div className="text-center py-4">
                      <div className="w-4 h-4 border-2 border-border border-t-primary rounded-full animate-spin mx-auto" />
                    </div>
                  )}
                  {auditResult && !auditLoading && (
                    <div>
                      {auditResult.accessible_schemas.length === 0 ? (
                        <p className="text-xs text-tertiary">No data accessible. Assign a role to start sharing.</p>
                      ) : (
                        <>
                          <p className="text-xs text-secondary mb-2">
                            {auditResult.total_readable} readable field{auditResult.total_readable !== 1 ? 's' : ''} across {auditResult.accessible_schemas.length} schema{auditResult.accessible_schemas.length !== 1 ? 's' : ''}
                          </p>
                          <div className="space-y-1 max-h-48 overflow-y-auto">
                            {auditResult.accessible_schemas.map((schema) => (
                              <div key={schema.schema_name} className="text-xs p-2 bg-surface-secondary rounded">
                                <div className="flex items-center gap-2">
                                  <span className="font-medium text-primary">{schema.schema_name}</span>
                                  <span className="text-tertiary">({schema.trust_domain})</span>
                                </div>
                                <span className="text-gruvbox-green">
                                  {schema.readable_fields.length} readable
                                </span>
                                {schema.writable_fields.length > 0 && (
                                  <span className="text-gruvbox-yellow ml-2">
                                    {schema.writable_fields.length} writable
                                  </span>
                                )}
                              </div>
                            ))}
                          </div>
                        </>
                      )}
                    </div>
                  )}
                </div>
              )}
            </div>
          ))}
        </div>
      )}
    </>
  )
}
