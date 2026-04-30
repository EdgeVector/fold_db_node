import { useState } from 'react'
import TrustTab from './TrustTab'
import SharingTab from './SharingTab'
import FeedTab from './FeedTab'
import SharedMomentsTab from './SharedMomentsTab'
import PersonasPanel from './personas/PersonasPanel'
import IngestionErrorsPanel from './personas/IngestionErrorsPanel'
import SuggestedPersonasPanel from './personas/SuggestedPersonasPanel'
import MyIdentityCardPanel from './personas/MyIdentityCardPanel'
import CrossUserSharingPanel from './sharing/CrossUserSharingPanel'
import ImportedIdentitiesPanel from './personas/ImportedIdentitiesPanel'
import ReceivedCardsPanel from './personas/ReceivedCardsPanel'

const SUB_TAB_GROUPS = [
  {
    label: 'Contacts',
    tabs: [
      { id: 'personas', label: 'Personas' },
      { id: 'suggestions', label: 'Suggestions' },
    ],
  },
  {
    label: 'Identity',
    tabs: [
      { id: 'my-card', label: 'My Card' },
      { id: 'received', label: 'Received Cards' },
    ],
  },
  {
    label: 'Network',
    tabs: [
      { id: 'contacts', label: 'Trusted' },
      { id: 'feed', label: 'Feed' },
      { id: 'shared', label: 'Moments' },
    ],
  },
  {
    label: 'Sharing',
    tabs: [
      { id: 'sharing', label: 'Trust Grants' },
      { id: 'cross-user', label: 'Cross-User Rules' },
    ],
  },
  {
    label: 'Admin',
    tabs: [
      { id: 'identities', label: 'Identity Records' },
      { id: 'failed', label: 'Ingestion Errors' },
    ],
  },
]

export default function PeopleTab({ onResult }) {
  const [activeSubTab, setActiveSubTab] = useState('personas')

  const renderContent = () => {
    switch (activeSubTab) {
      case 'personas':
        return <PersonasPanel />
      case 'suggestions':
        return <SuggestedPersonasPanel />
      case 'my-card':
        return <MyIdentityCardPanel />
      case 'received':
        return <ReceivedCardsPanel />
      case 'identities':
        return <ImportedIdentitiesPanel />
      case 'failed':
        return <IngestionErrorsPanel />
      case 'contacts':
        return <TrustTab onResult={onResult} />
      case 'feed':
        return <FeedTab />
      case 'sharing':
        return <SharingTab onResult={onResult} />
      case 'cross-user':
        return <CrossUserSharingPanel />
      case 'shared':
        return <SharedMomentsTab onResult={onResult} />
      default:
        return null
    }
  }

  return (
    <div className="flex gap-6 -mt-1">
      {/* Left rail — same pattern as the main app sidebar (groups +
        * items, narrow column, border-l-2 active state). 11 destinations
        * stacked vertically reads as wayfinding rather than the previous
        * horizontal "control panel" layout that fanned 5 group columns
        * across the page header. Content area on the right gets full
        * width back. */}
      <nav
        className="shrink-0 w-44 border-r border-border pr-2 -ml-1"
        aria-label="People sub-sections"
      >
        {SUB_TAB_GROUPS.map((group) => (
          <div key={group.label}>
            <div className="text-[10px] uppercase tracking-widest text-tertiary px-3 pt-3 pb-0.5">
              {group.label}
            </div>
            {group.tabs.map((tab) => {
              const isActive = activeSubTab === tab.id
              return (
                <button
                  key={tab.id}
                  type="button"
                  onClick={() => setActiveSubTab(tab.id)}
                  aria-current={isActive ? 'page' : undefined}
                  className={`w-full text-left px-3 py-1.5 text-sm transition-colors border-l-2 bg-transparent cursor-pointer ${
                    isActive
                      ? 'bg-surface-secondary border-l-gruvbox-yellow text-primary'
                      : 'text-secondary hover:text-primary hover:bg-surface-secondary border-l-transparent'
                  }`}
                >
                  {tab.label}
                </button>
              )
            })}
          </div>
        ))}
      </nav>
      <div className="flex-1 min-w-0">
        {renderContent()}
      </div>
    </div>
  )
}
