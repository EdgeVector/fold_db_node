import { useState, useEffect, useCallback } from 'react'
import { CameraIcon, MapPinIcon } from '@heroicons/react/24/outline'
import { discoveryClient } from '../../api/clients/discoveryClient'

function formatTimeBucket(bucket) {
  // "2026-03-15T14" → "March 15, 2026 at 2:00 PM"
  const [datePart, hourStr] = bucket.split('T')
  const hour = parseInt(hourStr, 10)
  const date = new Date(datePart + 'T00:00:00')
  const monthNames = ['January','February','March','April','May','June',
    'July','August','September','October','November','December']
  const ampm = hour >= 12 ? 'PM' : 'AM'
  const displayHour = hour % 12 || 12
  return `${monthNames[date.getMonth()]} ${date.getDate()}, ${date.getFullYear()} around ${displayHour}:00 ${ampm}`
}

function relativeTime(dateStr) {
  const now = Date.now()
  const then = new Date(dateStr).getTime()
  const seconds = Math.floor((now - then) / 1000)
  if (seconds < 60) return 'just now'
  const minutes = Math.floor(seconds / 60)
  if (minutes < 60) return `${minutes}m ago`
  const hours = Math.floor(minutes / 60)
  if (hours < 24) return `${hours}h ago`
  const days = Math.floor(hours / 24)
  return `${days}d ago`
}

function MomentCard({ moment }) {
  const peerName = moment.peer_display_name || moment.peer_pseudonym.slice(0, 8) + '...'

  return (
    <div className="card rounded p-4 space-y-3">
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-3">
          <div className="w-10 h-10 rounded-full bg-gruvbox-blue/20 flex items-center justify-center text-gruvbox-blue">
            <CameraIcon aria-hidden="true" className="w-5 h-5" />
          </div>
          <div>
            <p className="text-sm font-medium text-primary">
              You and <span className="text-gruvbox-blue">{peerName}</span> were both nearby
            </p>
            <p className="text-xs text-secondary">
              {formatTimeBucket(moment.time_bucket)}
            </p>
          </div>
        </div>
        <span className="text-xs text-tertiary">{relativeTime(moment.detected_at)}</span>
      </div>

      {moment.location_name && (
        <div className="flex items-center gap-2 text-sm text-secondary">
          <MapPinIcon aria-hidden="true" className="w-4 h-4 shrink-0" />
          <span>{moment.location_name}</span>
        </div>
      )}

      <div className="flex items-center gap-2 text-xs text-tertiary">
        <span className="badge badge-info">Geohash: {moment.geohash}</span>
        {moment.our_timestamp && (
          <span>Your photo: {new Date(moment.our_timestamp).toLocaleString()}</span>
        )}
      </div>
    </div>
  )
}

function OptInCard({ optIn, onOptOut }) {
  const [removing, setRemoving] = useState(false)
  const peerName = optIn.peer_display_name || optIn.peer_pseudonym.slice(0, 8) + '...'

  const handleOptOut = async () => {
    setRemoving(true)
    try {
      await onOptOut(optIn.peer_pseudonym)
    } finally {
      setRemoving(false)
    }
  }

  return (
    <div className="flex items-center justify-between p-3 rounded bg-surface-secondary">
      <div>
        <span className="text-sm font-medium text-primary">{peerName}</span>
        <span className="text-xs text-tertiary ml-2">
          since {new Date(optIn.opted_in_at).toLocaleDateString()}
        </span>
      </div>
      <button
        onClick={handleOptOut}
        disabled={removing}
        className="btn btn-sm btn-secondary text-gruvbox-red"
      >
        {removing ? 'Removing...' : 'Remove'}
      </button>
    </div>
  )
}

function EmptyMomentsState() {
  return (
    <div className="text-center py-12 space-y-3">
      <div className="flex justify-center text-secondary">
        <CameraIcon aria-hidden="true" className="w-12 h-12" />
      </div>
      <h3 className="text-lg font-medium text-primary">No Moments Yet</h3>
      <p className="text-sm text-secondary max-w-md mx-auto">
        When you and a connected peer both opt in to photo moment sharing, photos
        taken at similar times and locations will appear here as shared moments.
      </p>
    </div>
  )
}

