import { screen, fireEvent } from '@testing-library/react'
import { describe, it, expect, vi } from 'vitest'
import Header from '../../components/Header'
import { renderWithRedux } from '../utils/testUtilities'
import ingestionReducer from '../../store/ingestionSlice'
import { systemClient } from '../../api/clients/systemClient'

vi.mock('../../api/clients/systemClient', () => ({
  systemClient: {
    getDatabaseConfig: vi.fn().mockResolvedValue({ data: null }),
    getSystemStatus: vi.fn().mockResolvedValue({ data: null }),
    getNodePublicKey: vi.fn().mockResolvedValue({ data: null }),
  },
}))

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
    renderWithRedux(<Header onSettingsClick={vi.fn()} onAiSettingsClick={vi.fn()} onCloudSettingsClick={vi.fn()} />, {
      preloadedState: defaultPreloadedState, extraReducers
    })

    // Header shows "FoldDB"
    expect(screen.getByText(/FoldDB/i)).toBeInTheDocument()
  })

  it('has header styling', () => {
    renderWithRedux(<Header onSettingsClick={vi.fn()} onAiSettingsClick={vi.fn()} onCloudSettingsClick={vi.fn()} />, {
      preloadedState: defaultPreloadedState, extraReducers
    })

    const header = screen.getByRole('banner')
    expect(header).toHaveClass('bg-surface', 'border-b', 'flex-shrink-0')
  })

  it('has proper semantic structure', () => {
    renderWithRedux(<Header onSettingsClick={vi.fn()} onAiSettingsClick={vi.fn()} onCloudSettingsClick={vi.fn()} />, {
      preloadedState: defaultPreloadedState, extraReducers
    })

    const header = screen.getByRole('banner')
    expect(header).toBeInTheDocument()

    const link = screen.getByRole('link')
    expect(link).toBeInTheDocument()
    expect(link).toHaveAttribute('href', '/')
  })

  it('has proper layout classes', () => {
    renderWithRedux(<Header onSettingsClick={vi.fn()} onAiSettingsClick={vi.fn()} onCloudSettingsClick={vi.fn()} />, {
      preloadedState: defaultPreloadedState, extraReducers
    })

    const container = screen.getByRole('banner').firstChild
    expect(container).toHaveClass('flex', 'items-center', 'justify-between')
  })

  it('title link has logo styling', () => {
    renderWithRedux(<Header onSettingsClick={vi.fn()} onAiSettingsClick={vi.fn()} onCloudSettingsClick={vi.fn()} />, {
      preloadedState: defaultPreloadedState, extraReducers
    })

    const link = screen.getByRole('link')
    // text-base after the header tightening pass (was text-lg with the
    // 72px logo). Header.jsx → wordmark line.
    expect(link).toHaveClass('text-base', 'font-medium', 'text-primary')
  })

  it('displays settings button', () => {
    renderWithRedux(<Header onSettingsClick={vi.fn()} onAiSettingsClick={vi.fn()} onCloudSettingsClick={vi.fn()} />, {
      preloadedState: defaultPreloadedState, extraReducers
    })

    const settingsButton = screen.getByRole('button', { name: /settings/i })
    expect(settingsButton).toBeInTheDocument()
  })

  it('displays status indicators', () => {
    renderWithRedux(<Header onSettingsClick={vi.fn()} onAiSettingsClick={vi.fn()} onCloudSettingsClick={vi.fn()} />, {
      preloadedState: defaultPreloadedState, extraReducers
    })

    // Shows placeholder while loading storage mode
    expect(screen.getByText('...')).toBeInTheDocument()
  })

  it('calls onSettingsClick when settings button is clicked', () => {
    const mockSettingsClick = vi.fn()
    renderWithRedux(<Header onSettingsClick={mockSettingsClick} onAiSettingsClick={vi.fn()} onCloudSettingsClick={vi.fn()} />, {
      preloadedState: defaultPreloadedState, extraReducers
    })

    const settingsButton = screen.getByRole('button', { name: /settings/i })
    fireEvent.click(settingsButton)

    expect(mockSettingsClick).toHaveBeenCalledTimes(1)
  })

  it('shows pub key chip once /api/system/public-key resolves', async () => {
    // Stale Redux user.id must NEVER leak into the chip — only the public
    // key from the live API call. Regression guard for the dogfood
    // "test_use... → LD/WctGK... → 6b65a887..." flicker.
    vi.mocked(systemClient.getNodePublicKey).mockResolvedValueOnce({
      data: { success: true, public_key: 'LD/WctGKabcdefghijklmnop=', message: '' },
    } as Awaited<ReturnType<typeof systemClient.getNodePublicKey>>)

    const authenticatedState = {
      auth: createAuthState({ isAuthenticated: true, user: { id: 'test_user_should_not_leak' } })
    }
    renderWithRedux(<Header onSettingsClick={vi.fn()} onAiSettingsClick={vi.fn()} onCloudSettingsClick={vi.fn()} />, {
      preloadedState: authenticatedState, extraReducers
    })

    // public_key truncated to first 8 chars + ellipsis
    expect(await screen.findByText('LD/WctGK...')).toBeInTheDocument()
    // The stale user.id must not appear in any form
    expect(screen.queryByText('test_use...')).not.toBeInTheDocument()
  })

  it('hides identity chip until public-key resolves', () => {
    // No mockResolvedValueOnce: getNodePublicKey returns the default
    // { data: null }, so nodePublicKey stays null and the chip must not render.
    const authenticatedState = {
      auth: createAuthState({ isAuthenticated: true, user: { id: 'test_user' } })
    }
    renderWithRedux(<Header onSettingsClick={vi.fn()} onAiSettingsClick={vi.fn()} onCloudSettingsClick={vi.fn()} />, {
      preloadedState: authenticatedState, extraReducers
    })

    // No truncated chip text of any kind on first paint
    expect(screen.queryByText(/^test_use\.\.\./)).not.toBeInTheDocument()
    expect(screen.queryByText(/\.\.\.$/)).not.toBeInTheDocument()
  })
})
