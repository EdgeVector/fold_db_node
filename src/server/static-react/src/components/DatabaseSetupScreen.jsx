import { useState } from 'react'
import { applySetup } from '../api/clients/systemClient'

const colors = {
  bg: '#282828',
  bgElevated: '#3c3836',
  border: '#504945',
  text: '#ebdbb2',
  textBright: '#fbf1c7',
  dim: '#928374',
  orange: '#fe8019',
  green: '#b8bb26',
  blue: '#83a598',
  red: '#fb4934',
  purple: '#d3869b',
}

export default function DatabaseSetupScreen({ onComplete }) {
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState(null)
  const [showInviteInput, setShowInviteInput] = useState(false)
  const [inviteCode, setInviteCode] = useState('')
  const [showRestore, setShowRestore] = useState(false)
  const [recoveryPhrase, setRecoveryPhrase] = useState('')

  const handleLocalSetup = async () => {
    setLoading(true)
    setError(null)
    const defaultPath = '~/.folddb/data'
    try {
      const response = await applySetup({
        storage: { type: 'local', path: defaultPath },
      })
      if (response.success) {
        onComplete()
      } else {
        setError(response.data?.message || 'Setup failed')
      }
    } catch (e) {
      const msg = e?.message || String(e)
      if (msg.includes('could not acquire lock') || msg.includes('WouldBlock')) {
        setError(
          'Another FoldDB instance is already running and holds the database lock. ' +
          'Please close the other instance and try again.'
        )
      } else {
        setError(msg)
      }
    } finally {
      setLoading(false)
    }
  }

  const handleCloudSetup = async () => {
    if (!showInviteInput) {
      setShowInviteInput(true)
      setShowRestore(false)
      return
    }
    if (!inviteCode.trim()) {
      setError('Invite code is required')
      return
    }
    setLoading(true)
    setError(null)
    try {
      const resp = await fetch('/api/auth/register', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ invite_code: inviteCode.trim() }),
      })
      const data = await resp.json()

      if (!data.ok) {
        throw new Error(data.error || 'Registration failed')
      }

      localStorage.setItem('exemem_api_url', data.api_url)
      localStorage.setItem('exemem_api_key', data.api_key)

      const response = await applySetup({
        storage: {
          type: 'exemem',
          api_url: data.api_url,
          api_key: data.api_key,
        },
      })
      if (response.success) {
        onComplete()
      } else {
        setError(response.data?.message || 'Setup failed')
      }
    } catch (e) {
      setError(e?.message || String(e))
    } finally {
      setLoading(false)
    }
  }

  const handleRestore = async () => {
    const words = recoveryPhrase.trim().toLowerCase()
    const wordCount = words.split(/\s+/).length
    if (wordCount !== 24) {
      setError(`Recovery phrase must be 24 words (got ${wordCount})`)
      return
    }
    setLoading(true)
    setError(null)
    try {
      const resp = await fetch('/api/auth/restore', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ words }),
      })
      const data = await resp.json()

      if (!data.ok) {
        throw new Error(data.error || 'Restore failed')
      }

      localStorage.setItem('exemem_api_url', data.api_url)
      localStorage.setItem('exemem_api_key', data.api_key)

      const response = await applySetup({
        storage: {
          type: 'exemem',
          api_url: data.api_url,
          api_key: data.api_key,
        },
      })
      if (response.success) {
        onComplete()
      } else {
        setError(response.data?.message || 'Setup failed')
      }
    } catch (e) {
      setError(e?.message || String(e))
    } finally {
      setLoading(false)
    }
  }

  const inputStyle = {
    width: '100%', padding: '8px 12px', fontSize: '14px',
    fontFamily: "'IBM Plex Mono', monospace",
    background: colors.bgElevated, border: `1px solid ${colors.border}`,
    color: colors.textBright, outline: 'none', boxSizing: 'border-box',
  }

  const btnStyle = (color, enabled = true) => ({
    flex: 1, padding: '8px', fontSize: '12px', fontWeight: 700,
    cursor: enabled ? 'pointer' : 'not-allowed',
    background: enabled ? color : colors.bgElevated,
    border: `1px solid ${enabled ? color : colors.border}`,
    color: enabled ? colors.bg : colors.dim,
    fontFamily: "'IBM Plex Mono', monospace",
    opacity: enabled ? 1 : 0.5,
  })

  return (
    <div style={{
      position: 'fixed', top: 0, left: 0, width: '100%', height: '100%',
      background: colors.bg, zIndex: 1000,
      display: 'flex', alignItems: 'center', justifyContent: 'center',
      fontFamily: "'IBM Plex Mono', monospace", color: colors.text,
    }}>
      <div style={{ maxWidth: '600px', width: '90%', textAlign: 'center' }}>
        <h1 style={{ fontSize: '24px', fontWeight: 700, color: colors.textBright, marginBottom: '8px' }}>
          FoldDB
        </h1>
        <p style={{ fontSize: '14px', color: colors.dim, marginBottom: '32px' }}>
          Choose where to store your data
        </p>

        <div style={{ display: 'flex', gap: '16px', justifyContent: 'center', flexWrap: 'wrap' }}>
          {/* Local Storage Card */}
          <button
            onClick={handleLocalSetup}
            disabled={loading}
            style={{
              background: colors.bgElevated, border: `1px solid ${colors.border}`,
              padding: '24px', width: '250px', cursor: loading ? 'wait' : 'pointer',
              color: colors.text, textAlign: 'left',
              fontFamily: "'IBM Plex Mono', monospace",
              opacity: loading ? 0.6 : 1, transition: 'border-color 0.15s',
            }}
            onMouseEnter={e => { if (!loading) e.currentTarget.style.borderColor = colors.green }}
            onMouseLeave={e => { e.currentTarget.style.borderColor = colors.border }}
          >
            <div style={{ fontSize: '20px', marginBottom: '8px' }}>Local Storage</div>
            <div style={{
              display: 'inline-block', padding: '1px 6px', fontSize: '11px',
              fontWeight: 700, background: colors.green, color: colors.bg, marginBottom: '12px',
            }}>RECOMMENDED</div>
            <p style={{ fontSize: '13px', color: colors.dim, margin: 0, lineHeight: '1.5' }}>
              Store data on your device. Fast, private, works offline. Uses ~/.folddb/data.
            </p>
          </button>

          {/* Exemem Cloud Card */}
          <button
            onClick={handleCloudSetup}
            disabled={loading}
            style={{
              background: colors.bgElevated, border: `1px solid ${colors.border}`,
              padding: '24px', width: '250px', cursor: loading ? 'wait' : 'pointer',
              color: colors.text, textAlign: 'left',
              fontFamily: "'IBM Plex Mono', monospace",
              opacity: loading ? 0.6 : 1, transition: 'border-color 0.15s',
            }}
            onMouseEnter={e => { if (!loading) e.currentTarget.style.borderColor = colors.blue }}
            onMouseLeave={e => { e.currentTarget.style.borderColor = colors.border }}
          >
            <div style={{ fontSize: '20px', marginBottom: '8px' }}>Exemem Cloud</div>
            <div style={{
              display: 'inline-block', padding: '1px 6px', fontSize: '11px',
              fontWeight: 700, background: colors.blue, color: colors.bg, marginBottom: '12px',
            }}>CLOUD</div>
            <p style={{ fontSize: '13px', color: colors.dim, margin: 0, lineHeight: '1.5' }}>
              Store data in the cloud. Syncs across devices. Requires an Exemem account.
            </p>
          </button>
        </div>

        {/* Restore from recovery phrase link */}
        {!showInviteInput && !showRestore && (
          <p style={{ marginTop: '24px', fontSize: '13px' }}>
            <button
              onClick={() => { setShowRestore(true); setShowInviteInput(false); setError(null) }}
              disabled={loading}
              style={{
                background: 'none', border: 'none', color: colors.purple,
                cursor: 'pointer', fontFamily: "'IBM Plex Mono', monospace",
                fontSize: '13px', textDecoration: 'underline',
              }}
            >
              Restore from recovery phrase
            </button>
          </p>
        )}

        {/* Invite code input */}
        {showInviteInput && !loading && (
          <div style={{ marginTop: '24px', maxWidth: '300px', margin: '24px auto 0', textAlign: 'left' }}>
            <label style={{ fontSize: '12px', color: colors.dim, display: 'block', marginBottom: '4px' }}>
              Invite Code
            </label>
            <input
              type="text"
              value={inviteCode}
              onChange={(e) => setInviteCode(e.target.value.toUpperCase())}
              placeholder="EXM-XXXX-XXXX"
              style={{ ...inputStyle, letterSpacing: '2px' }}
              onFocus={e => { e.currentTarget.style.borderColor = colors.blue }}
              onBlur={e => { e.currentTarget.style.borderColor = colors.border }}
              autoFocus
            />
            <p style={{ fontSize: '11px', color: colors.dim, marginTop: '4px' }}>
              Get an invite code from an existing Exemem user.
            </p>
            <div style={{ display: 'flex', gap: '8px', marginTop: '12px' }}>
              <button
                onClick={() => { setShowInviteInput(false); setInviteCode(''); setError(null) }}
                style={{ ...btnStyle(colors.border, true), background: 'transparent', color: colors.dim }}
              >Back</button>
              <button
                onClick={handleCloudSetup}
                disabled={!inviteCode.trim()}
                style={btnStyle(colors.blue, !!inviteCode.trim())}
              >Continue</button>
            </div>
          </div>
        )}

        {/* Recovery phrase input */}
        {showRestore && !loading && (
          <div style={{ marginTop: '24px', maxWidth: '400px', margin: '24px auto 0', textAlign: 'left' }}>
            <label style={{ fontSize: '12px', color: colors.dim, display: 'block', marginBottom: '4px' }}>
              Recovery Phrase (24 words)
            </label>
            <textarea
              value={recoveryPhrase}
              onChange={(e) => setRecoveryPhrase(e.target.value.toLowerCase())}
              placeholder="Enter your 24-word recovery phrase..."
              rows={4}
              style={{
                ...inputStyle, resize: 'vertical', lineHeight: '1.6',
              }}
              onFocus={e => { e.currentTarget.style.borderColor = colors.purple }}
              onBlur={e => { e.currentTarget.style.borderColor = colors.border }}
              autoFocus
            />
            <p style={{ fontSize: '11px', color: colors.dim, marginTop: '4px' }}>
              Enter the 24 words you saved when you first set up your account.
            </p>
            <div style={{ display: 'flex', gap: '8px', marginTop: '12px' }}>
              <button
                onClick={() => { setShowRestore(false); setRecoveryPhrase(''); setError(null) }}
                style={{ ...btnStyle(colors.border, true), background: 'transparent', color: colors.dim }}
              >Back</button>
              <button
                onClick={handleRestore}
                disabled={recoveryPhrase.trim().split(/\s+/).length !== 24}
                style={btnStyle(colors.purple, recoveryPhrase.trim().split(/\s+/).length === 24)}
              >Restore</button>
            </div>
          </div>
        )}

        {loading && (
          <div style={{ marginTop: '24px', color: colors.dim, fontSize: '13px' }}>
            <div style={{
              width: '20px', height: '20px',
              border: `2px solid ${colors.border}`,
              borderTopColor: colors.green,
              borderRadius: '50%',
              animation: 'spin 0.8s linear infinite',
              margin: '0 auto 8px',
            }} />
            {showRestore ? 'Restoring identity...' : 'Initializing database...'}
            <style>{`@keyframes spin { to { transform: rotate(360deg) } }`}</style>
          </div>
        )}

        {error && (
          <div style={{
            marginTop: '24px', padding: '12px',
            background: `${colors.red}15`, border: `1px solid ${colors.red}`,
            fontSize: '13px', color: colors.red, textAlign: 'left',
          }}>
            {error}
          </div>
        )}
      </div>
    </div>
  )
}
