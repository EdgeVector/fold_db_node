/**
 * @fileoverview Consolidated Testing Utilities
 * TASK-010: Test Suite Fixes and Validation for PBI-REACT-SIMPLIFY-001
 * 
 * Unified testing utilities that eliminate duplication between testStore.jsx
 * and testingUtilities.jsx files. Provides comprehensive testing support
 * for Redux store setup, component rendering, API mocking, and validation.
 * 
 * @module testUtilities
 * @since 2.1.0
 */

import React, { type ReactElement, type ReactNode } from 'react';
import { render, renderHook, type RenderOptions } from '@testing-library/react';
import { Provider } from 'react-redux';
import { combineReducers, configureStore, type EnhancedStore, type Reducer, type ReducersMapObject } from '@reduxjs/toolkit';
import { vi, expect } from 'vitest';
import {
  TEST_TIMEOUT_DEFAULT_MS,
  MOCK_API_DELAY_MS,
  COVERAGE_THRESHOLD_PERCENT,
  TEST_VALIDATION_BATCH_SIZE,
} from '../config/constants';
import authReducer from '../../store/authSlice';
import schemaReducer from '../../store/schemaSlice';
import { SCHEMA_STATES } from '../../constants/schemas';

type TestStore = EnhancedStore;
// eslint-disable-next-line @typescript-eslint/no-explicit-any
type ExtraReducers = ReducersMapObject<Record<string, any>>;

export { SCHEMA_STATES };

/**
 * Creates a test store with optional initial state
 * Combines the best features from both previous implementations
 * @param {Object} preloadedState - Initial state for the store
 * @returns {Object} Configured test store
 */
/**
 * Creates a test store with optional initial state and extra reducers.
 * By default only includes auth + schemas reducers. Tests that render
 * components using other slices (e.g. ingestion) should pass extra
 * reducers via the second argument.
 */
export function createTestStore(
  preloadedState: Record<string, unknown> = {},
  { extraReducers = {} as ExtraReducers }: { extraReducers?: ExtraReducers } = {}
) {
  const defaultState = {
    auth: {
      isAuthenticated: false,
      systemKeyId: null,
      publicKey: null,
      loading: false,
      error: null
    },
    schemas: {
      schemas: {},  // Match actual store structure - object indexed by schema ID
      loading: {
        fetch: false,
        operations: {}
      },
      errors: {
        fetch: null,
        operations: {}
      },
      lastFetched: null,
      cache: {
        ttl: 300000,
        version: '2.1.0',
        lastUpdated: null
      },
      activeSchema: null
    }
  };

  return configureStore({
    reducer: combineReducers({
      auth: authReducer as Reducer,
      schemas: schemaReducer as Reducer,
      ...extraReducers,
    }),
    preloadedState: {
      ...defaultState,
      ...preloadedState
    } as never,
    middleware: (getDefaultMiddleware) =>
      getDefaultMiddleware({
        serializableCheck: {
          // Ignore these action types
          ignoredActions: [
            'schemas/fetchSchemas/fulfilled',
            'schemas/approveSchema/fulfilled',
            'schemas/blockSchema/fulfilled',
            'schemas/unloadSchema/fulfilled',
            'schemas/loadSchema/fulfilled',
            'persist/PERSIST'
          ],
          // Ignore these field paths in all actions
          ignoredActionsPaths: ['payload.schemas.definition'],
          // Ignore these paths in the state
          ignoredPaths: ['schemas.schemas.*.definition'],
        },
        immutableCheck: false
      })
  });
}

/**
 * Unified render function with Redux provider
 * @param {React.Component} ui - Component to render
 * @param {Object} options - Render options
 * @param {Object} options.preloadedState - Initial Redux state
 * @param {Object} options.store - Custom store instance
 * @param {Object} renderOptions - Additional render options
 * @returns {Object} Render result with store
 */
