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
    <div>
      <div className="border-b border-border mb-4 flex flex-wrap items-end gap-x-6 gap-y-2">
        {SUB_TAB_GROUPS.map((group, idx) => (
          <div
            key={group.label}
            className={`flex flex-col ${idx > 0 ? 'sm:border-l sm:border-border sm:pl-6' : ''}`}
          >
            <div className="text-[10px] uppercase tracking-widest text-tertiary px-2 pb-1">
              {group.label}
            </div>
            <div className="flex flex-wrap">
              {group.tabs.map(tab => (
                <button
                  key={tab.id}
                  onClick={() => setActiveSubTab(tab.id)}
                  className={`tab ${activeSubTab === tab.id ? 'tab-active' : ''}`}
                >
                  {tab.label}
                </button>
              ))}
            </div>
          </div>
        ))}
      </div>
      {renderContent()}
    </div>
  )
}
