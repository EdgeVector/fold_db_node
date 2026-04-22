export default function PrivacyGuarantees() {
  return (
    <div className="card-info p-3 rounded text-xs space-y-1.5">
      <div className="font-semibold text-gruvbox-blue">Privacy guarantees</div>
      <ul className="space-y-1 text-secondary">
        <li>Only anonymized fingerprints are shared — never your raw text or files</li>
        <li>You appear as a random network ID — your identity stays hidden until you accept a connection</li>
        <li>Fields marked sensitive are automatically excluded</li>
        <li>Hit <em>Stop sharing all</em> anytime to pull everything back</li>
      </ul>
    </div>
  )
}