export function renderWithRedux(
  ui: ReactElement,
  {
    preloadedState = {} as Record<string, unknown>,
    store,
    extraReducers = {} as ExtraReducers,
    ...renderOptions
  }: {
    preloadedState?: Record<string, unknown>;
    store?: TestStore;
    extraReducers?: ExtraReducers;
  } & Omit<RenderOptions, 'wrapper'> = {}
) {
  const resolvedStore: TestStore = store ?? createTestStore(preloadedState, { extraReducers });

  function Wrapper({ children }: { children?: ReactNode }) {
    return <Provider store={resolvedStore}>{children}</Provider>;
  }

  return {
    ...render(ui, { wrapper: Wrapper, ...renderOptions }),
    store: resolvedStore
  };
}

/**
 * Render hook with Redux provider
 * @param {Function} hook - Hook to render
 * @param {Object} options - Render options
 * @param {Object} options.preloadedState - Initial Redux state
 * @param {Object} options.store - Custom store instance
 * @param {Object} renderOptions - Additional render options
 * @returns {Object} Render result with store
 */
export function renderHookWithRedux<TResult, TProps>(
  hook: (props: TProps) => TResult,
  {
    preloadedState = {} as Record<string, unknown>,
    store,
    ...renderOptions
  }: {
    preloadedState?: Record<string, unknown>;
    store?: TestStore;
    initialProps?: TProps;
  } = {}
) {
  const resolvedStore: TestStore = store ?? createTestStore(preloadedState);

  function Wrapper({ children }: { children?: ReactNode }) {
    return <Provider store={resolvedStore}>{children}</Provider>;
  }

  return {
    ...renderHook(hook, { wrapper: Wrapper, ...renderOptions }),
    store: resolvedStore
  };
}

/**
 * Create initial test state for schemas
 * @param {Object} overrides - State overrides
 * @returns {Object} Initial schemas state
 */
export function createTestSchemaState(
  overrides: {
    schemas?: Record<string, unknown>;
    /**
     * Convenience for tests that think in terms of "the schemas the user has
     * approved." Each entry is folded into the schemas map keyed by name and
     * tagged with `state: 'approved'` so `selectApprovedSchemas` picks it up.
     */
    approvedSchemas?: Array<{ name: string; [key: string]: unknown }>;
  } = {}
) {
  const defaultState: { schemas: Record<string, unknown> } = {
    schemas: {
      schemas: {},
      loading: { fetch: false, operations: {} },
      errors: { fetch: null, operations: {} },
      lastFetched: null,
      cache: { ttl: 300000, version: '2.1.0', lastUpdated: null },
      activeSchema: null,
    },
  };

  const schemasMap: Record<string, unknown> = {};
  if (overrides.approvedSchemas) {
    for (const s of overrides.approvedSchemas) {
      schemasMap[s.name] = { state: 'approved', ...s };
    }
  }
  if (overrides.schemas) {
    Object.assign(schemasMap, overrides.schemas);
  }
  (defaultState.schemas as { schemas: Record<string, unknown> }).schemas = schemasMap;

  return defaultState;
}

/**
 * Creates mock schema data for testing
 * Uses declarative schema format matching the backend
 * @param {Object} overrides - Properties to override in mock schema
 * @returns {Object} Mock schema object
 */
export const createMockSchema = (overrides: Record<string, unknown> = {}) => {
  const defaults = {
    name: 'test_schema',
    state: SCHEMA_STATES.APPROVED,
    fields: ['id', 'name', 'created_at'],
    schema_type: { Single: {} }
  };
  
  // Merge overrides, converting old field format to array if needed
  const merged = { ...defaults, ...overrides };
  
  // Convert fields from object to array if provided in old format
  if (merged.fields && typeof merged.fields === 'object' && !Array.isArray(merged.fields)) {
    merged.fields = Object.keys(merged.fields);
  }
  
  return merged;
};

/**
 * Creates mock range schema data for testing
 * Uses declarative schema format matching the backend
 * @param {Object} overrides - Properties to override in mock range schema
 * @returns {Object} Mock range schema object
 */