export default function SharedMomentsTab({ onResult }) {
  const [moments, setMoments] = useState([])
  const [optIns, setOptIns] = useState([])
  const [loading, setLoading] = useState(true)
  const [detecting, setDetecting] = useState(false)
  const [showAddPeer, setShowAddPeer] = useState(false)
  const [newPeerPseudonym, setNewPeerPseudonym] = useState('')
  const [newPeerName, setNewPeerName] = useState('')
  const [addingPeer, setAddingPeer] = useState(false)

  const loadData = useCallback(async () => {
    setLoading(true)
    try {
      const [momentsRes, optInsRes] = await Promise.all([
        discoveryClient.listSharedMoments(),
        discoveryClient.listMomentOptIns(),
      ])

      if (momentsRes.success && momentsRes.data) {
        setMoments(momentsRes.data.moments || [])
      }
      if (optInsRes.success && optInsRes.data) {
        setOptIns(optInsRes.data.opt_ins || [])
      }
    } catch (err) {
      onResult?.({ error: err.message || 'Failed to load shared moments' })
    } finally {
      setLoading(false)
    }
  }, [onResult])

  useEffect(() => { loadData() }, [loadData])

  const handleOptIn = async () => {
    if (!newPeerPseudonym.trim()) return
    setAddingPeer(true)
    try {
      const res = await discoveryClient.momentOptIn(
        newPeerPseudonym.trim(),
        newPeerName.trim() || undefined,
      )
      if (res.success && res.data) {
        setOptIns(res.data.opt_ins || [])
        setNewPeerPseudonym('')
        setNewPeerName('')
        setShowAddPeer(false)
        onResult?.({ success: true })
      } else {
        onResult?.({ error: res.error || 'Failed to opt in' })
      }
    } catch (err) {
      onResult?.({ error: err.message || 'Failed to opt in' })
    } finally {
      setAddingPeer(false)
    }
  }

  const handleOptOut = async (peerPseudonym) => {
    try {
      const res = await discoveryClient.momentOptOut(peerPseudonym)
      if (res.success && res.data) {
        setOptIns(res.data.opt_ins || [])
        onResult?.({ success: true })
      } else {
        onResult?.({ error: res.error || 'Failed to opt out' })
      }
    } catch (err) {
      onResult?.({ error: err.message || 'Failed to opt out' })
    }
  }

  const handleDetect = async () => {
    setDetecting(true)
    try {
      const res = await discoveryClient.momentDetect()
      if (res.success && res.data) {
        if (res.data.new_moments_found > 0) {
          onResult?.({ success: true, message: `Found ${res.data.new_moments_found} new shared moment(s)!` })
        } else {
          onResult?.({ success: true, message: 'No new shared moments detected.' })
        }
        await loadData()
      } else {
        onResult?.({ error: res.error || 'Detection failed' })
      }
    } catch (err) {
      onResult?.({ error: err.message || 'Detection failed' })
    } finally {
      setDetecting(false)
    }
  }

  if (loading) {
    return (
      <div className="flex items-center justify-center py-12">
        <div className="w-6 h-6 border-2 border-border border-t-primary rounded-full animate-spin" />
      </div>
    )
  }

  return (
    <div className="max-w-3xl mx-auto space-y-6">
      {/* Header */}
      <div className="flex items-center justify-between">
        <div>
          <h2 className="text-lg font-semibold text-primary">Moments</h2>
          <p className="text-sm text-secondary">
            Photos taken at similar times and places with connected peers
          </p>
        </div>
        <div className="flex gap-2">
          <button
            onClick={handleDetect}
            disabled={detecting || optIns.length === 0}
            className="btn btn-sm btn-primary"
          >
            {detecting ? 'Detecting...' : 'Detect Moments'}
          </button>
        </div>
      </div>

      {/* Peer Opt-In Management */}
      <div className="card p-4 space-y-3">
        <div className="flex items-center justify-between">
          <h3 className="text-sm font-medium text-primary">
            Photo Sharing Peers ({optIns.length})
          </h3>
          <button
            onClick={() => setShowAddPeer(!showAddPeer)}
            className="btn btn-sm btn-secondary"
          >
            {showAddPeer ? 'Cancel' : '+ Add Peer'}
          </button>
        </div>

        <p className="text-xs text-tertiary">
          Both you and the peer must opt in. Only metadata hashes are compared — raw
          photo data never leaves your device.
        </p>

        {showAddPeer && (
          <div className="space-y-2 p-3 rounded bg-surface-secondary">
            <input
              type="text"
              value={newPeerPseudonym}
              onChange={(e) => setNewPeerPseudonym(e.target.value)}
              placeholder="Peer pseudonym (UUID)"
              className="input input-sm w-full"
            />
            <input
              type="text"
              value={newPeerName}
              onChange={(e) => setNewPeerName(e.target.value)}
              placeholder="Display name (optional)"
              className="input input-sm w-full"
            />
            <button
              onClick={handleOptIn}
              disabled={addingPeer || !newPeerPseudonym.trim()}
              className="btn btn-sm btn-primary"
            >
              {addingPeer ? 'Adding...' : 'Enable Moment Sharing'}
            </button>
          </div>
        )}

        {optIns.length > 0 ? (
          <div className="space-y-2">
            {optIns.map((optIn) => (
              <OptInCard
                key={optIn.peer_pseudonym}
                optIn={optIn}
                onOptOut={handleOptOut}
              />
            ))}
          </div>
        ) : (
          <p className="text-sm text-tertiary text-center py-3">
            No peers opted in yet. Add a connected peer to start detecting shared moments.
          </p>
        )}
      </div>

      {/* Shared Moments List */}
      {moments.length > 0 ? (
        <div className="space-y-3">
          {moments.map((moment) => (
            <MomentCard key={moment.moment_id} moment={moment} />
          ))}
        </div>
      ) : (
        <EmptyMomentsState />
      )}
    </div>
  )
}
