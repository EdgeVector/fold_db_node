import { describe, it, expect, beforeEach, afterEach } from 'vitest'
import { renderHook, act } from '@testing-library/react'
import { useTabRouting } from '../../hooks/useTabRouting'
import { DEFAULT_TAB } from '../../constants'

// Regression: QA #533 — hash-URL navigation out-of-sync with tab state.
// Found by /qa on 2026-04-18.
// Report: .gstack/qa-reports/qa-report-localhost-5175-2026-04-18.md
//
// Scope of the regression:
//   1. Sidebar-label aliases (`browse`, `discover`) were not in HASH_TO_TAB,
//      so URLs users would intuit from sidebar labels were silently ignored.
//   2. When the hash was unresolvable, the URL was left as-is while the UI
//      stayed on the previous tab — address bar out-of-sync with rendered
//      content.

describe('useTabRouting — hash handling (regression for QA #533)', () => {
  const originalHash = globalThis.location?.hash

  beforeEach(() => {
    window.location.hash = ''
  })

  afterEach(() => {
    window.location.hash = originalHash || ''
  })

  it('resolves sidebar-label alias /#browse to data-browser', () => {
    window.location.hash = 'browse'
    const { result } = renderHook(() => useTabRouting())
    expect(result.current.activeTab).toBe('data-browser')
  })

  it('resolves sidebar-label alias /#discover to discovery', () => {
    window.location.hash = 'discover'
    const { result } = renderHook(() => useTabRouting())
    expect(result.current.activeTab).toBe('discovery')
  })

  it('keeps canonical /#data-browser working', () => {
    window.location.hash = 'data-browser'
    const { result } = renderHook(() => useTabRouting())
    expect(result.current.activeTab).toBe('data-browser')
  })

  it('normalizes unknown hash so URL reflects rendered tab', () => {
    window.location.hash = 'not-a-real-tab'
    const { result } = renderHook(() => useTabRouting())
    // hook should NOT set an unknown tab, and should rewrite URL to match
    expect(result.current.activeTab).toBe(DEFAULT_TAB)
    expect(window.location.hash).toBe(`#${DEFAULT_TAB}`)
  })

  it('leaves the hash alone when it matches an alias (alias stays for bookmarkability)', () => {
    window.location.hash = 'browse'
    renderHook(() => useTabRouting())
    // alias resolved to data-browser, but we don't rewrite the hash —
    // user's bookmark remains `/#browse` and keeps working
    expect(window.location.hash).toBe('#browse')
  })

  it('responds to hashchange events after mount', () => {
    const { result } = renderHook(() => useTabRouting())
    expect(result.current.activeTab).toBe(DEFAULT_TAB)

    act(() => {
      window.location.hash = 'data-browser'
      window.dispatchEvent(new HashChangeEvent('hashchange'))
    })

    expect(result.current.activeTab).toBe('data-browser')
  })

  it('handleTabChange writes the hash for two-way sync', () => {
    const { result } = renderHook(() => useTabRouting())
    act(() => {
      result.current.handleTabChange('schemas')
    })
    expect(result.current.activeTab).toBe('schemas')
    expect(window.location.hash).toBe('#schemas')
  })
})
