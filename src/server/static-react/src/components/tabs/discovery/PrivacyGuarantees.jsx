export default function PrivacyGuarantees() {
  return (
    <div className="card-info p-3 rounded text-xs space-y-1.5">
      <div className="font-semibold text-gruvbox-blue">Privacy Guarantees</div>
      <ul className="space-y-1 text-secondary">
        <li>Only embedding vectors are shared — never raw text</li>
        <li>Each entry gets a unique pseudonym — your identity stays hidden</li>
        <li>Fields marked as sensitive are automatically excluded</li>
        <li>You can unpublish at any time to remove all shared data</li>
      </ul>
    </div>
  )
}
