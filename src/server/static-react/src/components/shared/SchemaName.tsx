import { useState, type MouseEvent } from 'react'
import { InformationCircleIcon } from '@heroicons/react/24/outline'
import { getSchemaDisplayName, type SchemaLike } from '../../utils/schemaUtils'

interface SchemaNameProps {
  schema?: SchemaLike | null
  name?: string
  className?: string
}

export default function SchemaName({
  schema,
  name,
  className = 'font-mono text-sm text-primary font-medium',
}: SchemaNameProps) {
  const schemaName = name || schema?.name || ''
  const displayName = schema ? getSchemaDisplayName(schema) : schemaName
  const descriptive = (schema as { descriptive_name?: string } | null | undefined)?.descriptive_name
  const showHashAffordance = !!(descriptive && descriptive.trim() && descriptive !== schemaName)
  const [copied, setCopied] = useState(false)
  const [tipOpen, setTipOpen] = useState(false)

  const handleCopy = async (e: MouseEvent<HTMLButtonElement>) => {
    e.stopPropagation()
    e.preventDefault()
    try {
      await navigator.clipboard?.writeText(schemaName)
      setCopied(true)
      setTimeout(() => setCopied(false), 1500)
    } catch {
      // clipboard unavailable — tooltip still shows the value
    }
  }

  return (
    <>
      <span className={className}>{displayName}</span>
      {showHashAffordance && (
        <span className="relative inline-flex ml-1">
          <button
            type="button"
            onClick={handleCopy}
            onMouseEnter={() => setTipOpen(true)}
            onMouseLeave={() => setTipOpen(false)}
            onFocus={() => setTipOpen(true)}
            onBlur={() => setTipOpen(false)}
            aria-label={`Copy schema hash for ${displayName}`}
            className="inline-flex items-center text-tertiary hover:text-primary transition-colors p-0.5 bg-transparent border-none cursor-pointer"
          >
            <InformationCircleIcon aria-hidden="true" className="w-3.5 h-3.5" />
          </button>
          {tipOpen && (
            <span
              role="tooltip"
              className="absolute left-1/2 -translate-x-1/2 bottom-full mb-1.5 z-50 px-2 py-1 text-[11px] font-mono whitespace-nowrap bg-gruvbox-elevated text-primary border border-border rounded shadow pointer-events-none"
            >
              {copied ? 'Copied!' : (
                <>
                  {schemaName}
                  <span className="ml-2 text-tertiary">click to copy</span>
                </>
              )}
            </span>
          )}
        </span>
      )}
    </>
  )
}
