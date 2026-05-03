import '@testing-library/jest-dom'
import { vi, beforeEach, type Mock } from 'vitest'
import { setupTestEnvironment, cleanupTestEnvironment } from './utils/testUtilities'
import { setupMockServer } from './mocks/apiMocks'
import { TEST_TIMEOUT_DEFAULT_MS } from './config/constants'

// Make vi available globally as jest for compatibility
;(globalThis as Record<string, unknown>).jest = vi

// Mock the production store to prevent Redux warnings during imports
vi.mock('../store/store', () => ({
  store: {
    getState: vi.fn(() => ({
      auth: {
        isAuthenticated: false,
        systemKeyId: null,
        publicKey: null,
        loading: false,
        error: null
      },
      schemas: {
        schemas: {},
        loading: { fetch: false, operations: {} },
        errors: { fetch: null, operations: {} },
        lastFetched: null,
        cache: { ttl: 300000, version: '2.1.0', lastUpdated: null },
        activeSchema: null
      }
    })),
    dispatch: vi.fn(),
    subscribe: vi.fn(),
    replaceReducer: vi.fn()
  }
}))

// Setup WebCrypto API for tests
import { webcrypto } from 'node:crypto'
Object.defineProperty(globalThis, 'crypto', {
  value: webcrypto,
})

// Mock Response for MSW tests
global.Response = Response
;(globalThis as Record<string, unknown>).TEST_TIMEOUT_MS = TEST_TIMEOUT_DEFAULT_MS

// Setup test environment with mocks and matchers
setupTestEnvironment()

// Setup MSW server for API mocking
setupMockServer()

// Mock EventSource for LogSidebar component
global.EventSource = vi.fn(() => ({
  onmessage: null,
  onerror: null,
  close: vi.fn(),
  addEventListener: vi.fn(),
  removeEventListener: vi.fn(),
})) as unknown as typeof EventSource

// Mock scrollIntoView for DOM elements
Element.prototype.scrollIntoView = vi.fn()

// Mock console methods to avoid noise in tests (but keep original for debugging)
const originalConsole = console
global.console = {
  ...console,
  error: vi.fn(),
  warn: vi.fn(),
  log: vi.fn(),
  debug: originalConsole.debug // Keep debug for test debugging
}

// Set default test timeout
vi.setConfig({ testTimeout: TEST_TIMEOUT_DEFAULT_MS })

// Reset all mocks before each test
beforeEach(() => {
  vi.clearAllMocks()
  cleanupTestEnvironment()

  const fetchMock = fetch as unknown as Mock | undefined
  if (fetchMock && typeof fetchMock.mockClear === 'function') {
    fetchMock.mockClear()
  }
  const eventSourceMock = global.EventSource as unknown as Mock | undefined
  if (eventSourceMock && typeof eventSourceMock.mockClear === 'function') {
    eventSourceMock.mockClear()
  }
  const scrollIntoViewMock = Element.prototype.scrollIntoView as unknown as Mock | undefined
  if (scrollIntoViewMock && typeof scrollIntoViewMock.mockClear === 'function') {
    scrollIntoViewMock.mockClear()
  }
})
