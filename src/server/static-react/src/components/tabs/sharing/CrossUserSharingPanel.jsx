import { useCallback, useEffect, useMemo, useState } from 'react'
import { ClipboardIcon, LockClosedIcon } from '@heroicons/react/24/outline'
import {
  listShareRules,
  createShareRule,
  deactivateShareRule,
  generateShareInvite,
  acceptShareInvite,
  listPendingShareInvites,
} from '../../../api/clients/sharingClient'
import { listContacts } from '../../../api/clients/trustClient'
import { getAllSchemasWithState } from '../../../api/clients/schemaClient'

/**
 * Cross-User Sharing panel.
 *
 * Shows the caller's outgoing ShareRules, lets them create new rules
 * scoped to a contact + schema/field, generate an invite for a rule
 * (which the backend pushes to the bulletin board if discovery is
 * configured), and accept incoming invites that have been polled into
 * the local pending-invites queue.
 *
 * Wraps the endpoints under /api/sharing/* defined in
 * src/handlers/sharing.rs and src/server/routes/sharing.rs.
 */

function truncate(hex, head = 8, tail = 4) {
  if (!hex) return ''
  if (hex.length <= head + tail + 1) return hex
  return `${hex.slice(0, head)}…${hex.slice(-tail)}`
}

function describeScope(scope) {
  if (scope === 'AllSchemas' || scope?.AllSchemas !== undefined) {
    return 'All my data'
  }
  if (scope?.Schema !== undefined) {
    return `Schema: ${scope.Schema}`
  }
  if (scope?.SchemaField !== undefined) {
    const [schema, field] = scope.SchemaField
    return `Field: ${schema}.${field}`
  }
  return JSON.stringify(scope)
}

export default function CrossUserSharingPanel() {
  const [rules, setRules] = useState([])
  const [rulesLoading, setRulesLoading] = useState(true)
  const [rulesError, setRulesError] = useState(null)

  const [contacts, setContacts] = useState([])
  const [schemas, setSchemas] = useState([])

  const [pending, setPending] = useState([])
  const [pendingLoading, setPendingLoading] = useState(true)
  const [pendingError, setPendingError] = useState(null)

  const [toast, setToast] = useState(null)
  useEffect(() => {
    if (!toast) return
    const t = setTimeout(() => setToast(null), 4000)
    return () => clearTimeout(t)
  }, [toast])

  const loadRules = useCallback(async () => {
    setRulesLoading(true)
    setRulesError(null)
    try {
      const res = await listShareRules()
      if (res.success && res.data) {
        setRules(res.data.rules ?? [])
      } else {
        setRulesError(res.error ?? 'Failed to load share rules')
      }
    } catch (e) {
      setRulesError(e?.message ?? 'Network error')
    } finally {
      setRulesLoading(false)
    }
  }, [])

  const loadPending = useCallback(async () => {
    setPendingLoading(true)
    setPendingError(null)
    try {
      const res = await listPendingShareInvites()
      if (res.success && res.data) {
        setPending(res.data.invites ?? [])
      } else {
        setPendingError(res.error ?? 'Failed to load pending invites')
      }
    } catch (e) {
      setPendingError(e?.message ?? 'Network error')
    } finally {
      setPendingLoading(false)
    }
  }, [])

  const loadContacts = useCallback(async () => {
    try {
      const res = await listContacts()
      if (res.success && res.data) {
        setContacts((res.data.contacts ?? []).filter(c => !c.revoked))
      }
    } catch {
      /* non-fatal: contact dropdown just stays empty */
    }
  }, [])

  const loadSchemas = useCallback(async () => {
    try {
      const res = await getAllSchemasWithState()
      if (res.success && res.data) {
        setSchemas(Object.keys(res.data))
      }
    } catch {
      /* non-fatal */
    }
  }, [])

  useEffect(() => {
    loadRules()
    loadPending()
    loadContacts()
    loadSchemas()
  }, [loadRules, loadPending, loadContacts, loadSchemas])

  return (
    <div className="space-y-6" data-testid="cross-user-sharing-panel">
      {toast && (
        <div
          className="px-3 py-2 rounded bg-gruvbox-green/20 border border-gruvbox-green/40 text-sm"
          data-testid="sharing-toast"
        >
          {toast}
        </div>
      )}

      <CreateRuleForm
        contacts={contacts}
        schemas={schemas}
        onCreated={rule => {
          setToast(`Share rule created for ${rule.recipient_display_name}`)
          loadRules()
        }}
      />

      <MyRulesSection
        rules={rules}
        loading={rulesLoading}
        error={rulesError}
        onRefresh={loadRules}
        onGenerateInvite={async rule => {
          try {
            const res = await generateShareInvite({
              rule_id: rule.rule_id,
              scope_description: describeScope(rule.scope),
            })
            if (res.success && res.data) {
              // Fallback: copy the invite JSON so the user can paste
              // it manually if the bulletin-board delivery fails.
              try {
                await navigator.clipboard?.writeText(
                  JSON.stringify(res.data.invite, null, 2),
                )
              } catch {
                /* clipboard API unavailable — fine, not a blocker */
              }
              setToast(
                `Invite generated for ${rule.recipient_display_name}. JSON copied to clipboard as fallback.`,
              )
            } else {
              setToast(res.error ?? 'Failed to generate invite')
            }
          } catch (e) {
            setToast(e?.message ?? 'Network error')
          }
        }}
        onDeactivate={async rule => {
          if (!window.confirm(`Deactivate share rule for ${rule.recipient_display_name}?`)) {
            return
          }
          try {
            const res = await deactivateShareRule(rule.rule_id)
            if (res.success) {
              setToast('Share rule deactivated')
              loadRules()
            } else {
              setToast(res.error ?? 'Failed to deactivate')
            }
          } catch (e) {
            setToast(e?.message ?? 'Network error')
          }
        }}
      />

      <PendingInvitesSection
        pending={pending}
        loading={pendingLoading}
        error={pendingError}
        onRefresh={loadPending}
        onAccept={async invite => {
          try {
            const res = await acceptShareInvite(invite)
            if (res.success) {
              setToast(`Accepted invite from ${invite.sender_display_name}`)
              // Remove from local list optimistically; the server
              // promotes the invite into a subscription on accept.
              setPending(p => p.filter(i => i.share_prefix !== invite.share_prefix))
            } else {
              setToast(res.error ?? 'Failed to accept invite')
            }
          } catch (e) {
            setToast(e?.message ?? 'Network error')
          }
        }}
      />
    </div>
  )
}

