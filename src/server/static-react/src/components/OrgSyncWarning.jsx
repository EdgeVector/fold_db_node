function OrgSyncWarning({ onClick }) {
  return (
    <button
      onClick={onClick}
      className="bg-transparent border-none cursor-pointer p-0 font-mono text-sm text-gruvbox-orange hover:text-primary flex items-center gap-1"
      title="One or more orgs have sync errors — click to open Settings"
    >
      <span className="text-base leading-none">!</span>
      <span>Org sync issue</span>
    </button>
  )
}

export default OrgSyncWarning
