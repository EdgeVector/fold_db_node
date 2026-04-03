import { useState, useCallback, useEffect } from 'react'
import { discoveryClient } from '../../api/clients/discoveryClient'
import { toErrorMessage } from '../../utils/schemaUtils'

/**
 * SVG-based radar/spider chart for interest fingerprint visualization.
 * Renders categories as axes radiating from center, with filled polygon
 * showing relative weights.
 */
function RadarChart({ categories, size = 360 }) {
  const maxCount = Math.max(...categories.map(c => c.count), 1)
  const cx = size / 2
  const cy = size / 2
  const radius = size / 2 - 40 // leave room for labels
  const n = categories.length

  // Compute polygon points for the data shape
  const dataPoints = categories.map((cat, i) => {
    const angle = (2 * Math.PI * i) / n - Math.PI / 2
    const r = (cat.count / maxCount) * radius
    return {
      x: cx + r * Math.cos(angle),
      y: cy + r * Math.sin(angle),
    }
  })

  // Compute label positions (slightly beyond the outer ring)
  const labelPoints = categories.map((cat, i) => {
    const angle = (2 * Math.PI * i) / n - Math.PI / 2
    const r = radius + 24
    return {
      x: cx + r * Math.cos(angle),
      y: cy + r * Math.sin(angle),
      angle,
      name: cat.name,
      count: cat.count,
    }
  })

  // Grid rings (25%, 50%, 75%, 100%)
  const rings = [0.25, 0.5, 0.75, 1.0]

  // Polygon path
  const polygonPath = dataPoints.map((p, i) =>
    `${i === 0 ? 'M' : 'L'}${p.x},${p.y}`
  ).join(' ') + ' Z'

  // Grid polygon paths
  const gridPaths = rings.map(frac => {
    const pts = categories.map((_, i) => {
      const angle = (2 * Math.PI * i) / n - Math.PI / 2
      const r = frac * radius
      return { x: cx + r * Math.cos(angle), y: cy + r * Math.sin(angle) }
    })
    return pts.map((p, i) => `${i === 0 ? 'M' : 'L'}${p.x},${p.y}`).join(' ') + ' Z'
  })

  // Axis lines
  const axisLines = categories.map((_, i) => {
    const angle = (2 * Math.PI * i) / n - Math.PI / 2
    return {
      x2: cx + radius * Math.cos(angle),
      y2: cy + radius * Math.sin(angle),
    }
  })

  // Text anchor based on position
  function textAnchor(angle) {
    const deg = (angle * 180) / Math.PI
    if (deg > -100 && deg < -80) return 'middle'
    if (deg > 80 && deg < 100) return 'middle'
    if (deg >= -80 && deg <= 80) return 'start'
    return 'end'
  }

  function dy(angle) {
    const deg = (angle * 180) / Math.PI
    if (deg > -100 && deg < -80) return '-0.3em'
    if (deg > 80 && deg < 100) return '1em'
    return '0.35em'
  }

  return (
    <svg
      viewBox={`0 0 ${size} ${size}`}
      className="w-full max-w-md mx-auto"
      style={{ aspectRatio: '1 / 1' }}
    >
      {/* Grid rings */}
      {gridPaths.map((d, i) => (
        <path
          key={`ring-${i}`}
          d={d}
          fill="none"
          stroke="currentColor"
          strokeWidth="0.5"
          className="text-border"
          opacity={0.4}
        />
      ))}

      {/* Axis lines */}
      {axisLines.map((line, i) => (
        <line
          key={`axis-${i}`}
          x1={cx}
          y1={cy}
          x2={line.x2}
          y2={line.y2}
          stroke="currentColor"
          strokeWidth="0.5"
          className="text-border"
          opacity={0.3}
        />
      ))}

      {/* Data polygon */}
      <path
        d={polygonPath}
        fill="rgba(69, 133, 136, 0.25)"
        stroke="rgb(69, 133, 136)"
        strokeWidth="2"
      />

      {/* Data points */}
      {dataPoints.map((p, i) => (
        <circle
          key={`dot-${i}`}
          cx={p.x}
          cy={p.y}
          r="4"
          fill="rgb(69, 133, 136)"
          stroke="rgb(40, 40, 40)"
          strokeWidth="1"
        />
      ))}

      {/* Labels */}
      {labelPoints.map((lp, i) => (
        <text
          key={`label-${i}`}
          x={lp.x}
          y={lp.y}
          textAnchor={textAnchor(lp.angle)}
          dy={dy(lp.angle)}
          className="fill-secondary"
          fontSize="10"
          fontFamily="inherit"
        >
          {lp.name} ({lp.count})
        </text>
      ))}
    </svg>
  )
}