function CreateRuleForm({ contacts, schemas, onCreated }) {
  const [recipientPubkey, setRecipientPubkey] = useState('')
  const [recipientName, setRecipientName] = useState('')
  const [scopeKind, setScopeKind] = useState('AllSchemas')
  const [schemaName, setSchemaName] = useState('')
  const [fieldName, setFieldName] = useState('')
  const [submitting, setSubmitting] = useState(false)
  const [error, setError] = useState(null)

  // When the user picks a contact from the dropdown, auto-fill
  // pubkey + display_name so they don't have to type either.
  const onPickContact = useCallback(
    e => {
      const pk = e.target.value
      setRecipientPubkey(pk)
      const c = contacts.find(x => x.public_key === pk)
      if (c) setRecipientName(c.display_name || '')
    },
    [contacts],
  )

  const canSubmit = useMemo(() => {
    if (!recipientPubkey || !recipientName) return false
    if (scopeKind === 'Schema' && !schemaName) return false
    if (scopeKind === 'SchemaField' && (!schemaName || !fieldName)) return false
    return !submitting
  }, [recipientPubkey, recipientName, scopeKind, schemaName, fieldName, submitting])

  const submit = async e => {
    e.preventDefault()
    if (!canSubmit) return
    setSubmitting(true)
    setError(null)
    let scope
    if (scopeKind === 'AllSchemas') scope = 'AllSchemas'
    else if (scopeKind === 'Schema') scope = { Schema: schemaName }
    else scope = { SchemaField: [schemaName, fieldName] }

    try {
      const res = await createShareRule({
        recipient_pubkey: recipientPubkey,
        recipient_display_name: recipientName,
        scope,
      })
      if (res.success && res.data) {
        onCreated(res.data.rule)
        // Reset form on success
        setRecipientPubkey('')
        setRecipientName('')
        setScopeKind('AllSchemas')
        setSchemaName('')
        setFieldName('')
      } else {
        setError(res.error ?? 'Failed to create share rule')
      }
    } catch (err) {
      setError(err?.message ?? 'Network error')
    } finally {
      setSubmitting(false)
    }
  }

  return (
    <section className="card p-4" data-testid="create-rule-form">
      <h3 className="text-base font-semibold mb-3">Create a new share rule</h3>
      <form onSubmit={submit} className="space-y-3">
        <div>
          <label className="text-xs text-secondary block mb-1">
            Recipient contact
          </label>
          <select
            value={recipientPubkey}
            onChange={onPickContact}
            className="input text-sm w-full"
            data-testid="create-rule-contact"
          >
            <option value="">— pick a contact —</option>
            {contacts.map(c => (
              <option key={c.public_key} value={c.public_key}>
                {c.display_name || '(unnamed)'} · {truncate(c.public_key)}
              </option>
            ))}
          </select>
        </div>

        <div className="grid grid-cols-2 gap-2">
          <div>
            <label className="text-xs text-secondary block mb-1">Public key</label>
            <input
              type="text"
              value={recipientPubkey}
              onChange={e => setRecipientPubkey(e.target.value)}
              placeholder="hex-encoded pubkey"
              className="input text-sm w-full font-mono"
              data-testid="create-rule-pubkey"
            />
          </div>
          <div>
            <label className="text-xs text-secondary block mb-1">Display name</label>
            <input
              type="text"
              value={recipientName}
              onChange={e => setRecipientName(e.target.value)}
              placeholder="e.g. Alice"
              className="input text-sm w-full"
              data-testid="create-rule-name"
            />
          </div>
        </div>

        <fieldset className="border border-border rounded p-2">
          <legend className="text-xs text-secondary px-1">Scope</legend>
          <div className="space-y-1 text-sm">
            <label className="flex items-center gap-2">
              <input
                type="radio"
                name="scope"
                value="AllSchemas"
                checked={scopeKind === 'AllSchemas'}
                onChange={() => setScopeKind('AllSchemas')}
                data-testid="create-rule-scope-all"
              />
              <span>All my data</span>
            </label>
            <label className="flex items-center gap-2">
              <input
                type="radio"
                name="scope"
                value="Schema"
                checked={scopeKind === 'Schema'}
                onChange={() => setScopeKind('Schema')}
                data-testid="create-rule-scope-schema"
              />
              <span>One schema</span>
            </label>
            <label className="flex items-center gap-2">
              <input
                type="radio"
                name="scope"
                value="SchemaField"
                checked={scopeKind === 'SchemaField'}
                onChange={() => setScopeKind('SchemaField')}
                data-testid="create-rule-scope-field"
              />
              <span>One field of one schema</span>
            </label>
          </div>

          {(scopeKind === 'Schema' || scopeKind === 'SchemaField') && (
            <div className="mt-2 grid grid-cols-2 gap-2">
              <div>
                <label className="text-xs text-secondary block mb-1">Schema</label>
                <select
                  value={schemaName}
                  onChange={e => setSchemaName(e.target.value)}
                  className="input text-sm w-full"
                  data-testid="create-rule-schema"
                >
                  <option value="">— pick a schema —</option>
                  {schemas.map(s => (
                    <option key={s} value={s}>
                      {s}
                    </option>
                  ))}
                </select>
              </div>
              {scopeKind === 'SchemaField' && (
                <div>
                  <label className="text-xs text-secondary block mb-1">Field name</label>
                  <input
                    type="text"
                    value={fieldName}
                    onChange={e => setFieldName(e.target.value)}
                    placeholder="field_name"
                    className="input text-sm w-full font-mono"
                    data-testid="create-rule-field"
                  />
                </div>
              )}
            </div>
          )}
        </fieldset>

        {error && (
          <div className="text-sm text-gruvbox-red" data-testid="create-rule-error">
            {error}
          </div>
        )}

        <div>
          <button
            type="submit"
            className="btn-primary text-sm"
            disabled={!canSubmit}
            data-testid="create-rule-submit"
          >
            {submitting ? 'Creating…' : 'Create share rule'}
          </button>
        </div>
      </form>
    </section>
  )
}

