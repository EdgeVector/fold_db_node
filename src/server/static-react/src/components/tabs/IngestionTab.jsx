import { useState, useEffect } from 'react'
import { ingestionClient } from '../../api/clients'
import { defaultApiClient } from '../../api/core/client'
import { generateBlogPosts } from '../../data/sampleBlogPosts'
import { twitterSamples, instagramSamples, linkedinSamples, tiktokSamples } from '../../data/sampleSocialPosts'

const SAMPLES_ENABLED = import.meta.env.VITE_ENABLE_SAMPLES === 'true'

function IngestionTab({ onResult }) {
  const [jsonData, setJsonData] = useState('')
  const [autoExecute, setAutoExecute] = useState(true)
  const [isLoading, setIsLoading] = useState(false)
  const [orgs, setOrgs] = useState([])
  const [selectedOrg, setSelectedOrg] = useState('')

  useEffect(() => {
    defaultApiClient.get('/org').then(res => {
      const data = res.data || res
      setOrgs(data.orgs || [])
    }).catch(() => {})
  }, [])

  const processIngestion = async () => {
    setIsLoading(true)
    onResult(null)
    try {
      const parsedData = JSON.parse(jsonData)
      const options = {
        autoExecute,
        pubKey: 'default',
        ...(selectedOrg ? { orgHash: selectedOrg } : {}),
      }
      const response = await ingestionClient.processIngestion(parsedData, options)
      if (response.success) {
        onResult({ success: true, data: response.data })
        setJsonData('')
      } else {
        onResult({ success: false, error: 'Failed to process ingestion' })
      }
    } catch (error) {
      onResult({ success: false, error: (error instanceof Error ? error.message : String(error)) || 'Failed to process ingestion' })
    } finally {
      setIsLoading(false)
    }
  }

  const loadSampleData = (type) => {
    const samples = { blogposts: generateBlogPosts(), twitter: twitterSamples, instagram: instagramSamples, linkedin: linkedinSamples, tiktok: tiktokSamples }
    setJsonData(JSON.stringify(samples[type], null, 2))
  }

  const selectedOrgName = orgs.find(o => o.org_hash === selectedOrg)?.org_name

  return (
    <div className="space-y-4">
      <div className="flex items-center justify-between">
        {SAMPLES_ENABLED && (
          <div className="flex gap-2">
            {['blogposts', 'twitter', 'instagram', 'linkedin', 'tiktok'].map(t => (
              <button key={t} onClick={() => loadSampleData(t)} className="btn-secondary btn-sm">
                {t === 'blogposts' ? 'Blog Posts (100)' : t.charAt(0).toUpperCase() + t.slice(1)}
              </button>
            ))}
          </div>
        )}
        <div className="flex items-center gap-4">
          {orgs.length > 0 && (
            <select
              value={selectedOrg}
              onChange={(e) => setSelectedOrg(e.target.value)}
              className="input-field text-sm py-1 px-2"
            >
              <option value="">Personal</option>
              {orgs.map(org => (
                <option key={org.org_hash} value={org.org_hash}>
                  {org.org_name}
                </option>
              ))}
            </select>
          )}
          <label className="flex items-center gap-2 text-sm cursor-pointer">
            <input type="checkbox" checked={autoExecute} onChange={(e) => setAutoExecute(e.target.checked)} className="checkbox" />
            <span className="text-secondary">Auto-execute</span>
          </label>
        </div>
      </div>

      <textarea
        value={jsonData}
        onChange={(e) => setJsonData(e.target.value)}
        placeholder="Enter JSON data or load a sample..."
        className="textarea h-72 font-mono"
      />

      <div className="flex justify-end items-center gap-3">
        {selectedOrg && (
          <span className="text-xs text-text-muted bg-primary/10 text-primary px-2 py-1 rounded">
            Ingesting into: {selectedOrgName}
          </span>
        )}
        <button onClick={processIngestion} disabled={isLoading || !jsonData.trim()} className="btn-primary btn-lg flex items-center gap-2">
          {isLoading ? <><span className="spinner" />Processing...</> : <>→ Process Data</>}
        </button>
      </div>
    </div>
  )
}

export default IngestionTab
