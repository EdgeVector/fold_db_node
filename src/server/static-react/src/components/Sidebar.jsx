import { useState } from 'react'
import { version } from '../../package.json'

const SIDEBAR_ITEMS = [
  { id: 'agent', label: 'Agent', icon: '\u{1F916}', group: 'MAIN' },
  { id: 'data-browser', label: 'Browse', icon: '\u{1F4CA}', group: 'DATA' },
  { id: 'query', label: 'Query', icon: '\u{1F50D}', group: 'DATA' },
  { id: 'schemas', label: 'Schemas', icon: '\u{1F4CB}', group: 'DATA' },
  { id: 'smart-folder', label: 'Import', icon: '\u{1F4E5}', group: 'IMPORT' },
  { id: 'file-upload', label: 'Files', icon: '\u{1F4C4}', group: 'IMPORT' },
  { id: 'people', label: 'People', icon: '\u{1F465}', group: 'SOCIAL' },
  { id: 'discovery', label: 'Discover', icon: '\u{1F310}', group: 'SOCIAL' },
  { id: 'settings', label: 'Settings', icon: '\u2699\uFE0F', group: 'SYSTEM' },
]

const GROUPS = ['MAIN', 'DATA', 'IMPORT', 'SOCIAL', 'SYSTEM']

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
              <div className="text-[10px] uppercase tracking-widest text-tertiary px-4 pt-4 pb-1">
                {group}
              </div>
              {grouped[group].map(item => {
                const isActive = activeTab === item.id
                return (
                  <button
                    key={item.id}
                    onClick={() => handleTabChange(item.id)}
                    className={`w-full text-left px-4 py-2 text-sm flex items-center gap-2 transition-colors border-l-2
                      ${isActive
                        ? 'bg-surface-secondary border-l-gruvbox-yellow text-primary'
                        : 'text-secondary hover:text-primary hover:bg-surface-secondary border-l-transparent'
                      }`}
                    aria-current={isActive ? 'page' : undefined}
                  >
                    <span>{item.icon}</span>
                    <span>{item.label}</span>
                  </button>
                )
              })}
            </div>
          ))}
        </div>

        <div className="border-t border-border">
          <div className="px-4 pb-3 pt-2 text-[10px] text-tertiary">
            v{version}
          </div>
        </div>
      </nav>
    </>
  )
}

export { SIDEBAR_ITEMS }
export default Sidebar
