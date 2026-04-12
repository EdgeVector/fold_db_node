export default function EmptyState() {
  return (
    <div className="card p-8 text-center space-y-4 rounded">
      <div className="text-3xl">
        {/* Simple icon using unicode */}
        <span className="text-gruvbox-yellow">&#9776;</span>
      </div>
      <div>
        <h3 className="text-lg text-primary font-semibold">No data to discover yet</h3>
        <p className="text-secondary text-sm mt-2 max-w-md mx-auto">
          Ingest some data first using the Data tab. Once you have schemas with data,
          you can choose which categories to share on the discovery network.
        </p>
      </div>
      <div className="card-info p-3 rounded text-xs text-secondary max-w-sm mx-auto">
        Discovery lets others find your data by topic — without revealing your identity
        or the actual content. You stay in full control of what gets shared.
      </div>
    </div>
  )
}