export const createMockRangeSchema = (overrides: Record<string, unknown> = {}) => {
  const defaults = {
    name: 'test_range_schema',
    state: SCHEMA_STATES.APPROVED,
    fields: ['timestamp', 'value', 'metadata'],
    key: { range_field: 'timestamp' },
    schema_type: 'Range'
  };
  
  // Merge overrides, converting old field format to array if needed
  const merged = { ...defaults, ...overrides };
  
  // Convert fields from object to array if provided in old format
  if (merged.fields && typeof merged.fields === 'object' && !Array.isArray(merged.fields)) {
    merged.fields = Object.keys(merged.fields);
  }
  
  return merged;
};

/**
 * Creates a list of mock schemas with different states for testing
 * Uses declarative schema format matching the backend
 * @param {number} count - Number of schemas to create
 * @param {Object} baseProps - Base properties for all schemas
 * @returns {Array} Array of mock schema objects
 */
export const createMockSchemaList = (count = 3, baseProps: Record<string, unknown> = {}) => {
  const states = [SCHEMA_STATES.APPROVED, SCHEMA_STATES.AVAILABLE, SCHEMA_STATES.BLOCKED];
  
  return Array.from({ length: count }, (_, index) => {
    const schema = {
      name: `schema_${index}`,
      state: states[index % states.length],
      fields: ['id', 'data'],
      schema_type: { Single: {} },
      ...baseProps
    };
    
    // Convert fields from object to array if provided in baseProps
    if (schema.fields && typeof schema.fields === 'object' && !Array.isArray(schema.fields)) {
      schema.fields = Object.keys(schema.fields);
    }
    
    return schema;
  });
};

/**
 * Creates mock authentication state for testing
 * @param {Object} overrides - Properties to override in auth state
 * @returns {Object} Mock auth state object
 */
export const createMockAuthState = (overrides: Record<string, unknown> = {}) => ({
  isAuthenticated: true,
  systemKeyId: 'mock_system_key_id',
  publicKey: 'mock_public_key',
  loading: false,
  error: null,
  ...overrides
});

/**
 * Utility to wait for async operations with timeout
 * @param {Function} condition - Function that returns true when condition is met
 * @param {number} timeout - Maximum time to wait in milliseconds
 * @param {number} interval - Polling interval in milliseconds
 * @returns {Promise} Resolves when condition is met or rejects on timeout
 */
export const waitForCondition = async (
  condition: () => boolean | Promise<boolean>,
  timeout = TEST_TIMEOUT_DEFAULT_MS,
  interval = MOCK_API_DELAY_MS
) => {
  const startTime = Date.now();
  
  while (Date.now() - startTime < timeout) {
    if (await condition()) {
      return;
    }
    await new Promise(resolve => setTimeout(resolve, interval));
  }
  
  throw new Error(`Condition not met within ${timeout}ms timeout`);
};

/**
 * Mock delay utility for simulating async operations
 * @param {number} ms - Delay in milliseconds
 * @returns {Promise} Promise that resolves after delay
 */
export const mockDelay = (ms = MOCK_API_DELAY_MS) => {
  return new Promise(resolve => setTimeout(resolve, ms));
};

/**
 * Creates a mock error object for testing error handling
 * @param {string} message - Error message
 * @param {number} status - HTTP status code
 * @param {Object} details - Additional error details
 * @returns {Error} Mock error object
 */
export const createMockError = (
  message = 'Test error',
  status = 500,
  details: Record<string, unknown> = {}
) => {
  const error = new Error(message) as Error & {
    status: number;
    details: Record<string, unknown>;
    toUserMessage: () => string;
  };
  error.status = status;
  error.details = details;
  error.toUserMessage = () => `User-friendly: ${message}`;
  return error;
};

/**
 * Validates test coverage against threshold
 * @param {Object} coverage - Coverage report object
 * @returns {boolean} True if coverage meets threshold
 */
export const validateCoverage = (coverage: Record<string, { pct?: number } | undefined>) => {
  const metrics = ['lines', 'functions', 'branches', 'statements'];

  return metrics.every(metric => {
    const percentage = coverage[metric]?.pct ?? 0;
    return percentage >= COVERAGE_THRESHOLD_PERCENT;
  });
};