/**
 * Tag cloud visualization as a fallback for fewer than 3 categories.
 * Tags are sized proportionally to their count.
 */
function TagCloud({ categories }) {
  const maxCount = Math.max(...categories.map(c => c.count), 1)

  return (
    <div className="flex flex-wrap gap-3 justify-center py-6">
      {categories.map(cat => {
        const weight = cat.count / maxCount
        const fontSize = 0.75 + weight * 0.75 // 0.75rem to 1.5rem
        const opacity = 0.5 + weight * 0.5
        return (
          <span
            key={cat.name}
            className={`inline-block px-3 py-1.5 rounded border transition-colors ${
              cat.enabled
                ? 'border-gruvbox-blue bg-gruvbox-blue/10 text-primary'
                : 'border-border bg-transparent text-tertiary'
            }`}
            style={{ fontSize: `${fontSize}rem`, opacity }}
          >
            {cat.name}
            <span className="ml-1.5 text-xs text-secondary">{cat.count}</span>
          </span>
        )
      })}
    </div>
  )
}

/**
 * Stats summary bar showing key profile metrics.
 */
function ProfileStats({ profile }) {
  const totalCategorized = profile.categories.reduce((sum, c) => sum + c.count, 0)
  const coveragePercent = profile.total_embeddings_scanned > 0
    ? Math.round((totalCategorized / profile.total_embeddings_scanned) * 100)
    : 0

  return (
    <div className="grid grid-cols-3 gap-4">
      <div className="card rounded p-4 text-center">
        <div className="text-2xl font-bold text-gruvbox-blue">
          {profile.categories.length}
        </div>
        <div className="text-xs text-secondary mt-1">Interests Detected</div>
      </div>
      <div className="card rounded p-4 text-center">
        <div className="text-2xl font-bold text-gruvbox-green">
          {profile.total_embeddings_scanned.toLocaleString()}
        </div>
        <div className="text-xs text-secondary mt-1">Data Points Scanned</div>
      </div>
      <div className="card rounded p-4 text-center">
        <div className="text-2xl font-bold text-gruvbox-yellow">
          {coveragePercent}%
        </div>
        <div className="text-xs text-secondary mt-1">Coverage</div>
      </div>
    </div>
  )
}

/**
 * Category list with toggle controls and similarity scores.
 */
function CategoryList({ categories, onToggle, toggling }) {
  return (
    <div className="space-y-2">
      {categories.map(cat => {
        const similarityPercent = (cat.avg_similarity * 100).toFixed(1)
        return (
          <div
            key={cat.name}
            className={`flex items-center justify-between p-3 rounded border transition-colors ${
              cat.enabled
                ? 'border-border bg-surface'
                : 'border-border/50 bg-surface opacity-60'
            }`}
          >
            <div className="flex items-center gap-3">
              <button
                type="button"
                role="switch"
                aria-checked={cat.enabled}
                disabled={toggling}
                onClick={() => onToggle(cat.name, !cat.enabled)}
                className={`relative inline-flex h-5 w-9 items-center rounded-full transition-colors ${
                  toggling ? 'opacity-50 cursor-not-allowed' : 'cursor-pointer'
                } ${cat.enabled ? 'bg-gruvbox-green' : 'bg-gruvbox-elevated border border-border'}`}
              >
                <span
                  className={`inline-block h-3.5 w-3.5 rounded-full bg-primary transition-transform ${
                    cat.enabled ? 'translate-x-[18px]' : 'translate-x-[3px]'
                  }`}
                />
              </button>
              <div>
                <span className="text-sm font-medium text-primary">{cat.name}</span>
                <div className="text-xs text-tertiary">
                  {cat.count} items &middot; {similarityPercent}% avg match
                </div>
              </div>
            </div>
            <div className="flex items-center gap-2">
              {/* Mini bar showing relative weight */}
              <div className="w-20 h-1.5 bg-surface-secondary rounded-full overflow-hidden">
                <div
                  className="h-full bg-gruvbox-blue rounded-full"
                  style={{ width: `${Math.min(cat.avg_similarity * 200, 100)}%` }}
                />
              </div>
            </div>
          </div>
        )
      })}
    </div>
  )
}

