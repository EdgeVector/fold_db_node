import { useState, useRef, useEffect } from 'react'

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
}

/**
 * EmailSignupFlow — 3-state component:
 * 1. Email input
 * 2. 6-digit code verification
 * 3. Success (returns credentials to parent)
 */
export default function EmailSignupFlow({ onSuccess, onCancel }) {
  const [step, setStep] = useState('email') // 'email' | 'code' | 'success'
  const [email, setEmail] = useState('')
  const [code, setCode] = useState(['', '', '', '', '', ''])
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState(null)
  const [resendCooldown, setResendCooldown] = useState(0)
  const codeInputs = useRef([])

  // Resend cooldown timer
  useEffect(() => {
    if (resendCooldown <= 0) return
    const timer = setTimeout(() => setResendCooldown(c => c - 1), 1000)
    return () => clearTimeout(timer)
  }, [resendCooldown])

  const handleSendCode = async () => {
    if (!email || !email.includes('@')) {
      setError('Please enter a valid email address')
      return
    }
    setLoading(true)
    setError(null)
    try {
      const resp = await fetch('/api/auth/magic-link/start', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ email }),
      })
      const data = await resp.json()
      if (data.ok) {
        setStep('code')
        setResendCooldown(60)
      } else {
        setError(data.error || 'Failed to send verification code')
      }
    } catch (e) {
      setError(e?.message || 'Network error')
    } finally {
      setLoading(false)
    }
  }

  const handleVerifyCode = async () => {
    const codeStr = code.join('')
    if (codeStr.length !== 6) {
      setError('Please enter the full 6-digit code')
      return
    }
    setLoading(true)
    setError(null)
    try {
      const resp = await fetch('/api/auth/magic-link/verify', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ email, code: codeStr }),
      })
      const data = await resp.json()
      if (data.ok) {
        // Generate AES-256 encryption key via Web Crypto API
        const cryptoKey = await crypto.subtle.generateKey(
          { name: 'AES-GCM', length: 256 },
          true,
          ['encrypt', 'decrypt']
        )
        const exported = await crypto.subtle.exportKey('raw', cryptoKey)
        const encKeyBase64 = btoa(String.fromCharCode(...new Uint8Array(exported)))

        // Store full credentials (including encryption key) in keychain
        await fetch('/api/auth/credentials', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({
            user_hash: data.user_hash,
            session_token: data.session_token,
            api_key: data.api_key,
            encryption_key: encKeyBase64,
          }),
        })

        setStep('success')
        if (onSuccess) {
          onSuccess({
            user_hash: data.user_hash,
            session_token: data.session_token,
            api_key: data.api_key,
            encryption_key: encKeyBase64,
            is_new_user: data.is_new_user,
          })
        }
      } else {
        setError(data.error || 'Verification failed')
      }
    } catch (e) {
      setError(e?.message || 'Network error')
    } finally {
      setLoading(false)
    }
  }

  const handleResend = async () => {
    if (resendCooldown > 0) return
    await handleSendCode()
  }

  const handleCodeChange = (index, value) => {
    if (value.length > 1) {
      // Handle paste
      const digits = value.replace(/\D/g, '').slice(0, 6)
      const newCode = [...code]
      for (let i = 0; i < digits.length && index + i < 6; i++) {
        newCode[index + i] = digits[i]
      }
      setCode(newCode)
      const nextIndex = Math.min(index + digits.length, 5)
      codeInputs.current[nextIndex]?.focus()
      return
    }
    if (value && !/^\d$/.test(value)) return
    const newCode = [...code]
    newCode[index] = value
    setCode(newCode)
    if (value && index < 5) {
      codeInputs.current[index + 1]?.focus()
    }
  }

  const handleCodeKeyDown = (index, e) => {
    if (e.key === 'Backspace' && !code[index] && index > 0) {
      codeInputs.current[index - 1]?.focus()
    }
    if (e.key === 'Enter') {
      handleVerifyCode()
    }
  }

  // Email step
  if (step === 'email') {
    return (
      <div style={{ width: '100%' }}>
        <p style={{ fontSize: '13px', color: colors.dim, marginBottom: '16px' }}>
          Enter your email to create an Exemem account or sign in.
        </p>
        <input
          type="email"
          placeholder="you@example.com"
          value={email}
          onChange={e => setEmail(e.target.value)}
          onKeyDown={e => e.key === 'Enter' && handleSendCode()}
          disabled={loading}
          style={{
            width: '100%',
            padding: '10px 12px',
            background: colors.bg,
            border: `1px solid ${colors.border}`,
            color: colors.text,
            fontFamily: "'IBM Plex Mono', monospace",
            fontSize: '14px',
            outline: 'none',
            boxSizing: 'border-box',
          }}
        />
        {error && (
          <div style={{ marginTop: '8px', fontSize: '12px', color: colors.red }}>
            {error}
          </div>
        )}
        <div style={{ display: 'flex', gap: '8px', marginTop: '16px' }}>
          {onCancel && (
            <button
              onClick={onCancel}
              disabled={loading}
              style={{
                padding: '8px 16px',
                background: 'transparent',
                border: `1px solid ${colors.border}`,
                color: colors.dim,
                fontFamily: "'IBM Plex Mono', monospace",
                fontSize: '13px',
                cursor: 'pointer',
              }}
            >
              Back
            </button>
          )}
          <button
            onClick={handleSendCode}
            disabled={loading || !email}
            style={{
              flex: 1,
              padding: '8px 16px',
              background: colors.blue,
              border: 'none',
              color: colors.bg,
              fontFamily: "'IBM Plex Mono', monospace",
              fontSize: '13px',
              fontWeight: 700,
              cursor: loading ? 'wait' : 'pointer',
              opacity: loading || !email ? 0.6 : 1,
            }}
          >
            {loading ? 'Sending...' : 'Send Code'}
          </button>
        </div>
      </div>
    )
  }

  // Code verification step
  if (step === 'code') {
    return (
      <div style={{ width: '100%' }}>
        <p style={{ fontSize: '13px', color: colors.dim, marginBottom: '4px' }}>
          Enter the 6-digit code sent to:
        </p>
        <p style={{ fontSize: '13px', color: colors.textBright, marginBottom: '16px', fontWeight: 700 }}>
          {email}
        </p>
        <div style={{ display: 'flex', gap: '8px', justifyContent: 'center', marginBottom: '16px' }}>
          {code.map((digit, i) => (
            <input
              key={i}
              ref={el => codeInputs.current[i] = el}
              type="text"
              inputMode="numeric"
              maxLength={6}
              value={digit}
              onChange={e => handleCodeChange(i, e.target.value)}
              onKeyDown={e => handleCodeKeyDown(i, e)}
              disabled={loading}
              style={{
                width: '40px',
                height: '48px',
                textAlign: 'center',
                fontSize: '20px',
                fontWeight: 700,
                background: colors.bg,
                border: `1px solid ${digit ? colors.orange : colors.border}`,
                color: colors.orange,
                fontFamily: "'IBM Plex Mono', monospace",
                outline: 'none',
              }}
            />
          ))}
        </div>
        {error && (
          <div style={{ marginTop: '8px', marginBottom: '8px', fontSize: '12px', color: colors.red }}>
            {error}
          </div>
        )}
        <button
          onClick={handleVerifyCode}
          disabled={loading || code.join('').length !== 6}
          style={{
            width: '100%',
            padding: '10px',
            background: colors.orange,
            border: 'none',
            color: colors.bg,
            fontFamily: "'IBM Plex Mono', monospace",
            fontSize: '13px',
            fontWeight: 700,
            cursor: loading ? 'wait' : 'pointer',
            opacity: loading || code.join('').length !== 6 ? 0.6 : 1,
            marginBottom: '12px',
          }}
        >
          {loading ? 'Verifying...' : 'Verify'}
        </button>
        <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
          <button
            onClick={() => { setStep('email'); setCode(['', '', '', '', '', '']); setError(null) }}
            style={{
              background: 'transparent',
              border: 'none',
              color: colors.dim,
              fontFamily: "'IBM Plex Mono', monospace",
              fontSize: '12px',
              cursor: 'pointer',
              padding: 0,
              textDecoration: 'underline',
            }}
          >
            Change email
          </button>
          <button
            onClick={handleResend}
            disabled={resendCooldown > 0 || loading}
            style={{
              background: 'transparent',
              border: 'none',
              color: resendCooldown > 0 ? colors.dim : colors.blue,
              fontFamily: "'IBM Plex Mono', monospace",
              fontSize: '12px',
              cursor: resendCooldown > 0 ? 'default' : 'pointer',
              padding: 0,
              textDecoration: resendCooldown > 0 ? 'none' : 'underline',
            }}
          >
            {resendCooldown > 0 ? `Resend in ${resendCooldown}s` : 'Resend code'}
          </button>
        </div>
      </div>
    )
  }

  // Success step
  return (
    <div style={{ width: '100%', textAlign: 'center' }}>
      <div style={{ fontSize: '32px', marginBottom: '12px' }}>&#10003;</div>
      <p style={{ fontSize: '14px', color: colors.green, fontWeight: 700, marginBottom: '8px' }}>
        Account verified!
      </p>
      <p style={{ fontSize: '13px', color: colors.dim }}>
        Setting up your cloud database...
      </p>
    </div>
  )
}