function MyRulesSection({ rules, loading, error, onRefresh, onGenerateInvite, onDeactivate }) {
  return (
    <section className="card p-4" data-testid="my-rules-section">
      <div className="flex items-center justify-between mb-3">
        <h3 className="text-base font-semibold">My share rules</h3>
        <button
          type="button"
          className="btn-secondary text-xs"
          onClick={onRefresh}
          disabled={loading}
          data-testid="my-rules-refresh"
        >
          {loading ? 'Loading…' : 'Refresh'}
        </button>
      </div>

      {error && (
        <div className="text-sm text-gruvbox-red" data-testid="my-rules-error">
          {error}
        </div>
      )}

      {!loading && !error && rules.length === 0 && (
        <div className="text-sm text-secondary" data-testid="my-rules-empty">
          No share rules yet. Use the form above to share a schema with a contact.
        </div>
      )}

      {rules.length > 0 && (
        <ul className="space-y-2">
          {rules.map(rule => (
            <li
              key={rule.rule_id}
              className="border border-border rounded p-3 bg-surface"
              data-testid={`rule-row-${rule.rule_id}`}
            >
              <div className="flex items-start justify-between gap-3">
                <div className="min-w-0 flex-1">
                  <div className="flex items-center gap-2 flex-wrap">
                    <span className="font-medium">{rule.recipient_display_name}</span>
                    <span
                      className="text-[10px] px-2 py-0.5 rounded-full bg-gruvbox-green/10 text-gruvbox-green border border-gruvbox-green/30"
                      title={`Signature: ${rule.signature.slice(0, 8)}…`}
                      data-testid={`rule-signed-${rule.rule_id}`}
                    >
                      ✓ Signed
                    </span>
                    {rule.active ? (
                      <span className="text-[10px] px-2 py-0.5 rounded-full bg-gruvbox-blue/10 text-gruvbox-blue border border-gruvbox-blue/30">
                        active
                      </span>
                    ) : (
                      <span className="text-[10px] px-2 py-0.5 rounded-full bg-gruvbox-red/10 text-gruvbox-red border border-gruvbox-red/30">
                        revoked
                      </span>
                    )}
                  </div>
                  <div className="text-xs text-secondary mt-1">
                    {describeScope(rule.scope)}
                  </div>
                  <div className="text-[11px] text-tertiary font-mono mt-1 flex items-center gap-1 break-all">
                    <span>{truncate(rule.share_prefix, 20, 8)}</span>
                    <button
                      type="button"
                      className="text-tertiary hover:text-primary"
                      title="Copy recipient pubkey"
                      aria-label="Copy recipient pubkey"
                      onClick={() =>
                        navigator.clipboard?.writeText(rule.recipient_pubkey)
                      }
                      data-testid={`rule-copy-pubkey-${rule.rule_id}`}
                    >
                      <ClipboardIcon aria-hidden="true" className="w-4 h-4" />
                    </button>
                  </div>
                </div>
                <div className="flex flex-col gap-1 shrink-0">
                  {rule.active && (
                    <>
                      <button
                        type="button"
                        className="btn-secondary text-xs"
                        onClick={() => onGenerateInvite(rule)}
                        data-testid={`rule-invite-${rule.rule_id}`}
                      >
                        Generate invite
                      </button>
                      <button
                        type="button"
                        className="btn-secondary text-xs text-gruvbox-red"
                        onClick={() => onDeactivate(rule)}
                        data-testid={`rule-deactivate-${rule.rule_id}`}
                      >
                        Deactivate
                      </button>
                    </>
                  )}
                </div>
              </div>
            </li>
          ))}
        </ul>
      )}
    </section>
  )
}

