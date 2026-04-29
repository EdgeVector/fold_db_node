import { screen, fireEvent } from '@testing-library/react'
import { describe, it, expect, vi } from 'vitest'
import Header from '../../components/Header'
import { renderWithRedux } from '../utils/testUtilities.jsx'
import ingestionReducer from '../../store/ingestionSlice'

// Header uses selectIngestionConfig, so include the ingestion reducer
const extraReducers = { ingestion: ingestionReducer }

const createAuthState = (overrides = {}) => ({
  isAuthenticated: false,
  systemKeyId: null,
  publicKey: null,
  loading: false,
  error: null,
  user: null,
  ...overrides
})

describe('Header Component', () => {
  const defaultPreloadedState = {
    auth: createAuthState()
  }

  it('renders header with correct title', () => {
    renderWithRedux(<Header onSettingsClick={vi.fn()} />, {
      preloadedState: defaultPreloadedState, extraReducers
    })

    // Header shows "FoldDB"
    expect(screen.getByText(/FoldDB/i)).toBeInTheDocument()
  })

  it('has header styling', () => {
    renderWithRedux(<Header onSettingsClick={vi.fn()} />, {
      preloadedState: defaultPreloadedState, extraReducers
    })

    const header = screen.getByRole('banner')
    expect(header).toHaveClass('bg-surface', 'border-b', 'flex-shrink-0')
  })

  it('has proper semantic structure', () => {
    renderWithRedux(<Header onSettingsClick={vi.fn()} />, {
      preloadedState: defaultPreloadedState, extraReducers
    })

    const header = screen.getByRole('banner')
    expect(header).toBeInTheDocument()

    const link = screen.getByRole('link')
    expect(link).toBeInTheDocument()
    expect(link).toHaveAttribute('href', '/')
  })

  it('has proper layout classes', () => {
    renderWithRedux(<Header onSettingsClick={vi.fn()} />, {
      preloadedState: defaultPreloadedState, extraReducers
    })

    const container = screen.getByRole('banner').firstChild
    expect(container).toHaveClass('flex', 'items-center', 'justify-between')
  })

  it('title link has logo styling', () => {
    renderWithRedux(<Header onSettingsClick={vi.fn()} />, {
      preloadedState: defaultPreloadedState, extraReducers
    })

    const link = screen.getByRole('link')
    // text-base after the header tightening pass (was text-lg with the
    // 72px logo). Header.jsx → wordmark line.
    expect(link).toHaveClass('text-base', 'font-medium', 'text-primary')
  })

  it('displays settings button', () => {
    renderWithRedux(<Header onSettingsClick={vi.fn()} />, {
      preloadedState: defaultPreloadedState, extraReducers
    })

    const settingsButton = screen.getByRole('button', { name: /settings/i })
    expect(settingsButton).toBeInTheDocument()
  })

  it('displays status indicators', () => {
    renderWithRedux(<Header onSettingsClick={vi.fn()} />, {
      preloadedState: defaultPreloadedState, extraReducers
    })

    // Shows placeholder while loading storage mode
    expect(screen.getByText('...')).toBeInTheDocument()
  })

  it('calls onSettingsClick when settings button is clicked', () => {
    const mockSettingsClick = vi.fn()
    renderWithRedux(<Header onSettingsClick={mockSettingsClick} />, {
      preloadedState: defaultPreloadedState, extraReducers
    })

    const settingsButton = screen.getByRole('button', { name: /settings/i })
    fireEvent.click(settingsButton)

    expect(mockSettingsClick).toHaveBeenCalledTimes(1)
  })

  it('shows user info when authenticated', () => {
    const authenticatedState = {
      auth: createAuthState({ isAuthenticated: true, user: { id: 'testuser123456' } })
    }
    renderWithRedux(<Header onSettingsClick={vi.fn()} />, {
      preloadedState: authenticatedState, extraReducers
    })

    // Node ID is truncated to first 8 chars with copy-on-click
    expect(screen.getByText('testuser...')).toBeInTheDocument()
  })
})
