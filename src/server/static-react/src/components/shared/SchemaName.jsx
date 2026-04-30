/**
 * Displays a schema's descriptive name with an optional inline info button
 * that reveals the canonical hash on hover and copies it on click.
 *
 * Used across DataBrowserTab, IngestionReport, SchemaTab, etc.
 *
 * Previously the truncated hash was rendered inline next to every name
 * ("Edge  59a46a2535c7…") on every row, every page. It was rarely
 * actionable, ate ~120px of horizontal space per row, and read as
 * leftover internal id noise. Tucked behind a small info icon: hover
 * to see the full hash, click to copy.
 */
import { useState } from 'react'
import { InformationCircleIcon } from '@heroicons/react/24/outline'
import { getSchemaDisplayName } from '../../utils/schemaUtils'

export default function SchemaName({ schema, name, className = 'font-mono text-sm text-primary font-medium' }) {
  const schemaName = name || schema?.name || ''
  const displayName = schema ? getSchemaDisplayName(schema) : schemaName
  const descriptive = schema?.descriptive_name
  const showHashAffordance = !!(descriptive && descriptive.trim() && descriptive !== schemaName)
  const [copied, setCopied] = useState(false)
  // Custom tooltip rather than the `title` attribute — native browser
  // tooltips have ~1s delay and frequently miss-fire on small icons
  // (mouse leaves the 14px target before the tooltip appears).
  const [tipOpen, setTipOpen] = useState(false)

  const handleCopy = async (e) => {
    e.stopPropagation() // don't trigger row expand
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
