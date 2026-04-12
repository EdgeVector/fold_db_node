import { useCallback, useEffect, useState } from 'react'
import { useApprovedSchemas } from '../../hooks/useApprovedSchemas.js'
import { discoveryClient } from '../../api/clients/discoveryClient'
import { toErrorMessage } from '../../utils/schemaUtils'
import SearchPanel from './discovery/SearchPanel'
import FaceSearchPanel from './discovery/FaceSearchPanel'
import ConnectionRequestsPanel from './discovery/ConnectionRequestsPanel'
import SentRequestsPanel from './discovery/SentRequestsPanel'
import PeopleLikeYouPanel from './discovery/PeopleLikeYouPanel'
import InterestsPanel from './discovery/InterestsPanel'
import SharedEventsPanel from './discovery/SharedEventsPanel'
import ManageInterestsPanel from './discovery/ManageInterestsPanel'
import DiscoverySectionNav from './discovery/DiscoverySectionNav'
import { groupByCategory } from './discovery/discoveryUtils'

export default function DiscoveryTab({ onResult }) {
  const { approvedSchemas } = useApprovedSchemas()
  const [configs, setConfigs] = useState([])
  const [publishing, setPublishing] = useState(false)
  const [activeSection, setActiveSection] = useState('people')
  const [error, setError] = useState(null)
  const [serviceAvailable, setServiceAvailable] = useState(true)
  const [expandedCategories, setExpandedCategories] = useState(new Set())
  const [toggling, setToggling] = useState(false)
  const [lastPublishResult, setLastPublishResult] = useState(null)
  const [publishFacesCategories, setPublishFacesCategories] = useState(new Set())

  const optedInNames = new Set(configs.map(c => c.schema_name))

  // Categories that have been published (have at least one opted-in schema)
  // We track this via lastPublishResult — if publish was called, those categories are live
  const publishedCategories = new Set(
    lastPublishResult
      ? configs.filter(c => optedInNames.has(c.schema_name)).map(c => c.category)
      : []
  )

  const categoryGroups = groupByCategory(approvedSchemas || [])
  const categoryNames = Object.keys(categoryGroups).sort()
  const hasSchemas = (approvedSchemas || []).length > 0

  const loadConfigs = useCallback(async () => {
    try {
      const res = await discoveryClient.listOptIns()
      if (res.success) {
        setConfigs(res.data?.configs || [])
        setServiceAvailable(true)
        // If there are existing opt-ins, they may have been published before
        if ((res.data?.configs || []).length > 0) {
          setLastPublishResult({ existing: true })
        }
      } else if (res.status === 503) {
        setServiceAvailable(false)
      }
    } catch {
      setServiceAvailable(false)
    }
  }, [])

  useEffect(() => { loadConfigs() }, [loadConfigs])

  const handlePublishFacesToggle = async (category, enabled) => {
    setPublishFacesCategories(prev => {
      const next = new Set(prev)
      if (enabled) next.add(category)
      else next.delete(category)
      return next
    })
    // If any schemas in this category are already opted in, re-opt them in with the new
    // publish_faces value. Otherwise the checkbox only affects future opt-ins (was a bug
    // where existing opt-ins silently ignored the checkbox change).
    const schemas = categoryGroups[category] || []
    const alreadyOptedIn = schemas.filter(s => optedInNames.has(s.name))
    if (alreadyOptedIn.length === 0) return
    setToggling(true)
    setError(null)
    try {
      for (const s of alreadyOptedIn) {
        const res = await discoveryClient.optIn({
          schema_name: s.name,
          category,
          include_preview: false,
          publish_faces: enabled,
        })
        if (res.success) {
          setConfigs(res.data?.configs || [])
        } else {
          setError(res.error)
          break
        }
      }
    } catch (e) {
      setError(toErrorMessage(e))
    } finally {
      setToggling(false)
    }
  }

  const handleToggleCategory = async (category, schemas, enable) => {
    setToggling(true)
    setError(null)
    try {
      if (enable) {
        // Opt in all schemas in this category
        for (const s of schemas) {
          if (!optedInNames.has(s.name)) {
            const res = await discoveryClient.optIn({
              schema_name: s.name,
              category,
              include_preview: false,
              publish_faces: publishFacesCategories.has(category),
            })
            if (res.success) {
              setConfigs(res.data?.configs || [])
            } else {
              setError(res.error)
              break
            }
          }
        }
      } else {
        // Opt out all schemas in this category
        for (const s of schemas) {
          if (optedInNames.has(s.name)) {
            const res = await discoveryClient.optOut(s.name)
            if (res.success) {
              setConfigs(res.data?.configs || [])
            } else {
              setError(res.error)
              break
            }
          }
        }
      }
    } catch (e) {
      setError(toErrorMessage(e))
    } finally {
      setToggling(false)
    }
  }

  const handleBulkAction = async (action) => {
    setToggling(true)
    setError(null)
    try {
      if (action === 'publish-all') {
        for (const [cat, schemas] of Object.entries(categoryGroups)) {
          for (const s of schemas) {
            if (!optedInNames.has(s.name)) {
              const res = await discoveryClient.optIn({
                schema_name: s.name,
                category: cat,
                include_preview: false,
                publish_faces: publishFacesCategories.has(cat),
              })
              if (res.success) setConfigs(res.data?.configs || [])
            }
          }
        }
      } else if (action === 'unpublish-all') {
        for (const c of configs) {
          const res = await discoveryClient.optOut(c.schema_name)
          if (res.success) setConfigs(res.data?.configs || [])
        }
        setLastPublishResult(null)
      }
    } catch (e) {
      setError(toErrorMessage(e))
    } finally {
      setToggling(false)
    }
  }

  const handlePublish = async () => {
    setPublishing(true)
    setError(null)
    try {
      const res = await discoveryClient.publish()
      if (res.success) {
        setLastPublishResult(res.data)
        onResult({
          success: true,
          data: {
            message: `Published: ${res.data?.accepted} accepted, ${res.data?.quarantined} quarantined, ${res.data?.skipped} skipped`,
            ...res.data,
          },
        })
      } else {
        setError(res.error)
        onResult({ error: res.error })
      }
    } catch (e) {
      const msg = toErrorMessage(e)
      setError(msg)
      onResult({ error: msg })
    } finally {
      setPublishing(false)
    }
  }

  const toggleExpand = (cat) => {
    setExpandedCategories(prev => {
      const next = new Set(prev)
      if (next.has(cat)) next.delete(cat)
      else next.add(cat)
      return next
    })
  }

  if (!serviceAvailable) {
    return (
      <div className="space-y-4">
        <div className="card p-6 text-center rounded">
          <h3 className="text-lg text-primary mb-2">Discovery Not Available</h3>
          <p className="text-secondary text-sm">
            Discovery requires an Exemem cloud account. Enable cloud backup in
            Settings to join the discovery network and find users with similar data.
          </p>
        </div>
      </div>
    )
  }

  return (
    <div className="space-y-4">
      <DiscoverySectionNav activeSection={activeSection} onChange={setActiveSection} />

      {error && <div className="text-sm text-gruvbox-red">{error}</div>}

      {/* People Like You Section */}
      {activeSection === 'people' && (
        <PeopleLikeYouPanel onResult={onResult} />
      )}

      {/* Shared Events Section */}
      {activeSection === 'shared-events' && (
        <SharedEventsPanel onResult={onResult} />
      )}

      {/* Interests Section */}
      {activeSection === 'interests' && (
        <InterestsPanel onResult={onResult} />
      )}

      {/* Manage Section — Category Cards */}
      {activeSection === 'manage' && (
        <ManageInterestsPanel
          hasSchemas={hasSchemas}
          configs={configs}
          approvedSchemas={approvedSchemas || []}
          categoryGroups={categoryGroups}
          categoryNames={categoryNames}
          optedInNames={optedInNames}
          publishedCategories={publishedCategories}
          expandedCategories={expandedCategories}
          publishFacesCategories={publishFacesCategories}
          toggling={toggling}
          publishing={publishing}
          lastPublishResult={lastPublishResult}
          onToggleCategory={handleToggleCategory}
          onBulkAction={handleBulkAction}
          onPublish={handlePublish}
          onExpandToggle={toggleExpand}
          onPublishFacesToggle={handlePublishFacesToggle}
        />
      )}

      {/* Search Section */}
      {activeSection === 'search' && (
        <SearchPanel onResult={onResult} />
      )}

      {/* Face Search Section */}
      {activeSection === 'face-search' && (
        <FaceSearchPanel onResult={onResult} />
      )}

      {/* Received Connection Requests */}
      {activeSection === 'requests' && (
        <ConnectionRequestsPanel onResult={onResult} />
      )}

      {/* Sent Connection Requests */}
      {activeSection === 'sent' && (
        <SentRequestsPanel />
      )}
    </div>
  )
}