function PendingInvitesSection({ pending, loading, error, onRefresh, onAccept }) {
  return (
    <section className="card p-4" data-testid="pending-invites-section">
      <div className="flex items-center justify-between mb-3">
        <h3 className="text-base font-semibold">Pending incoming invites</h3>
        <button
          type="button"
          className="btn-secondary text-xs"
          onClick={onRefresh}
          disabled={loading}
          data-testid="pending-invites-refresh"
        >
          {loading ? 'Loading…' : 'Refresh'}
        </button>
      </div>

      {error && (
        <div className="text-sm text-gruvbox-red" data-testid="pending-invites-error">
          {error}
        </div>
      )}

      {!loading && !error && pending.length === 0 && (
        <div className="text-sm text-secondary" data-testid="pending-invites-empty">
          No pending invites. Invites sent to you via the bulletin board will appear here
          after the inbound poller picks them up.
        </div>
      )}

      {pending.length > 0 && (
        <ul className="space-y-2">
          {pending.map((invite, i) => (
            <PendingInviteRow
              key={`${invite.share_prefix}-${i}`}
              invite={invite}
              onAccept={() => onAccept(invite)}
            />
          ))}
        </ul>
      )}
    </section>
  )
}

function PendingInviteRow({ invite, onAccept }) {
  const [expanded, setExpanded] = useState(false)
  return (
    <li
      className="border border-border rounded p-3 bg-surface"
      data-testid={`pending-invite-row-${invite.share_prefix}`}
    >
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2">
            <span className="font-medium">{invite.sender_display_name}</span>
            <span
              className="inline-flex items-center gap-1 text-[10px] px-2 py-0.5 rounded-full bg-gruvbox-aqua/10 text-gruvbox-aqua border border-gruvbox-aqua/30"
              title="This invite carries an encrypted share key"
            >
              <LockClosedIcon aria-hidden="true" className="w-3 h-3" />
              <span>E2E</span>
            </span>
          </div>
          <div className="text-xs text-secondary mt-1">{invite.scope_description}</div>
          {expanded && (
            <div className="text-[11px] text-tertiary font-mono mt-2 space-y-0.5 break-all">
              <div>
                sender: <span>{truncate(invite.sender_pubkey, 12, 6)}</span>
              </div>
              <div>
                prefix: <span>{invite.share_prefix}</span>
              </div>
              <div>key bytes: {invite.share_e2e_secret.length}</div>
            </div>
          )}
          <button
            type="button"
            className="text-[11px] text-tertiary underline underline-offset-2 mt-1"
            onClick={() => setExpanded(x => !x)}
            data-testid={`pending-invite-toggle-${invite.share_prefix}`}
          >
            {expanded ? 'Hide details' : 'Show details'}
          </button>
        </div>
        <div className="shrink-0">
          <button
            type="button"
            className="btn-primary text-xs"
            onClick={onAccept}
            data-testid={`pending-invite-accept-${invite.share_prefix}`}
          >
            Accept
          </button>
        </div>
      </div>
    </li>
  )
}