/**
 * Creates a batch of test operations for integration testing
 * @param {Array} operations - Array of operation functions
 * @param {number} batchSize - Size of each batch
 * @returns {Array} Array of batched operations
 */
export const createTestBatch = <T,>(operations: T[], batchSize = TEST_VALIDATION_BATCH_SIZE): T[][] => {
  const batches = [];
  
  for (let i = 0; i < operations.length; i += batchSize) {
    batches.push(operations.slice(i, i + batchSize));
  }
  
  return batches;
};

/**
 * Mock localStorage for testing
 */
export const mockLocalStorage = (() => {
  let store: Record<string, string> = {};

  return {
    getItem: (key: string) => store[key] || null,
    setItem: (key: string, value: unknown) => { store[key] = String(value); },
    removeItem: (key: string) => { delete store[key]; },
    clear: () => { store = {}; },
    get length() { return Object.keys(store).length; },
    key: (index: number) => Object.keys(store)[index] || null
  };
})();

/**
 * Mock sessionStorage for testing
 */
export const mockSessionStorage = (() => {
  let store: Record<string, string> = {};

  return {
    getItem: (key: string) => store[key] || null,
    setItem: (key: string, value: unknown) => { store[key] = String(value); },
    removeItem: (key: string) => { delete store[key]; },
    clear: () => { store = {}; },
    get length() { return Object.keys(store).length; },
    key: (index: number) => Object.keys(store)[index] || null
  };
})();

/**
 * Custom matcher for testing schema objects
 * @param {Object} received - Received schema object
 * @param {Object} expected - Expected schema properties
 * @returns {Object} Matcher result
 */
export const toBeValidSchema = (
  received: Record<string, unknown>,
  expected: Record<string, unknown> = {}
) => {
  const requiredFields = ['name', 'state', 'fields'];
  const validStates = Object.values(SCHEMA_STATES) as string[];

  const missingFields = requiredFields.filter(field => !(field in received));
  
  if (missingFields.length > 0) {
    return {
      message: () => `Expected schema to have required fields: ${missingFields.join(', ')}`,
      pass: false
    };
  }
  
  if (!validStates.includes(received.state as string)) {
    return {
      message: () => `Expected schema state to be one of: ${validStates.join(', ')}, got: ${received.state}`,
      pass: false
    };
  }
  
  if (typeof received.fields !== 'object' || received.fields === null) {
    return {
      message: () => 'Expected schema to have valid fields object',
      pass: false
    };
  }
  
  // Check expected properties if provided
  for (const [key, value] of Object.entries(expected)) {
    if (received[key] !== value) {
      return {
        message: () => `Expected schema.${key} to be ${value}, got: ${received[key]}`,
        pass: false
      };
    }
  }
  
  return {
    message: () => 'Schema is valid',
    pass: true
  };
};

/**
 * Setup function for test environment
 * Call this in test setup files
 */
export const setupTestEnvironment = () => {
  // Mock localStorage and sessionStorage
  Object.defineProperty(window, 'localStorage', {
    value: mockLocalStorage,
    writable: true
  });
  
  Object.defineProperty(window, 'sessionStorage', {
    value: mockSessionStorage,
    writable: true
  });
  
  // Mock IntersectionObserver
  global.IntersectionObserver = class IntersectionObserver {
    root = null;
    rootMargin = '';
    thresholds: ReadonlyArray<number> = [];
    constructor() {}
    disconnect() {}
    observe() {}
    unobserve() {}
    takeRecords(): IntersectionObserverEntry[] { return []; }
  } as unknown as typeof IntersectionObserver;

  // Mock ResizeObserver
  global.ResizeObserver = class ResizeObserver {
    constructor() {}
    disconnect() {}
    observe() {}
    unobserve() {}
  };

  // Mock matchMedia
  Object.defineProperty(window, 'matchMedia', {
    writable: true,
    value: (query: string) => ({
      matches: false,
      media: query,
      onchange: null,
      addListener: () => {},
      removeListener: () => {},
      addEventListener: () => {},
      removeEventListener: () => {},
      dispatchEvent: () => {}
    })
  });
  
  // Add custom matchers
  expect.extend({
    toBeValidSchema
  });
};

