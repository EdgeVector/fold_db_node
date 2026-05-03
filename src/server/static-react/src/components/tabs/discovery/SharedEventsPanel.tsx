import { useCallback, useEffect, useState } from 'react'
import { discoveryClient } from '../../../api/clients/discoveryClient'
import { toErrorMessage } from '../../../utils/schemaUtils'
import LocalModeNotice from './LocalModeNotice'
import { isLocalModeError } from './discoveryUtils'

export default function SharedEventsPanel({ onResult }) {
  const [status, setStatus] = useState(null)
  const [sharedEvents, setSharedEvents] = useState([])
  const [loading, setLoading] = useState(true)
  const [toggling, setToggling] = useState(false)
  const [error, setError] = useState(null)

  const loadData = useCallback(async () => {
    try {
      const statusRes = await discoveryClient.getCalendarSharingStatus()
      if (statusRes.success) {
        setStatus(statusRes.data)
        setError(null)
        if (statusRes.data?.opted_in) {
          const eventsRes = await discoveryClient.getSharedEvents()
          if (eventsRes.success) {
            setSharedEvents(eventsRes.data?.shared_events || [])
          }
        }
      } else {
        setError(statusRes.error || 'Failed to load calendar sharing status')
      }
    } catch (e) {
      setError(toErrorMessage(e) || 'Network error')
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => { loadData() }, [loadData])

  const handleToggle = async (enable) => {
    setToggling(true)
    try {
      const res = enable
        ? await discoveryClient.calendarSharingOptIn()
        : await discoveryClient.calendarSharingOptOut()
      if (res.success) {
        setStatus(res.data)
        if (!enable) setSharedEvents([])
        onResult({
          success: true,
          data: { message: enable ? 'Calendar sharing enabled' : 'Calendar sharing disabled' },
        })
        if (enable) {
          const eventsRes = await discoveryClient.getSharedEvents()
          if (eventsRes.success) setSharedEvents(eventsRes.data?.shared_events || [])
        }
      } else {
        onResult({ error: res.error || 'Failed to toggle calendar sharing' })
      }
    } catch (e) {
      onResult({ error: toErrorMessage(e) || 'Network error' })
    } finally {
      setToggling(false)
    }
  }

  if (loading) return <p className="text-secondary text-sm">Loading calendar sharing...</p>

  if (error) {
    if (isLocalModeError(error)) return <LocalModeNotice />
    return (
      <div className="space-y-3">
        <div className="text-sm text-gruvbox-red">{error}</div>
        <button onClick={loadData} className="btn-secondary btn-sm">Retry</button>
      </div>
    )
  }

  const optedIn = status?.opted_in || false

  return (
    <div className="space-y-4">
      {/* Opt-in toggle */}
      <div className="card p-4 space-y-3">
        <div className="flex items-center justify-between">
          <div>
            <div className="text-sm font-medium text-primary">Share Calendar with Connections</div>
            <div className="text-xs text-secondary mt-1">
              Compare events with accepted connections to discover shared conferences, meetups, and events.
              Only overlap existence is revealed — never full calendar details.
            </div>
          </div>
          <button
            onClick={() => handleToggle(!optedIn)}
            disabled={toggling}
            className={`relative inline-flex h-5 w-9 items-center rounded-full transition-colors ${
              toggling ? 'opacity-50 cursor-not-allowed' : 'cursor-pointer'
            } ${optedIn ? 'bg-gruvbox-green' : 'bg-gruvbox-elevated border border-border'}`}
          >
            <span
              className={`inline-block h-3.5 w-3.5 rounded-full bg-primary transition-transform ${
                optedIn ? 'translate-x-[18px]' : 'translate-x-[3px]'
              }`}
            />
          </button>
        </div>

        {optedIn && status && (
          <div className="text-xs text-tertiary">
            {status.local_event_count} events synced &middot; {status.peer_count} peer{status.peer_count !== 1 ? 's' : ''} connected
          </div>
        )}
      </div>

      {/* Privacy notice */}
      {optedIn && (
        <div className="card-info p-3 rounded text-xs space-y-1.5">
          <div className="font-semibold text-gruvbox-blue">Privacy</div>
          <ul className="space-y-1 text-secondary">
            <li>Only event overlap existence is shared — never full calendar details</li>
            <li>Both you and your connection must opt in for comparison</li>
            <li>Events are compared by date, title, and location similarity</li>
            <li>You can opt out at any time to stop sharing</li>
          </ul>
        </div>
      )}

      {/* Shared events */}
      {optedIn && sharedEvents.length > 0 && (
        <div className="space-y-2">
          <div className="text-sm font-medium text-primary">Shared Events</div>
          {sharedEvents.map((evt, i) => (
            <div key={i} className="card p-3 space-y-1">
              <div className="flex items-center justify-between">
                <div className="text-sm font-medium text-primary">{evt.event_title}</div>
                <span className="badge badge-info text-xs">
                  {evt.connection_count} connection{evt.connection_count !== 1 ? 's' : ''}
                </span>
              </div>
              <div className="text-xs text-secondary">
                {evt.start_time} — {evt.end_time}
              </div>
              {evt.location && (
                <div className="text-xs text-tertiary">{evt.location}</div>
              )}
              <div className="text-xs text-gruvbox-green">
                You and {evt.connection_count} connection{evt.connection_count !== 1 ? 's' : ''} are attending this event
              </div>
            </div>
          ))}
        </div>
      )}

      {/* Empty state */}
      {optedIn && sharedEvents.length === 0 && (
        <div className="card p-6 text-center">
          <p className="text-secondary text-sm">
            No shared events detected yet. Sync your calendar and connect with peers who also have calendar sharing enabled.
          </p>
        </div>
      )}

      {/* Not opted in */}
      {!optedIn && (
        <div className="card p-6 text-center">
          <p className="text-secondary text-sm">
            Enable calendar sharing to discover events you have in common with your connections.
          </p>
        </div>
      )}
    </div>
  )
}
