import { useState } from 'react'
import {
  SparklesIcon,
  CircleStackIcon,
  FolderIcon,
  DocumentArrowUpIcon,
  ArrowDownOnSquareIcon,
  UsersIcon,
  GlobeAltIcon,
  MagnifyingGlassIcon,
  TableCellsIcon,
  Cog6ToothIcon,
} from '@heroicons/react/24/outline'
// Note: app version is shown once, in the Footer. Don't duplicate it here.

// Sidebar items render with Heroicons outline (24-stroke) glyphs so the
// icon weight matches the IBM Plex Mono UI and respects currentColor.
// Previously emoji ('\u{1F916}' etc.) \u2014 those rendered at OS-default
// saturation, which blew out the muted Gruvbox palette and looked like
// a SaaS template. See design review notes in PR description.
const SIDEBAR_ITEMS = [
  { id: 'agent', label: 'Agent', Icon: SparklesIcon, group: 'MAIN' },
  { id: 'data-browser', label: 'Browse', Icon: CircleStackIcon, group: 'DATA' },
  { id: 'smart-folder', label: 'Import', Icon: FolderIcon, group: 'IMPORT' },
  { id: 'file-upload', label: 'Files', Icon: DocumentArrowUpIcon, group: 'IMPORT' },
  { id: 'apple-import', label: 'Apple', Icon: ArrowDownOnSquareIcon, group: 'IMPORT' },
  { id: 'people', label: 'People', Icon: UsersIcon, group: 'SOCIAL' },
  { id: 'discovery', label: 'Discover', Icon: GlobeAltIcon, group: 'SOCIAL' },
  { id: 'query', label: 'Query', Icon: MagnifyingGlassIcon, group: 'ADMIN' },
  { id: 'schemas', label: 'Schemas', Icon: TableCellsIcon, group: 'ADMIN' },
  { id: 'settings', label: 'Settings', Icon: Cog6ToothIcon, group: 'SYSTEM' },
]

const GROUPS = ['MAIN', 'DATA', 'IMPORT', 'SOCIAL', 'ADMIN', 'SYSTEM']

function Sidebar({ activeTab, onTabChange }) {
  const [mobileOpen, setMobileOpen] = useState(false)
  const grouped = {}
  for (const group of GROUPS) {
    grouped[group] = SIDEBAR_ITEMS.filter(item => item.group === group)
  }

  const handleTabChange = (id) => {
    onTabChange(id)
    setMobileOpen(false) // auto-close on mobile after selection
  }

  return (
    <>
      {/* Mobile hamburger toggle */}
      <button
        onClick={() => setMobileOpen(!mobileOpen)}
        className="sm:hidden fixed bottom-4 left-4 z-50 bg-gruvbox-yellow text-surface w-10 h-10 rounded-full flex items-center justify-center shadow-lg border-none cursor-pointer text-lg"
        aria-label="Toggle navigation"
      >
        {mobileOpen ? '\u2715' : '\u2630'}
      </button>

      {/* Overlay for mobile */}
      {mobileOpen && (
        <div
          className="sm:hidden fixed inset-0 bg-black/50 z-30"
          onClick={() => setMobileOpen(false)}
        />
      )}

      <nav className={`
        bg-surface border-r border-border h-full overflow-y-auto flex flex-col shrink-0
        fixed sm:static z-40 top-0 left-0
        w-44 transition-transform duration-200
        ${mobileOpen ? 'translate-x-0' : '-translate-x-full sm:translate-x-0'}
      `}>
        <div className="flex-1">
          {GROUPS.map(group => (
            <div key={group}>
              <div className="text-[10px] uppercase tracking-widest text-tertiary px-4 pt-3 pb-0.5">
                {group}
              </div>
              {grouped[group].map(item => {
                const isActive = activeTab === item.id
                const Icon = item.Icon
                return (
                  <button
                    key={item.id}
                    onClick={() => handleTabChange(item.id)}
                    className={`w-full text-left px-4 py-1.5 text-sm flex items-center gap-2 transition-colors border-l-2
                      ${isActive
                        ? 'bg-surface-secondary border-l-gruvbox-yellow text-primary'
                        : 'text-secondary hover:text-primary hover:bg-surface-secondary border-l-transparent'
                      }`}
                    aria-current={isActive ? 'page' : undefined}
                  >
                    <Icon aria-hidden="true" className="w-4 h-4 shrink-0" />
                    <span>{item.label}</span>
                  </button>
                )
              })}
            </div>
          ))}
        </div>
      </nav>
    </>
  )
}

export { SIDEBAR_ITEMS }
export default Sidebar
