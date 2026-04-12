const SECTIONS = [
  { id: 'people', label: 'People Like You' },
  { id: 'shared-events', label: 'Shared Events' },
  { id: 'interests', label: 'Your Interests' },
  { id: 'manage', label: 'Interest Categories' },
  { id: 'search', label: 'Search Network' },
  { id: 'face-search', label: 'Face Search' },
  { id: 'requests', label: 'Received' },
  { id: 'sent', label: 'Sent' },
]

export default function DiscoverySectionNav({ activeSection, onChange }) {
  return (
    <div className="flex gap-1 border-b border-border pb-1">
      {SECTIONS.map(s => (
        <button
          key={s.id}
          onClick={() => onChange(s.id)}
          className={`px-3 py-1 text-sm rounded-t ${
            activeSection === s.id
              ? 'bg-surface text-primary border border-border border-b-surface'
              : 'text-secondary hover:text-primary'
          }`}
        >
          {s.label}
        </button>
      ))}
    </div>
  )
}
