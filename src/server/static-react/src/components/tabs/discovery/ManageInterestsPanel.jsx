import CategoryCard from './CategoryCard'
import PrivacyGuarantees from './PrivacyGuarantees'
import EmptyState from './EmptyState'

export default function ManageInterestsPanel({
  hasSchemas,
  configs,
  approvedSchemas,
  categoryGroups,
  categoryNames,
  optedInNames,
  publishedCategories,
  expandedCategories,
  publishFacesCategories,
  toggling,
  publishing,
  lastPublishResult,
  onToggleCategory,
  onBulkAction,
  onPublish,
  onExpandToggle,
  onPublishFacesToggle,
}) {
  if (!hasSchemas) return <EmptyState />

  return (
    <div className="space-y-4">
      {/* Bulk Actions */}
      <div className="flex items-center justify-between">
        <div className="text-xs text-secondary">
          {configs.length} of {approvedSchemas.length} schemas opted in
        </div>
        <div className="flex gap-2">
          <button
            onClick={() => onBulkAction('publish-all')}
            disabled={toggling || configs.length === approvedSchemas.length}
            className="btn-secondary btn-sm"
          >
            Opt In All
          </button>
          <button
            onClick={() => onBulkAction('unpublish-all')}
            disabled={toggling || configs.length === 0}
            className="btn-secondary btn-sm text-gruvbox-red"
          >
            Opt Out All
          </button>
        </div>
      </div>

      {/* Privacy Guarantees */}
      <PrivacyGuarantees />

      {/* Category Cards */}
      <div className="space-y-3">
        {categoryNames.map(cat => (
          <CategoryCard
            key={cat}
            category={cat}
            schemas={categoryGroups[cat]}
            optedInNames={optedInNames}
            publishedCategories={publishedCategories}
            onToggle={onToggleCategory}
            toggling={toggling}
            expanded={expandedCategories.has(cat)}
            onExpandToggle={() => onExpandToggle(cat)}
            publishFaces={publishFacesCategories.has(cat)}
            onPublishFacesToggle={onPublishFacesToggle}
          />
        ))}
      </div>

      {/* Publish Button */}
      {configs.length > 0 && (
        <button
          onClick={onPublish}
          disabled={publishing}
          className="btn-primary w-full"
        >
          {publishing
            ? 'Publishing...'
            : `Publish ${configs.length} Schema${configs.length !== 1 ? 's' : ''} to Network`}
        </button>
      )}

      {/* Last Publish Result */}
      {lastPublishResult && lastPublishResult.accepted !== undefined && (
        <div className="card-success p-3 rounded text-xs text-secondary">
          Last publish: {lastPublishResult.accepted} accepted, {lastPublishResult.quarantined} quarantined, {lastPublishResult.skipped} skipped of {lastPublishResult.total} total
        </div>
      )}
    </div>
  )
}
