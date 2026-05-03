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

  const totalSchemas = approvedSchemas.length

  return (
    <div className="space-y-4">
      <p className="text-sm text-secondary">
        Pick which of your data types are visible on the discovery network. Only
        anonymized fingerprints are shared — never the raw content. Change your
        mind anytime and hit <em>Update network</em> to push it.
      </p>

      {/* Bulk Actions */}
      <div className="flex items-center justify-between">
        <div className="text-xs text-secondary">
          Sharing {configs.length} of {totalSchemas} data type{totalSchemas !== 1 ? 's' : ''}
        </div>
        <div className="flex gap-2">
          <button
            onClick={() => onBulkAction('publish-all')}
            disabled={toggling || configs.length === totalSchemas}
            className="btn-secondary btn-sm"
          >
            Share all
          </button>
          <button
            onClick={() => onBulkAction('unpublish-all')}
            disabled={toggling || configs.length === 0}
            className="btn-secondary btn-sm text-gruvbox-red"
          >
            Stop sharing all
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
          {publishing ? 'Updating...' : 'Update network share'}
        </button>
      )}

      {/* Last Publish Result */}
      {lastPublishResult && lastPublishResult.accepted !== undefined && (
        <div className="card-success p-3 rounded text-xs text-secondary">
          Last update: {lastPublishResult.accepted} accepted, {lastPublishResult.quarantined} quarantined, {lastPublishResult.skipped} skipped of {lastPublishResult.total} total
        </div>
      )}
    </div>
  )
}
