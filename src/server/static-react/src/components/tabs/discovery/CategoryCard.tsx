import { buildPreviewItems, fieldCount } from './discoveryUtils'

function ToggleSwitch({ enabled, onChange, disabled }) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={enabled}
      disabled={disabled}
      onClick={() => onChange(!enabled)}
      className={`relative inline-flex h-5 w-9 items-center rounded-full transition-colors ${
        disabled ? 'opacity-50 cursor-not-allowed' : 'cursor-pointer'
      } ${enabled ? 'bg-gruvbox-green' : 'bg-gruvbox-elevated border border-border'}`}
    >
      <span
        className={`inline-block h-3.5 w-3.5 rounded-full bg-primary transition-transform ${
          enabled ? 'translate-x-[18px]' : 'translate-x-[3px]'
        }`}
      />
    </button>
  )
}

export default function CategoryCard({
  category,
  schemas,
  optedInNames,
  publishedCategories,
  onToggle,
  toggling,
  expanded,
  onExpandToggle,
  publishFaces,
  onPublishFacesToggle,
}) {
  const allOptedIn = schemas.every(s => optedInNames.has(s.name))
  const someOptedIn = schemas.some(s => optedInNames.has(s.name))
  const isPublished = publishedCategories.has(category)
  const previewItems = buildPreviewItems(schemas)

  return (
    <div className="card rounded p-4 space-y-2">
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-3">
          <ToggleSwitch
            enabled={allOptedIn}
            onChange={(val) => onToggle(category, schemas, val)}
            disabled={toggling}
          />
          <div>
            <div className="flex items-center gap-2">
              <span className="font-semibold text-sm text-primary">{category}</span>
              {isPublished && (
                <span className="badge badge-success">published</span>
              )}
              {someOptedIn && !isPublished && (
                <span className="badge badge-warning">unpublished</span>
              )}
            </div>
            <div className="text-xs text-secondary mt-0.5">
              {schemas.length} schema{schemas.length !== 1 ? 's' : ''} &middot; {fieldCount(schemas)} field{fieldCount(schemas) !== 1 ? 's' : ''}
            </div>
          </div>
        </div>
        <button
          onClick={onExpandToggle}
          className="text-xs text-secondary hover:text-primary transition-colors"
        >
          {expanded ? 'Hide preview' : 'Show preview'}
        </button>
      </div>

      {/*
        Inline face-publish opt-in. Visible when this category is opted-in.
        Previously this was buried inside the "Show preview" panel — users
        opting in to Photography had no signal that face publishing was a
        separate, off-by-default toggle. Surfacing it next to the switch
        makes the choice explicit and discoverable.
      */}
      {someOptedIn && (
        <label className="flex items-center gap-2 ml-12 cursor-pointer select-none">
          <input
            type="checkbox"
            checked={publishFaces}
            onChange={(e) => onPublishFacesToggle(category, e.target.checked)}
            disabled={toggling}
            className="accent-gruvbox-green"
          />
          <span className="text-xs text-secondary">
            Also share face fingerprints <span className="text-tertiary">(people in your photos become findable on the network)</span>
          </span>
        </label>
      )}

      {expanded && (
        <div className="border-t border-border pt-3 space-y-2">
          <div className="text-xs text-secondary font-semibold">
            What will be shared:
          </div>
          {previewItems.length === 0 ? (
            <div className="text-xs text-tertiary">No fields detected</div>
          ) : (
            <div className="grid grid-cols-1 sm:grid-cols-2 gap-1">
              {previewItems.map((item, i) => (
                <div key={i} className="flex items-center gap-2 text-xs">
                  <span className="text-gruvbox-blue font-mono">{item.field}</span>
                  <span className="text-tertiary">({item.type})</span>
                  <span className="text-tertiary">from {item.schema}</span>
                </div>
              ))}
            </div>
          )}
          <div className="text-xs text-tertiary mt-1">
            Embedding vectors of these fields will be published — raw text is never shared.
          </div>
        </div>
      )}
    </div>
  )
}