export default function MyProfileTab({ onResult }) {
  const [profile, setProfile] = useState(null)
  const [loading, setLoading] = useState(true)
  const [detecting, setDetecting] = useState(false)
  const [toggling, setToggling] = useState(false)

  const loadProfile = useCallback(async () => {
    try {
      const res = await discoveryClient.getInterests()
      if (res.success) {
        setProfile(res.data)
      }
    } catch (e) {
      onResult?.({ error: toErrorMessage(e) || 'Failed to load profile' })
    } finally {
      setLoading(false)
    }
  }, [onResult])

  useEffect(() => { loadProfile() }, [loadProfile])

  const handleDetect = async () => {
    setDetecting(true)
    try {
      const res = await discoveryClient.detectInterests()
      if (res.success) {
        onResult?.({
          success: true,
          data: { message: `Detected ${res.data?.categories?.length || 0} interest categories` },
        })
        await loadProfile()
      } else {
        onResult?.({ error: res.error || 'Detection failed' })
      }
    } catch (e) {
      onResult?.({ error: toErrorMessage(e) || 'Network error' })
    } finally {
      setDetecting(false)
    }
  }

  const handleToggle = async (categoryName, enabled) => {
    setToggling(true)
    try {
      const res = await discoveryClient.toggleInterest(categoryName, enabled)
      if (res.success) {
        setProfile(res.data)
      } else {
        onResult?.({ error: res.error || 'Toggle failed' })
      }
    } catch (e) {
      onResult?.({ error: toErrorMessage(e) || 'Network error' })
    } finally {
      setToggling(false)
    }
  }

  if (loading) {
    return (
      <div className="flex items-center justify-center py-16">
        <div className="text-center">
          <div className="w-6 h-6 border-2 border-border border-t-primary rounded-full animate-spin mx-auto mb-4" />
          <p className="text-secondary text-sm">Loading your interest fingerprint...</p>
        </div>
      </div>
    )
  }

  const categories = profile?.categories || []
  const enabledCategories = categories.filter(c => c.enabled)
  const hasProfile = profile && profile.seed_version > 0 && categories.length > 0

  if (!hasProfile) {
    return (
      <div className="space-y-6">
        <div className="card p-8 text-center space-y-4 rounded">
          <div className="text-4xl text-gruvbox-blue">&#9673;</div>
          <h3 className="text-lg text-primary font-semibold">
            No Interest Fingerprint Yet
          </h3>
          <p className="text-secondary text-sm max-w-md mx-auto">
            Your interest fingerprint is auto-generated from the data you ingest.
            It shows what topics you care about, weighted by how much data you have in each category.
          </p>
          <p className="text-tertiary text-xs max-w-md mx-auto">
            Ingest some data first, then click below to generate your fingerprint.
            It will update automatically as you add more data.
          </p>
          <div className="flex items-center gap-3 justify-center">
            <button
              onClick={handleDetect}
              disabled={detecting}
              className="btn-primary"
            >
              {detecting ? 'Scanning your data...' : 'Generate Fingerprint'}
            </button>
            <button
              onClick={() => { window.location.hash = 'smart-folder' }}
              className="btn-secondary text-sm"
            >
              Import data
            </button>
          </div>
        </div>
      </div>
    )
  }

  return (
    <div className="space-y-6">
      {/* Header with detected time and re-detect */}
      <div className="flex items-center justify-between">
        <div className="text-xs text-tertiary">
          Last detected: {new Date(profile.detected_at).toLocaleString()}
        </div>
        <button
          onClick={handleDetect}
          disabled={detecting}
          className="btn-secondary btn-sm"
        >
          {detecting ? 'Scanning...' : 'Re-scan'}
        </button>
      </div>

      {/* Stats summary */}
      <ProfileStats profile={profile} />

      {/* Fingerprint visualization */}
      <div className="card rounded p-6">
        <h3 className="text-sm font-semibold text-primary mb-4">
          Interest Fingerprint
        </h3>
        {enabledCategories.length >= 3 ? (
          <RadarChart categories={enabledCategories} />
        ) : (
          <TagCloud categories={categories} />
        )}
      </div>

      {/* Category list with toggles */}
      <div>
        <h3 className="text-sm font-semibold text-primary mb-3">
          Categories ({categories.length})
        </h3>
        <p className="text-xs text-tertiary mb-3">
          Toggle categories to control which interests appear in your fingerprint
          and are visible on the discovery network.
        </p>
        <CategoryList
          categories={categories}
          onToggle={handleToggle}
          toggling={toggling}
        />
      </div>

      {/* Privacy note */}
      <div className="card-info p-3 rounded text-xs space-y-1">
        <div className="font-semibold text-gruvbox-blue">Your fingerprint is private by default</div>
        <p className="text-secondary">
          This fingerprint is computed locally from your data. It is only shared on the discovery
          network if you explicitly opt in via the Discovery tab. You control which categories
          are visible.
        </p>
      </div>
    </div>
  )
}
