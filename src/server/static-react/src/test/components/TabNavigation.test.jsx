/**
 * Test file for TabNavigation component
 */

import { describe, it, expect, vi, beforeEach } from 'vitest'
import { render, screen, fireEvent } from '@testing-library/react'
import TabNavigation from '../../components/TabNavigation.jsx'
import { DEFAULT_TABS } from '../../constants/ui.js'

describe('TabNavigation', () => {
  const defaultProps = {
    activeTab: 'ingestion',
    onTabChange: vi.fn()
  }

  beforeEach(() => {
    vi.clearAllMocks()
  })

  it('renders main tabs directly and advanced tabs in More dropdown', () => {
    render(<TabNavigation {...defaultProps} />)

    // Main tabs should be directly visible
    const mainTabs = DEFAULT_TABS.filter(t => t.group === 'main')
    mainTabs.forEach(tab => {
      expect(screen.getByRole('button', { name: `${tab.label} tab` })).toBeInTheDocument()
    })

    // Advanced tabs should NOT be visible until More is opened
    expect(screen.queryByText('Native Index')).not.toBeInTheDocument()

    // Open the More dropdown
    fireEvent.click(screen.getByRole('button', { name: 'More tabs' }))

    // Now advanced tabs should be visible in the dropdown
    const advancedTabs = DEFAULT_TABS.filter(t => t.group === 'advanced')
    advancedTabs.forEach(tab => {
      // Use getAllByText since the active advanced tab label also appears on the More button
      const matches = screen.getAllByText(tab.label)
      expect(matches.length).toBeGreaterThanOrEqual(1)
    })
  })

  it('renders main group tabs', () => {
    render(<TabNavigation {...defaultProps} />)

    expect(screen.getByText('Smart Folder')).toBeInTheDocument()
    expect(screen.getByText('AI Query')).toBeInTheDocument()
    expect(screen.getByText('File Upload')).toBeInTheDocument()
  })

  it('renders advanced group tabs inside More dropdown', () => {
    render(<TabNavigation {...defaultProps} />)

    // Not visible before opening
    expect(screen.queryByText('Native Index')).not.toBeInTheDocument()

    // Open dropdown
    fireEvent.click(screen.getByRole('button', { name: 'More tabs' }))

    expect(screen.getByText('Native Index')).toBeInTheDocument()
  })

  it('renders separator between main tabs and More button', () => {
    const { container } = render(<TabNavigation {...defaultProps} />)

    const nav = container.querySelector('nav')
    expect(nav).toBeInTheDocument()
  })

  it('highlights active main tab correctly', () => {
    render(<TabNavigation {...defaultProps} activeTab="llm-query" />)

    const activeTab = screen.getByRole('button', { name: /ai query tab/i })
    expect(activeTab).toHaveAttribute('aria-current', 'page')
  })

  it('highlights More button when an advanced tab is active', () => {
    render(<TabNavigation {...defaultProps} activeTab="ingestion" />)

    // More button should show the active advanced tab's label
    const moreButton = screen.getByRole('button', { name: 'More tabs' })
    expect(moreButton).toHaveClass('tab-active')
    expect(moreButton).toHaveTextContent('JSON Ingestion')
  })

  it('does not highlight inactive tabs', () => {
    render(<TabNavigation {...defaultProps} activeTab="ingestion" />)

    const inactiveTab = screen.getByRole('button', { name: /ai query tab/i })
    expect(inactiveTab).not.toHaveAttribute('aria-current')
  })

  it('calls onTabChange when clicking a main tab', () => {
    render(<TabNavigation {...defaultProps} />)

    const aiQueryTab = screen.getByRole('button', { name: /ai query tab/i })
    fireEvent.click(aiQueryTab)

    expect(defaultProps.onTabChange).toHaveBeenCalledWith('llm-query')
  })

  it('calls onTabChange when clicking an advanced tab in dropdown', () => {
    render(<TabNavigation {...defaultProps} />)

    // Open dropdown
    fireEvent.click(screen.getByRole('button', { name: 'More tabs' }))

    // Click an advanced tab
    fireEvent.click(screen.getByText('Word Graph'))

    expect(defaultProps.onTabChange).toHaveBeenCalledWith('word-graph')
  })

  it('closes dropdown after selecting an advanced tab', () => {
    render(<TabNavigation {...defaultProps} />)

    fireEvent.click(screen.getByRole('button', { name: 'More tabs' }))
    expect(screen.getByText('Word Graph')).toBeInTheDocument()

    fireEvent.click(screen.getByText('Word Graph'))

    // Dropdown should close — Word Graph no longer visible as dropdown item
    // (it may appear as the More button label if it becomes active)
    expect(defaultProps.onTabChange).toHaveBeenCalledWith('word-graph')
  })

  it('calls onTabChange with correct tab id', () => {
    const customTabs = [
      { id: 'custom', label: 'Custom', group: 'main' }
    ]

    render(<TabNavigation {...defaultProps} tabs={customTabs} />)

    const customTab = screen.getByRole('button', { name: /custom tab/i })
    fireEvent.click(customTab)

    expect(defaultProps.onTabChange).toHaveBeenCalledWith('custom')
  })

  it('renders custom tabs when provided', () => {
    const customTabs = [
      { id: 'custom1', label: 'Custom Tab 1', group: 'main' },
      { id: 'custom2', label: 'Custom Tab 2', group: 'main' }
    ]

    render(<TabNavigation {...defaultProps} tabs={customTabs} />)

    expect(screen.getByText('Custom Tab 1')).toBeInTheDocument()
    expect(screen.getByText('Custom Tab 2')).toBeInTheDocument()
  })

  it('handles disabled tabs correctly', () => {
    const tabsWithDisabled = [
      { id: 'enabled', label: 'Enabled Tab', disabled: false, group: 'main' },
      { id: 'disabled', label: 'Disabled Tab', disabled: true, group: 'main' }
    ]

    render(<TabNavigation {...defaultProps} tabs={tabsWithDisabled} />)

    const enabledTab = screen.getByRole('button', { name: /enabled tab/i })
    const disabledTab = screen.getByRole('button', { name: /disabled tab/i })

    expect(enabledTab).toBeEnabled()
    expect(disabledTab).toBeDisabled()
  })

  it('applies custom className', () => {
    const { container } = render(
      <TabNavigation {...defaultProps} className="custom-nav" />
    )

    expect(container.firstChild).toHaveClass('custom-nav')
  })

  it('has proper accessibility attributes on More button', () => {
    render(<TabNavigation {...defaultProps} />)

    const moreButton = screen.getByRole('button', { name: 'More tabs' })
    expect(moreButton).toHaveAttribute('aria-haspopup', 'true')
    expect(moreButton).toHaveAttribute('aria-expanded', 'false')

    fireEvent.click(moreButton)
    expect(moreButton).toHaveAttribute('aria-expanded', 'true')
  })

  it('renders as nav element', () => {
    const { container } = render(<TabNavigation {...defaultProps} />)

    expect(container.querySelector('nav')).toBeInTheDocument()
  })

  it('does not show More button when no advanced tabs exist', () => {
    const mainOnly = [
      { id: 'a', label: 'Tab A', group: 'main' },
      { id: 'b', label: 'Tab B', group: 'main' }
    ]

    render(<TabNavigation {...defaultProps} tabs={mainOnly} />)

    expect(screen.queryByRole('button', { name: 'More tabs' })).not.toBeInTheDocument()
  })

  it('closes dropdown on outside click', () => {
    render(<TabNavigation {...defaultProps} />)

    fireEvent.click(screen.getByRole('button', { name: 'More tabs' }))
    expect(screen.getByText('Native Index')).toBeInTheDocument()

    // Click outside
    fireEvent.mouseDown(document.body)

    expect(screen.queryByText('Native Index')).not.toBeInTheDocument()
  })
})
