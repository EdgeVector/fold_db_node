import { useState } from 'react'
import TrustTab from './TrustTab'
import SharingTab from './SharingTab'
import FeedTab from './FeedTab'
import SharedMomentsTab from './SharedMomentsTab'
import PersonasPanel from './personas/PersonasPanel'
import IngestionErrorsPanel from './personas/IngestionErrorsPanel'
import SuggestedPersonasPanel from './personas/SuggestedPersonasPanel'
import MyIdentityCardPanel from './personas/MyIdentityCardPanel'

const SUB_TABS = [
  { id: 'personas', label: 'Personas' },
  { id: 'suggestions', label: 'Suggestions' },
  { id: 'my-card', label: 'My Card' },
  { id: 'failed', label: 'Failed' },
  { id: 'contacts', label: 'Contacts' },
  { id: 'feed', label: 'Feed' },
  { id: 'sharing', label: 'Sharing' },
  { id: 'shared', label: 'Shared Moments' },
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
          >
            {tab.label}
          </button>
        ))}
      </div>
      {renderContent()}
    </div>
  )
}
