interface RoleSelectProps {
  value: string
  onChange: (value: string) => void
}

/** Inline role selector for the connect flow. */
export default function RoleSelect({ value, onChange }: RoleSelectProps) {
  return (
    <select
      value={value}
      onChange={(e) => onChange(e.target.value)}
      className="input text-xs w-32"
    >
      <option value="acquaintance">Acquaintance</option>
      <option value="friend">Friend</option>
      <option value="inner_circle">Inner Circle</option>
    </select>
  )
}
