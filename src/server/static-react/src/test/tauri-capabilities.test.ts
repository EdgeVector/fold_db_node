// @ts-nocheck Migration debt: converted from .jsx in the JS->TS finalization batch; strict-mode cleanup of vi.mock typings tracked as follow-up.
import { describe, it, expect } from 'vitest'
import { readFileSync } from 'node:fs'
import { resolve, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'

const __dirname = dirname(fileURLToPath(import.meta.url))

const caps = JSON.parse(
  readFileSync(resolve(__dirname, '../../src-tauri/capabilities/default.json'), 'utf-8')
)

describe('Tauri capabilities', () => {
  it('includes dialog:allow-open required by FolderInput browse button', () => {
    expect(caps.permissions).toContain('dialog:allow-open')
  })
})
