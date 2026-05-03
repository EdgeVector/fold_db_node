// Grouped sub-tab nav. Mirrors PeopleTab's SUB_TAB_GROUPS layout so the two
// social surfaces read the same. The groups describe the user's intent:
//
//   Meet people       - find new peers on the network
//   Requests          - the back-and-forth before a connection is formed
//   Share             - what you expose so others can find you
//   With connections  - features that only work with accepted connections
//
// Every existing section id is preserved so deep links keep working.
const SECTION_GROUPS = [
  {
    label: 'Meet people',
    sections: [
      { id: 'people', label: 'People like you' },
      { id: 'search', label: 'Search' },
      { id: 'face-search', label: 'Face search' },
    ],
  },
  {
    label: 'Requests',
    sections: [
      { id: 'requests', label: 'Received' },
      { id: 'sent', label: 'Sent' },
    ],
  },
  {
    label: 'Share',
    sections: [
      { id: 'interests', label: 'Your interests' },
      { id: 'manage', label: 'Data types' },
    ],
  },
  {
    label: 'With connections',
    sections: [
      { id: 'shared-events', label: 'Shared events' },
    ],
  },
]

export default function DiscoverySectionNav({ activeSection, onChange }) {
  return (
    <div className="border-b border-border mb-4 flex flex-wrap items-end gap-x-6 gap-y-2">
      {SECTION_GROUPS.map((group, idx) => (
        <div
          key={group.label}
          className={`flex flex-col ${idx > 0 ? 'sm:border-l sm:border-border sm:pl-6' : ''}`}
        >
          <div className="text-[10px] uppercase tracking-widest text-tertiary px-2 pb-1">
            {group.label}
          </div>
          <div className="flex flex-wrap">
            {group.sections.map(s => (
              <button
                key={s.id}
                onClick={() => onChange(s.id)}
                className={`tab ${activeSection === s.id ? 'tab-active' : ''}`}
              >
                {s.label}
              </button>
            ))}
          </div>
        </div>
      ))}
    </div>
  )
}
