import { useState, useCallback, useEffect } from 'react'
import { discoveryClient } from '../../api/clients/discoveryClient'
import { toErrorMessage } from '../../utils/schemaUtils'

const SECTION_ICONS = {
  search: '\u{1F50D}',
  users: '\u{1F465}',
  camera: '\u{1F4F8}',
  fingerprint: '\u{1F9EC}',
}

function DigestCard({ digest }) {
  const periodStart = new Date(digest.period_start)
  const periodEnd = new Date(digest.period_end)
  const generated = new Date(digest.generated_at)

  const formatDate = (d) =>
    d.toLocaleDateString(undefined, { month: 'short', day: 'numeric' })

  return (
    <div className="card rounded p-6 space-y-5">
      {/* Header */}
      <div className="flex items-center justify-between">
        <div>
          <h3 className="text-base font-semibold text-primary">
            Weekly Digest
          </h3>
          <p className="text-xs text-tertiary mt-0.5">
            {formatDate(periodStart)} &ndash; {formatDate(periodEnd)}
          </p>
        </div>
        <span className="text-xs text-tertiary bg-surface-secondary px-2 py-1 rounded">
          {digest.digest_id}
        </span>
      </div>

      {/* Summary */}
      <div className="card-info p-4 rounded">
        <p className="text-sm text-secondary">{digest.summary}</p>
      </div>

      {/* Sections */}
      {digest.sections.map((section, idx) => (
        <div key={idx} className="space-y-2">
          <h4 className="text-sm font-medium text-primary flex items-center gap-2">
            <span>{SECTION_ICONS[section.icon] || '\u{2022}'}</span>
            {section.title}
          </h4>
          <ul className="space-y-1.5 pl-6">
            {section.items.map((item, i) => (
              <li key={i} className="text-sm text-secondary list-disc">
                {item}
              </li>
            ))}
          </ul>
        </div>
      ))}

      {/* Empty state within a digest */}
      {digest.sections.length === 0 && (
        <p className="text-sm text-tertiary text-center py-4">
          Quiet week! No social activity to report.
        </p>
      )}

      {/* Footer */}
      <div className="text-xs text-tertiary text-right">
        Generated {generated.toLocaleString()}
      </div>
    </div>
  )
}

function DigestHistory({ digests, currentId }) {
  const [expanded, setExpanded] = useState(false)

  const past = digests.filter((d) => d.digest_id !== currentId)
  if (past.length === 0) return null

  return (
    <div className="space-y-3">
      <button
        type="button"
        onClick={() => setExpanded(!expanded)}
        className="text-sm text-secondary hover:text-primary transition-colors flex items-center gap-1"
      >
        <span className={`transition-transform ${expanded ? 'rotate-90' : ''}`}>
          &#9654;
        </span>
        Past digests ({past.length})
      </button>
      {expanded && (
        <div className="space-y-4">
          {past.map((d) => (
            <DigestCard key={d.digest_id} digest={d} />
          ))}
        </div>
      )}
    </div>
  )
}

export default function WeeklyDigestTab({ onResult }) {
  const [digest, setDigest] = useState(null)
  const [allDigests, setAllDigests] = useState([])
  const [loading, setLoading] = useState(true)
  const [generating, setGenerating] = useState(false)

  const loadDigest = useCallback(async () => {
    try {
      const [latestRes, historyRes] = await Promise.all([
        discoveryClient.getLatestDigest(),
        discoveryClient.listDigests(),
      ])
      if (latestRes.success && latestRes.data?.digest) {
        setDigest(latestRes.data.digest)
      }
      if (historyRes.success && historyRes.data?.digests) {
        setAllDigests(historyRes.data.digests)
      }
    } catch (e) {
      onResult?.({ error: toErrorMessage(e) || 'Failed to load digest' })
    } finally {
      setLoading(false)
    }
  }, [onResult])

  useEffect(() => {
    loadDigest()
  }, [loadDigest])

  const handleGenerate = async () => {
    setGenerating(true)
    try {
      const res = await discoveryClient.generateDigest()
      if (res.success && res.data?.digest) {
        setDigest(res.data.digest)
        // Refresh history
        const historyRes = await discoveryClient.listDigests()
        if (historyRes.success && historyRes.data?.digests) {
          setAllDigests(historyRes.data.digests)
        }
        onResult?.({
          success: true,
          data: {
            message: `Generated digest ${res.data.digest.digest_id} with ${res.data.digest.sections.length} sections`,
          },
        })
      } else {
        onResult?.({ error: res.error || 'Failed to generate digest' })
      }
    } catch (e) {
      onResult?.({ error: toErrorMessage(e) || 'Network error' })
    } finally {
      setGenerating(false)
    }
  }

  if (loading) {
    return (
      <div className="flex items-center justify-center py-16">
        <div className="text-center">
          <div className="w-6 h-6 border-2 border-border border-t-primary rounded-full animate-spin mx-auto mb-4" />
          <p className="text-secondary text-sm">Loading weekly digest...</p>
        </div>
      </div>
    )
  }

  if (!digest) {
    return (
      <div className="space-y-6">
        <div className="card p-8 text-center space-y-4 rounded">
          <div className="text-4xl text-gruvbox-blue">&#128220;</div>
          <h3 className="text-lg text-primary font-semibold">
            No Weekly Digest Yet
          </h3>
          <p className="text-secondary text-sm max-w-md mx-auto">
            Your weekly digest summarizes social activity: new discovery matches,
            connection updates, shared moments, and interest changes.
          </p>
          <p className="text-tertiary text-xs max-w-md mx-auto">
            Digests are generated automatically each week. You can also generate
            one now to see your current activity summary.
          </p>
          <button
            onClick={handleGenerate}
            disabled={generating}
            className="btn-primary"
          >
            {generating ? 'Generating...' : 'Generate Digest Now'}
          </button>
        </div>
      </div>
    )
  }

  return (
    <div className="space-y-6">
      {/* Header with refresh */}
      <div className="flex items-center justify-between">
        <div className="text-xs text-tertiary">
          Auto-generated weekly
        </div>
        <button
          onClick={handleGenerate}
          disabled={generating}
          className="btn-secondary btn-sm"
        >
          {generating ? 'Generating...' : 'Refresh'}
        </button>
      </div>

      {/* Current digest */}
      <DigestCard digest={digest} />

      {/* Past digests */}
      <DigestHistory digests={allDigests} currentId={digest.digest_id} />

      {/* Privacy note */}
      <div className="card-info p-3 rounded text-xs space-y-1">
        <div className="font-semibold text-gruvbox-blue">
          Your digest is local
        </div>
        <p className="text-secondary">
          This digest is generated locally from your data and never leaves your
          device. It summarizes activity from your discovery network connections.
        </p>
      </div>
    </div>
  )
}