/**
 * Cleanup function for test environment
 * Call this in test teardown
 */
export const cleanupTestEnvironment = () => {
  mockLocalStorage.clear();
  mockSessionStorage.clear();
  
  // Clear any timers
  if (typeof vi !== 'undefined' && vi.clearAllTimers) {
    vi.clearAllTimers();
  }
  const jestGlobal = (globalThis as Record<string, unknown>).jest as
    | { clearAllTimers?: () => void }
    | undefined;
  if (jestGlobal && typeof jestGlobal.clearAllTimers === 'function') {
    jestGlobal.clearAllTimers();
  }
};

/**
 * Mock API responses for testing
 */
export const mockApiResponses = {
  schemas: {
    available: [
      { id: 'schema1', name: 'Test Schema 1', approved: false },
      { id: 'schema2', name: 'Test Schema 2', approved: false }
    ],
    approved: [
      { id: 'schema3', name: 'Approved Schema', approved: true }
    ]
  },
  fields: {
    schema1: [
      { name: 'field1', type: 'string' },
      { name: 'field2', type: 'number' }
    ]
  }
};

/**
 * Creates an authenticated state for testing with user object shape
 * Used by component tests that check auth-dependent rendering
 * @param {Object} overrides - Properties to override
 * @returns {Object} Authenticated Redux state
 */
export const createAuthenticatedState = (
  overrides: { auth?: Record<string, unknown>; schemas?: Record<string, unknown> } = {}
) => ({
  auth: {
    isAuthenticated: true,
    user: {
      id: 'test-user-id',
      username: 'testuser',
      role: 'admin',
    },
    loading: false,
    error: null,
    ...(overrides.auth || {}),
  },
  schemas: {
    schemas: {
      'UserSchema': {
        name: 'UserSchema',
        state: 'approved',
        fields: {
          id: { field_type: 'String' },
          name: { field_type: 'String' },
          age: { field_type: 'Number' },
        },
      },
      'ProductSchema': {
        name: 'ProductSchema',
        state: 'approved',
        fields: {
          id: { field_type: 'String' },
          name: { field_type: 'String' },
          price: { field_type: 'Number' },
        },
      },
    },
    loading: false,
    error: null,
    ...(overrides.schemas || {}),
  },
});

/**
 * Creates an unauthenticated state for testing
 * @param {Object} overrides - Properties to override
 * @returns {Object} Unauthenticated Redux state
 */
export const createUnauthenticatedState = (
  overrides: { auth?: Record<string, unknown>; schemas?: Record<string, unknown> } = {}
) => ({
  auth: {
    isAuthenticated: false,
    user: null,
    loading: false,
    error: null,
    ...(overrides.auth || {}),
  },
  schemas: {
    schemas: {},
    loading: false,
    error: null,
    ...(overrides.schemas || {}),
  },
});

/**
 * Build a fully-typed EnhancedApiResponse-shaped object for mocked client
 * methods without forcing tests to import the production type or hand-fill
 * `status` / `meta`. Always cast at the call site via `vi.mocked(fn)`.
 */
export function apiOk<T>(data: T): { success: true; data: T; status: number } {
  return { success: true, data, status: 200 };
}

export function apiErr(error: string, status = 500): { success: false; error: string; status: number } {
  return { success: false, error, status };
}

// Export all utilities as default
export default {
  createTestStore,
  renderWithRedux,
  renderHookWithRedux,
  createTestSchemaState,
  createMockSchema,
  createMockRangeSchema,
  createMockSchemaList,
  createMockAuthState,
  createAuthenticatedState,
  createUnauthenticatedState,
  waitForCondition,
  mockDelay,
  createMockError,
  validateCoverage,
  createTestBatch,
  mockLocalStorage,
  mockSessionStorage,
  toBeValidSchema,
  setupTestEnvironment,
  cleanupTestEnvironment,
  mockApiResponses,
  apiOk,
  apiErr,
  SCHEMA_STATES
};