export default function IdentityCardPanel({
  identityLoading,
  identityCard,
  editName,
  setEditName,
  editHint,
  setEditHint,
  handleSaveIdentity,
  savingIdentity,
}) {
  return (
    <div className="border border-border rounded-lg p-4 bg-surface">
      <h3 className="text-sm font-medium text-primary mb-1">Identity Card</h3>
      <p className="text-xs text-secondary mb-4">
        Your display name and contact hint are shared only with people you send trust invites to.
        This information stays on your device and is never synced to Exemem.
      </p>

      {identityLoading ? (
        <div className="text-center py-8">
          <div className="w-6 h-6 border-2 border-border border-t-primary rounded-full animate-spin mx-auto" />
        </div>
      ) : (
        <form onSubmit={handleSaveIdentity} className="space-y-4">
          <div>
            <label className="block text-xs text-secondary mb-1">Display Name *</label>
            <input
              className="input w-full"
              type="text"
              placeholder="Your name..."
              value={editName}
              onChange={(e) => setEditName(e.target.value)}
            />
          </div>
          <div>
            <label className="block text-xs text-secondary mb-1">Contact Hint (optional)</label>
            <input
              className="input w-full"
              type="text"
              placeholder="Email, phone, or handle for verification..."
              value={editHint}
              onChange={(e) => setEditHint(e.target.value)}
            />
            <p className="text-xs text-tertiary mt-1">
              Helps others verify it's really you when they receive your trust invite.
            </p>
          </div>
          <button
            type="submit"
            className="btn"
            disabled={savingIdentity || !editName.trim()}
          >
            {savingIdentity ? 'Saving...' : (identityCard ? 'Update' : 'Save')}
          </button>
        </form>
      )}
    </div>
  )
}
