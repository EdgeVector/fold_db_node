import { useEffect, useRef, useState } from 'react'
import { ingestionClient } from '../../api/clients'
import type {
  RoleInfo,
  RoleMetricsSnapshot,
  TestRoleResponse,
  Role,
} from '../../api/clients/ingestionClient'

const STATS_POLL_MS = 5000

type TestResult = TestRoleResponse | { error: string }

export default function ActiveModelsTable() {
  const [roles, setRoles] = useState<RoleInfo[]>([])
  const [rolesError, setRolesError] = useState<string | null>(null)
  const [rolesLoading, setRolesLoading] = useState(true)
  const [stats, setStats] = useState<Record<string, RoleMetricsSnapshot>>({})
  const [expandedRole, setExpandedRole] = useState<Role | null>(null)
  const [testPrompts, setTestPrompts] = useState<Record<string, string>>({})
  const [testResults, setTestResults] = useState<Record<string, TestResult | null>>({})
  const [testingRole, setTestingRole] = useState<Role | null>(null)
  const pollIntervalRef = useRef<ReturnType<typeof setInterval> | null>(null)

  useEffect(() => {
    let cancelled = false
    async function load() {
      try {
        const resp = await ingestionClient.getRoles()
        if (cancelled) return
        const data = (resp.data ?? resp) as { roles?: RoleInfo[] }
        setRoles(data?.roles ?? [])
        setRolesError(null)
      } catch (err) {
        if (cancelled) return
        setRolesError(err instanceof Error ? err.message : String(err))
      } finally {
        if (!cancelled) setRolesLoading(false)
      }
    }
    load()
    return () => {
      cancelled = true
    }
  }, [])

  useEffect(() => {
    let mounted = true

    async function fetchStats() {
      try {
        const resp = await ingestionClient.getAiStats()
        if (!mounted) return
        const data = (resp.data ?? resp) as { stats?: RoleMetricsSnapshot[] }
        const byRole: Record<string, RoleMetricsSnapshot> = {}
        for (const snap of data?.stats ?? []) {
          byRole[snap.role as string] = snap
        }
        setStats(byRole)
      } catch {
        /* silent — stats are best-effort */
      }
    }

    function startPolling() {
      if (pollIntervalRef.current) return
      fetchStats()
      pollIntervalRef.current = setInterval(fetchStats, STATS_POLL_MS)
    }

    function stopPolling() {
      if (pollIntervalRef.current) {
        clearInterval(pollIntervalRef.current)
        pollIntervalRef.current = null
      }
    }

    function onVisibilityChange() {
      if (document.hidden) stopPolling()
      else startPolling()
    }

    startPolling()
    document.addEventListener('visibilitychange', onVisibilityChange)

    return () => {
      mounted = false
      stopPolling()
      document.removeEventListener('visibilitychange', onVisibilityChange)
    }
  }, [])

  async function handleTest(role: Role) {
    const prompt = (testPrompts[role as string] ?? '').trim()
    if (!prompt) return
    setTestingRole(role)
    setTestResults(prev => ({ ...prev, [role as string]: null }))
    try {
      const resp = await ingestionClient.testRole(role, prompt)
      const data = (resp.data ?? resp) as TestRoleResponse
      setTestResults(prev => ({ ...prev, [role as string]: data }))
    } catch (err) {
      setTestResults(prev => ({
        ...prev,
        [role as string]: { error: err instanceof Error ? err.message : String(err) },
      }))
    } finally {
      setTestingRole(null)
    }
  }

  function renderStats(role: Role) {
    const snap = stats[role as string]
    if (!snap || snap.call_count === 0) {
      return <span className="text-xs text-secondary">No calls yet</span>
    }
    const avg = snap.avg_latency_ms.toFixed(0)
    const errorClass =
      snap.error_count > 0 ? 'text-gruvbox-yellow' : 'text-secondary'
    return (
      <span className={`text-xs ${errorClass}`}>
        {snap.call_count} calls · avg {avg}ms · {snap.error_count} errs
      </span>
    )
  }

  function renderStatusDot(role: Role) {
    const info = roles.find(r => r.role === role)
    if (!info) return null
    if (info.status === 'ok') return null
    const label =
      info.status === 'missing_api_key'
        ? 'API key required'
        : info.status === 'ollama_not_configured'
          ? 'Ollama URL missing'
          : info.status
    return (
      <span
        className="text-xs text-gruvbox-red ml-1"
        title={`${info.display_name}: ${label}`}
      >
        • {label}
      </span>
    )
  }

  if (rolesLoading) {
    return (
      <div className="space-y-2">
        <div className="label">Active AI Models</div>
        <div className="card" aria-busy="true">
          <span className="text-xs text-secondary">Loading active models…</span>
        </div>
      </div>
    )
  }

  if (rolesError) {
    return (
      <div className="space-y-2">
        <div className="label">Active AI Models</div>
        <div className="card card-error">
          <span className="text-sm text-gruvbox-red">
            ✗ Failed to load AI roles: {rolesError}
          </span>
        </div>
      </div>
    )
  }

  return (
    <div
      role="region"
      aria-label="Active AI models"
      className="space-y-2"
    >
      <div className="flex items-baseline justify-between">
        <label className="label">Active AI Models</label>
        <span className="text-xs text-secondary">
          What each role is running right now
        </span>
      </div>
      <div className="card p-0 overflow-hidden">
        <div
          className="grid items-center gap-2 px-3 py-2 border-b border-border text-xs text-secondary"
          style={{ gridTemplateColumns: '9rem 1fr 11rem 6rem' }}
        >
          <span>Role</span>
          <span>Active Model</span>
          <span className="text-right">Stats</span>
          <span className="text-right">Actions</span>
        </div>
        {roles.map(info => {
          const expanded = expandedRole === info.role
          const result = testResults[info.role as string]
          const hasError = result && 'error' in result
          return (
            <div key={info.role as string} className="border-b border-border last:border-b-0">
              <div
                className="grid items-center gap-2 px-3 py-2"
                style={{ gridTemplateColumns: '9rem 1fr 11rem 6rem' }}
              >
                <span className="font-medium text-sm">{info.display_name}</span>
                <span className="text-sm">
                  <span className="text-secondary">{info.provider}</span>
                  <span className="mx-1 text-secondary">·</span>
                  <span>{info.model}</span>
                  {info.override_active && (
                    <span
                      className="text-xs text-gruvbox-yellow ml-2"
                      title="User override active"
                      aria-label="User override active"
                    >
                      [*]
                    </span>
                  )}
                  {renderStatusDot(info.role)}
                </span>
                <span className="text-right">{renderStats(info.role)}</span>
                <span className="text-right flex justify-end gap-2">
                  <button
                    type="button"
                    className="text-xs text-secondary hover:text-primary"
                    aria-label={`${expanded ? 'Collapse' : 'Details for'} ${info.display_name}`}
                    aria-expanded={expanded}
                    onClick={() =>
                      setExpandedRole(expanded ? null : info.role)
                    }
                  >
                    {expanded ? '▼' : '▶'}
                  </button>
                  {info.is_text_capable ? (
                    <button
                      type="button"
                      className="text-xs text-gruvbox-blue hover:underline disabled:text-secondary"
                      disabled={testingRole === info.role}
                      onClick={() => {
                        setTestPrompts(prev => ({
                          ...prev,
                          [info.role as string]: prev[info.role as string] ?? 'Say hello.',
                        }))
                        setExpandedRole(info.role)
                      }}
                    >
                      Test
                    </button>
                  ) : (
                    <span
                      className="text-xs text-secondary"
                      title="Vision/OCR tests require image upload (not supported in v1)"
                    >
                      Test
                    </span>
                  )}
                </span>
              </div>

              {expanded && (
                <div className="px-3 py-3 bg-surface-hover text-xs space-y-2">
                  <div>
                    <span className="text-secondary">Purpose:</span>{' '}
                    {info.doc}
                  </div>
                  <div>
                    <span className="text-secondary">Temperature:</span>{' '}
                    {info.generation_params.temperature?.toFixed(2)}
                    <span className="mx-2">·</span>
                    <span className="text-secondary">num_predict:</span>{' '}
                    {info.generation_params.num_predict}
                  </div>
                  {info.is_text_capable && (
                    <div className="space-y-2 pt-2 border-t border-border">
                      <label
                        htmlFor={`test-prompt-${info.role as string}`}
                        className="block text-secondary"
                      >
                        Test prompt
                      </label>
                      <textarea
                        id={`test-prompt-${info.role as string}`}
                        className="input w-full text-xs"
                        rows={2}
                        value={testPrompts[info.role as string] ?? ''}
                        onChange={e =>
                          setTestPrompts(prev => ({
                            ...prev,
                            [info.role as string]: e.target.value,
                          }))
                        }
                      />
                      <div className="flex gap-2">
                        <button
                          type="button"
                          className="text-xs text-gruvbox-blue hover:underline disabled:text-secondary"
                          disabled={
                            testingRole === info.role ||
                            !(testPrompts[info.role as string] ?? '').trim()
                          }
                          onClick={() => handleTest(info.role)}
                        >
                          {testingRole === info.role ? 'Testing…' : 'Run test'}
                        </button>
                      </div>
                      {result && (
                        <div
                          className={`p-2 card ${hasError ? 'card-error' : 'card-success'}`}
                          role="status"
                          aria-live="polite"
                        >
                          {hasError ? (
                            <span className="text-gruvbox-red">
                              ✗ {(result as { error: string }).error}
                            </span>
                          ) : (
                            <div className="space-y-1">
                              <div className="text-secondary">
                                Model:{' '}
                                <span className="text-primary">
                                  {(result as TestRoleResponse).provider} ·{' '}
                                  {(result as TestRoleResponse).model}
                                </span>
                                <span className="mx-2">·</span>
                                <span>
                                  {(result as TestRoleResponse).latency_ms.toFixed(0)}
                                  ms
                                </span>
                              </div>
                              <pre className="whitespace-pre-wrap break-words text-primary max-h-40 overflow-auto">
                                {(result as TestRoleResponse).response}
                              </pre>
                            </div>
                          )}
                        </div>
                      )}
                    </div>
                  )}
                </div>
              )}
            </div>
          )
        })}
      </div>
    </div>
  )
}
