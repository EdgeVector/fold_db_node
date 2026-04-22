export default function EmptyState() {
  return (
    <div className="card p-8 text-center space-y-4 rounded">
      <div className="text-3xl">
        <span className="text-gruvbox-yellow">&#9776;</span>
      </div>
      <div>
        <h3 className="text-lg text-primary font-semibold">Nothing to share yet</h3>
        <p className="text-secondary text-sm mt-2 max-w-md mx-auto">
          Import some data first in the <span className="text-primary">Import</span> tab.
          Once you have a few things, come back here to pick what you're comfortable sharing.
        </p>
      </div>
      <div className="card-info p-3 rounded text-xs text-secondary max-w-sm mx-auto">
        Sharing only exposes anonymized fingerprints by topic — never your identity or
        the actual content. You stay in control.
      </div>
    </div>
  )
}
