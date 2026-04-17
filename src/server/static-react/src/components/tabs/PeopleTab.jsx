import { useEffect, useState } from 'react'
import TrustTab from './TrustTab'
import SharingTab from './SharingTab'
import FeedTab from './FeedTab'
import SharedMomentsTab from './SharedMomentsTab'
import PersonasPanel from './personas/PersonasPanel'
import IngestionErrorsPanel from './personas/IngestionErrorsPanel'
import SuggestedPersonasPanel from './personas/SuggestedPersonasPanel'
import { getSuggestionCount } from '../../api/clients/fingerprintsClient'

const SUB_TABS = [
  { id: 'personas', label: 'Personas' },
  { id: 'suggestions', label: 'Suggestions' },
  { id: 'failed', label: 'Failed' },
  { id: 'contacts', label: 'Contacts' },
  { id: 'feed', label: 'Feed' },
  { id: 'sharing', label: 'Sharing' },
  { id: 'shared', label: 'Shared Moments' },
]

/** How often to poll the suggestion count in ms. 30s matches the
 *  existing Header notification cadence — long enough that the
 *  backend load is negligible, short enough that newly-proposed
 *  clusters surface on the badge within half a minute of ingest. */
const SUGGESTION_COUNT_POLL_MS = 30_000

export default function PeopleTab({ onResult }) {
  const [activeSubTab, setActiveSubTab] = useState('personas')
  const [suggestionCount, setSuggestionCount] = useState(0)

  // Poll the count endpoint. The endpoint is cheap (a single atomic
  // read on the backend) so polling is fine — we don't need
  // websockets or long-polling at Phase 2 scale. On tab switch to
  // Suggestions we refetch immediately so the badge drops to zero
  // the moment the user has seen the list.
  useEffect(() => {
    let cancelled = false
    const fetchCount = async () => {
      try {
        const res = await getSuggestionCount()
        if (!cancelled && res.success) {
          setSuggestionCount(res.data?.count ?? 0)
        }
      } catch {
        // Silent — the count is a UX affordance, not a correctness
        // requirement. A network blip should not spam the console.
      }
    }
    fetchCount()
    const id = setInterval(fetchCount, SUGGESTION_COUNT_POLL_MS)
    return () => {
      cancelled = true
      clearInterval(id)
    }
  }, [activeSubTab]) // refetch on tab switch

  const renderContent = () => {
    switch (activeSubTab) {
      case 'personas':
        return <PersonasPanel />
      case 'suggestions':
        return <SuggestedPersonasPanel />
      case 'failed':
        return <IngestionErrorsPanel />
      case 'contacts':
        return <TrustTab onResult={onResult} />
      case 'feed':
        return <FeedTab />
      case 'sharing':
        return <SharingTab onResult={onResult} />
      case 'shared':
        return <SharedMomentsTab onResult={onResult} />
      default:
        return null
    }
  }

  return (
    <div>
      <div className="flex border-b border-border mb-4">
        {SUB_TABS.map(tab => (
          <button
            key={tab.id}
            onClick={() => setActiveSubTab(tab.id)}
            className={`tab ${activeSubTab === tab.id ? 'tab-active' : ''}`}
            data-testid={`people-subtab-${tab.id}`}
          >
            {tab.label}
            {tab.id === 'suggestions' && suggestionCount > 0 && (
              <span
                className="ml-1.5 inline-flex items-center justify-center min-w-[18px] h-[18px] px-1 text-[10px] font-bold bg-gruvbox-red text-white rounded-full align-middle"
                data-testid="people-subtab-suggestions-badge"
              >
                {suggestionCount > 99 ? '99+' : suggestionCount}
              </span>
            )}
          </button>
        ))}
      </div>
      {renderContent()}
    </div>
  )
}
