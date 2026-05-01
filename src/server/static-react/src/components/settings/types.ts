import type { ReactNode } from 'react'

/**
 * Result of a settings-panel save action. Rendered by both the panel
 * itself (as a styled card) and indirectly by the host tab. Centralized
 * here so the producers (`AiConfigSettings`, `DatabaseSettings`) and
 * consumers (`SettingsTab`) cannot drift in shape — the bug we just
 * fixed (#unknown — Apr 2026) was a JSX consumer rendering this object
 * directly as a React child.
 */
export interface SaveStatus {
  success: boolean
  message: string
}

export interface SettingsPanelProps {
  configSaveStatus: SaveStatus | null
  setConfigSaveStatus: (status: SaveStatus | null) => void
  onClose: () => void
}

export interface SettingsPanelHook {
  content: ReactNode
}
