import { useEffect, useState } from 'react'
import { useAppSelector } from '../../store/hooks'
import { selectIsAiConfigured, selectAiProvider } from '../../store/ingestionSlice'
import { useApprovedSchemas } from '../../hooks/useApprovedSchemas'
import type { StepId } from './OnboardingWizard'

interface FingerprintItemProps {
  label: string
  value: string
  color: string
}

function FingerprintItem({ label, value, color }: FingerprintItemProps) {
  return (
    <div className="flex items-center justify-between py-1.5 border-b border-border last:border-b-0">
      <span className="text-xs text-secondary">{label}</span>
      <span className={`text-xs font-medium ${color}`}>{value}</span>
    </div>
  )
}

interface AllSetStepProps {
  onFinish: () => void
  completedSteps: Set<StepId>
}

export default function AllSetStep({ onFinish, completedSteps }: AllSetStepProps) {
  const aiConfigured = useAppSelector(selectIsAiConfigured)
  const aiProvider = useAppSelector(selectAiProvider)
  const { approvedSchemas } = useApprovedSchemas()
  const [storageMode, setStorageMode] = useState('Local')

  useEffect(() => {
    async function detectStorage() {
      try {
        const resp = await fetch('/api/system/database-config')
        if (!resp.ok) throw new Error(`HTTP ${resp.status}`)
        const data = await resp.json() as { data?: { type?: string }; type?: string }
        const dbType = data?.data?.type || data?.type
        if (dbType === 'exemem') setStorageMode('Cloud (Exemem)')
        else if (dbType === 'local') setStorageMode('Local')
        else if (dbType) setStorageMode(dbType)
      } catch {
        const mode = localStorage.getItem('folddb_storage_mode')
        if (mode) setStorageMode(mode)
      }
    }
    detectStorage()
  }, [])

  const schemaCount = approvedSchemas?.length || 0

  return (
    <div>
      <h2 className="text-sm font-bold mb-1">
        <span className="text-gruvbox-green">ALL SET</span>{' '}
        <span className="text-secondary">You&apos;re ready to go</span>
      </h2>
      <p className="text-primary mb-4">Your FoldDB node is configured and ready to use.</p>

      <div className="card p-4 mb-4">
        <h3 className="text-xs font-bold text-secondary uppercase tracking-widest mb-3">Your Node</h3>
        <FingerprintItem
          label="Identity"
          value={completedSteps?.has('identity') ? 'Set' : 'Skipped'}
          color={completedSteps?.has('identity') ? 'text-gruvbox-green' : 'text-secondary'}
        />
        <FingerprintItem
          label="AI Provider"
          value={aiConfigured ? aiProvider || 'Configured' : 'Not configured'}
          color={aiConfigured ? 'text-gruvbox-green' : 'text-gruvbox-yellow'}
        />
        <FingerprintItem
          label="Storage"
          value={storageMode}
          color="text-gruvbox-blue"
        />
        <FingerprintItem
          label="Schemas"
          value={schemaCount > 0 ? `${schemaCount} approved` : 'None yet'}
          color={schemaCount > 0 ? 'text-gruvbox-green' : 'text-secondary'}
        />
        <FingerprintItem
          label="Apple Data"
          value={completedSteps?.has('apple-data') ? 'Imported' : 'Skipped'}
          color={completedSteps?.has('apple-data') ? 'text-gruvbox-green' : 'text-secondary'}
        />
        <FingerprintItem
          label="Cloud Backup"
          value={completedSteps?.has('cloud-backup') ? 'Enabled' : 'Skipped'}
          color={completedSteps?.has('cloud-backup') ? 'text-gruvbox-green' : 'text-secondary'}
        />
        <FingerprintItem
          label="Discovery"
          value={completedSteps?.has('discovery') ? 'Joined' : 'Skipped'}
          color={completedSteps?.has('discovery') ? 'text-gruvbox-green' : 'text-secondary'}
        />
      </div>

      <div className="card p-4 mb-4">
        <h3 className="text-xs font-bold text-secondary uppercase tracking-widest mb-2">Next Steps</h3>
        <ul className="space-y-1.5 text-sm text-primary">
          {!aiConfigured && (
            <li className="flex items-center gap-2">
              <span className="text-gruvbox-yellow">●</span>
              Configure AI in Settings to enable ingestion and search
            </li>
          )}
          <li className="flex items-center gap-2">
            <span className="text-gruvbox-blue">●</span>
            Use the Agent tab to chat with your data
          </li>
          <li className="flex items-center gap-2">
            <span className="text-gruvbox-blue">●</span>
            Import files via Smart Folder or File Upload
          </li>
          <li className="flex items-center gap-2">
            <span className="text-gruvbox-blue">●</span>
            Re-run this setup anytime from Settings
          </li>
        </ul>
      </div>

      <button onClick={onFinish} className="btn-primary w-full text-center">
        Go to Dashboard
      </button>
    </div>
  )
}
