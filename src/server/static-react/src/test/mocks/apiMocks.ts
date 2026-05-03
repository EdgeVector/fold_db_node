/**
 * @fileoverview API Mocking Utilities for Testing
 *
 * Provides comprehensive API mocking utilities using MSW (Mock Service Worker)
 * for testing the unified API client system. Includes handlers for all major
 * API endpoints with realistic response patterns and error scenarios.
 *
 * TASK-006: Testing Enhancement - Created API mocking utilities
 *
 * @module apiMocks
 * @since 2.0.0
 */

import { http, HttpResponse, type DefaultBodyType, type PathParams, type RequestHandler } from 'msw';
import { setupServer, type SetupServerApi } from 'msw/node';
import { afterAll, afterEach, beforeAll, vi } from 'vitest';
import {
  createMockRangeSchema,
  createMockSchema,
  mockDelay,
  SCHEMA_STATES,
} from '../utils/testUtilities';
import { MOCK_API_DELAY_MS, TEST_TIMEOUT_DEFAULT_MS } from '../config/constants';

// ============================================================================
// MOCK DATA
// ============================================================================

type MockSchema = {
  name: string;
  state: string;
  fields: string[] | Record<string, unknown>;
  schema_type?: unknown;
  [key: string]: unknown;
};
type MockSchemaMap = Record<string, MockSchema>;

/**
 * Mock schemas for testing
 */
export const mockSchemas: MockSchemaMap = {
  user_profiles: createMockSchema({
    name: 'user_profiles',
    descriptive_name: 'User Profiles',
    state: SCHEMA_STATES.APPROVED,
    fields: {
      id: { field_type: 'String' },
      name: { field_type: 'String' },
      email: { field_type: 'String' },
      age: { field_type: 'Number' },
    },
  }),
  time_series: createMockRangeSchema({
    name: 'time_series',
    descriptive_name: 'Time Series',
    state: SCHEMA_STATES.APPROVED,
  }),
  events: createMockSchema({
    name: 'events',
    descriptive_name: 'Events',
    state: SCHEMA_STATES.AVAILABLE,
    fields: {
      event_id: { field_type: 'String' },
      event_type: { field_type: 'String' },
      timestamp: { field_type: 'String' },
    },
  }),
  blocked_schema: createMockSchema({
    name: 'blocked_schema',
    descriptive_name: 'Blocked Schema',
    state: SCHEMA_STATES.BLOCKED,
    fields: {
      id: { field_type: 'String' },
      data: { field_type: 'String' },
    },
  }),
};

/**
 * Mock persisted schema states
 */
export const mockPersistedStates: Record<string, string> = {
  user_profiles: SCHEMA_STATES.APPROVED,
  time_series: SCHEMA_STATES.APPROVED,
  events: SCHEMA_STATES.AVAILABLE,
  blocked_schema: SCHEMA_STATES.BLOCKED,
};

/**
 * Mock available schema names
 */
export const mockAvailableSchemas: string[] = Object.keys(mockSchemas);

/**
 * Mock authentication data
 */
export const mockAuthData = {
  systemPublicKey: 'mock_system_public_key_123',
  isValid: true,
  keyId: 'system_key_001',
};

/**
 * Mock mutation results keyed by mutation type
 */
export const mockMutationResults: Record<string, { success: boolean; data: Record<string, unknown> }> = {
  create: {
    success: true,
    data: { id: 'created_item_123', status: 'created' },
  },
  update: {
    success: true,
    data: { id: 'updated_item_456', status: 'updated' },
  },
  delete: {
    success: true,
    data: { id: 'deleted_item_789', status: 'deleted' },
  },
};

/**
 * Mock query results
 */
export const mockQueryResults = {
  basic: {
    success: true,
    data: [
      { id: '1', name: 'John Doe', email: 'john@example.com' },
      { id: '2', name: 'Jane Smith', email: 'jane@example.com' },
    ],
  },
  range: {
    success: true,
    data: [
      { timestamp: '2025-01-01T00:00:00Z', value: 100 },
      { timestamp: '2025-01-01T01:00:00Z', value: 105 },
    ],
  },
  empty: {
    success: true,
    data: [],
  },
};

// ============================================================================
// MSW HANDLERS
// ============================================================================

const notFoundResponse = (schemaName: string) =>
  HttpResponse.json(
    { success: false, error: { message: `Schema ${schemaName} not found` } },
    { status: 404 }
  );

/**
 * Default MSW request handlers for API endpoints
 */
export const defaultHandlers: RequestHandler[] = [
  http.post('/api/security/verify', async () => {
    await mockDelay(MOCK_API_DELAY_MS);
    return HttpResponse.json({
      success: true,
      data: { verified: true, message: 'Verification successful' },
    });
  }),

  http.get('/api/security/system-public-key', async () => {
    await mockDelay(MOCK_API_DELAY_MS);
    return HttpResponse.json({
      success: true,
      data: {
        publicKey: 'mock-system-public-key-base64',
        keyId: 'system-key-1',
        algorithm: 'Ed25519',
      },
    });
  }),

  http.post('/api/validate', async () => {
    await mockDelay(MOCK_API_DELAY_MS);
    return HttpResponse.json({
      success: true,
      data: { valid: true, errors: [] },
    });
  }),

  http.get('/api/schemas/available', async () => {
    await mockDelay(MOCK_API_DELAY_MS);
    return HttpResponse.json({
      success: true,
      data: mockAvailableSchemas,
    });
  }),

  http.get('/api/schemas', async () => {
    await mockDelay(MOCK_API_DELAY_MS);
    return HttpResponse.json({
      success: true,
      data: Object.values(mockSchemas),
    });
  }),

  http.get('/schemas', async () => {
    await mockDelay(MOCK_API_DELAY_MS);
    return HttpResponse.json({
      success: true,
      data: Object.values(mockSchemas),
    });
  }),

  http.get<{ schemaName: string }>('/schema/:schemaName', async ({ params }) => {
    const { schemaName } = params;
    await mockDelay(MOCK_API_DELAY_MS);

    const schema = mockSchemas[schemaName];
    if (!schema) {
      return notFoundResponse(schemaName);
    }

    return HttpResponse.json({ success: true, data: schema });
  }),

  http.get<{ schemaName: string }>('/api/schema/:schemaName', async ({ params }) => {
    const { schemaName } = params;
    await mockDelay(MOCK_API_DELAY_MS);

    const schema = mockSchemas[schemaName];
    if (!schema) {
      return notFoundResponse(schemaName);
    }

    return HttpResponse.json({ success: true, data: schema, ...schema });
  }),

  http.post<{ schemaName: string }>('/api/schema/:schemaName/approve', async ({ params }) => {
    const { schemaName } = params;
    await mockDelay(MOCK_API_DELAY_MS);

    const schema = mockSchemas[schemaName];
    if (!schema) {
      return notFoundResponse(schemaName);
    }

    if (schema.state !== SCHEMA_STATES.AVAILABLE) {
      return HttpResponse.json(
        {
          success: false,
          error: { message: `Schema ${schemaName} cannot be approved from state ${schema.state}` },
        },
        { status: 400 }
      );
    }

    return HttpResponse.json({
      success: true,
      data: { schema: { ...schema, state: SCHEMA_STATES.APPROVED } },
    });
  }),

  http.post<{ schemaName: string }>('/api/schema/:schemaName/block', async ({ params }) => {
    const { schemaName } = params;
    await mockDelay(MOCK_API_DELAY_MS);

    const schema = mockSchemas[schemaName];
    if (!schema) {
      return notFoundResponse(schemaName);
    }

    return HttpResponse.json({
      success: true,
      data: { schema: { ...schema, state: SCHEMA_STATES.BLOCKED } },
    });
  }),

  http.post<{ schemaName: string }>('/api/schema/:schemaName/load', async ({ params }) => {
    const { schemaName } = params;
    await mockDelay(MOCK_API_DELAY_MS);

    const schema = mockSchemas[schemaName];
    if (!schema) {
      return notFoundResponse(schemaName);
    }

    return HttpResponse.json({
      success: true,
      data: { schema: { ...schema, state: SCHEMA_STATES.APPROVED } },
    });
  }),

  http.post<{ schemaName: string }>('/api/schema/:schemaName/unload', async ({ params }) => {
    const { schemaName } = params;
    await mockDelay(MOCK_API_DELAY_MS);

    const schema = mockSchemas[schemaName];
    if (!schema) {
      return notFoundResponse(schemaName);
    }

    if (schema.state === SCHEMA_STATES.APPROVED) {
      return HttpResponse.json(
        {
          success: false,
          error: { message: `Cannot unload approved schema ${schemaName}` },
        },
        { status: 400 }
      );
    }

    return HttpResponse.json({
      success: true,
      data: { schema: { ...schema, state: SCHEMA_STATES.AVAILABLE } },
    });
  }),

  http.post('/api/mutation', async ({ request }) => {
    const body = (await request.json()) as {
      schema?: string;
      mutation_type?: string;
      data?: unknown;
    };
    await mockDelay(MOCK_API_DELAY_MS);

    if (!body.schema || !body.mutation_type || !body.data) {
      return HttpResponse.json(
        { success: false, error: { message: 'Invalid mutation structure' } },
        { status: 400 }
      );
    }

    const schema = mockSchemas[body.schema];
    if (!schema || schema.state !== SCHEMA_STATES.APPROVED) {
      return HttpResponse.json(
        {
          success: false,
          error: { message: 'Only approved schemas can be used for mutations' },
        },
        { status: 403 }
      );
    }

    const mutationType = body.mutation_type.toLowerCase();
    const result = mockMutationResults[mutationType] ?? mockMutationResults.create;

    return HttpResponse.json(result);
  }),

  http.post('/api/query', async ({ request }) => {
    const body = (await request.json()) as { schema?: string; fields?: unknown };
    await mockDelay(MOCK_API_DELAY_MS);

    if (!body.schema || !body.fields) {
      return HttpResponse.json(
        { success: false, error: { message: 'Invalid query structure' } },
        { status: 400 }
      );
    }

    const schema = mockSchemas[body.schema];
    if (!schema || schema.state !== SCHEMA_STATES.APPROVED) {
      return HttpResponse.json(
        {
          success: false,
          error: { message: 'Only approved schemas can be used for queries' },
        },
        { status: 403 }
      );
    }

    const isRangeSchema = Boolean(
      (schema.schema_type as { Range?: unknown } | undefined)?.Range
    );
    const result = isRangeSchema ? mockQueryResults.range : mockQueryResults.basic;

    return HttpResponse.json(result);
  }),

  http.get('/api/native-index/search', async ({ request }) => {
    const url = new URL(request.url);
    const term = url.searchParams.get('term') ?? '';
    await mockDelay(MOCK_API_DELAY_MS);
    const sample = [
      {
        schema_name: 'user_profiles',
        field: 'name',
        key_value: { hash: null, range: 'user-1' },
        value: 'John Doe',
        metadata: { word: term.toLowerCase() },
      },
      {
        schema_name: 'time_series',
        field: 'value',
        key_value: { hash: 'metrics', range: '2025-01-01T00:00:00Z' },
        value: 100,
        metadata: { word: term.toLowerCase() },
      },
    ];
    return HttpResponse.json(sample);
  }),

  http.post('/api/security/register-key', async ({ request }) => {
    const body = (await request.json()) as { publicKey?: string; signature?: string };
    await mockDelay(MOCK_API_DELAY_MS);

    if (!body.publicKey || !body.signature) {
      return HttpResponse.json(
        { success: false, error: { message: 'Missing required fields' } },
        { status: 400 }
      );
    }

    return HttpResponse.json({
      success: true,
      data: { keyId: `user_key_${Date.now()}`, status: 'registered' },
    });
  }),

  http.post('/api/keys', async ({ request }) => {
    const body = (await request.json()) as Record<string, unknown>;
    await mockDelay(MOCK_API_DELAY_MS);

    if (!body.publicKey) {
      return HttpResponse.json(
        { success: false, error: { message: 'Missing required field: publicKey' } },
        { status: 400 }
      );
    }

    return HttpResponse.json({
      success: true,
      data: {
        id: `key_${Date.now()}_${Math.random().toString(36).slice(2, 11)}`,
        ...body,
        status: 'active',
        createdAt: new Date().toISOString(),
      },
    });
  }),

  http.get('/api/keys', async () => {
    await mockDelay(MOCK_API_DELAY_MS);
    return HttpResponse.json({
      success: true,
      data: [
        {
          id: 'key_1',
          publicKey: 'mock_public_key_1',
          status: 'active',
          algorithm: 'Ed25519',
          createdAt: new Date().toISOString(),
        },
      ],
    });
  }),

  http.patch<{ keyId: string }>('/api/keys/:keyId', async ({ request, params }) => {
    const body = (await request.json()) as Record<string, unknown>;
    await mockDelay(MOCK_API_DELAY_MS);

    return HttpResponse.json({
      success: true,
      data: {
        id: params.keyId,
        ...body,
        updatedAt: new Date().toISOString(),
      },
    });
  }),
];

// ============================================================================
// ERROR SCENARIOS
// ============================================================================

const serverError = (message: string) =>
  HttpResponse.json({ success: false, error: { message } }, { status: 500 });

/**
 * Handlers for testing error scenarios.
 */
export const errorHandlers: Record<string, RequestHandler[]> = {
  networkError: [
    http.get('/api/schemas/available', () => HttpResponse.error()),
    http.get('/schemas', () => HttpResponse.error()),
    http.get('/schema/:schemaName', () => HttpResponse.error()),
    http.get('/schemas/state/:state', () => HttpResponse.error()),
    http.get('/api/keys', () => HttpResponse.error()),
    http.patch('/api/keys/:keyId', () => HttpResponse.error()),
  ],

  serverError: [
    http.get('/api/schemas/available', () => serverError('Internal server error')),
    http.get('/schemas', () => serverError('Internal server error')),
    http.get('/schema/:schemaName', () => serverError('Failed to fetch schema')),
    http.get('/schemas/state/:state', () => serverError('Failed to fetch schemas by state')),
    http.get('/api/keys', () => serverError('Internal server error')),
    http.post('/api/keys', () => serverError('Failed to store key')),
    http.patch('/api/keys/:keyId', () => serverError('Failed to update key')),
  ],

  timeout: [
    http.get('/api/schemas/available', async () => {
      await mockDelay(TEST_TIMEOUT_DEFAULT_MS + 1000);
      return HttpResponse.json({ success: false });
    }),
  ],

  unauthorized: [
    http.post('/api/mutation', () =>
      HttpResponse.json(
        { success: false, error: { message: 'Unauthorized' } },
        { status: 401 }
      )
    ),
  ],

  schemaNotApproved: [
    http.post('/api/mutation', () =>
      HttpResponse.json(
        { success: false, error: { message: 'Schema not approved for mutations' } },
        { status: 403 }
      )
    ),
  ],
};

// ============================================================================
// TEST SERVER SETUP
// ============================================================================

/**
 * Creates a configured MSW server for testing
 */
export const createMockServer = (customHandlers: RequestHandler[] = []): SetupServerApi => {
  const handlers = [...defaultHandlers, ...customHandlers];
  return setupServer(...handlers);
};

/**
 * Default mock server instance
 */
export const mockServer: SetupServerApi = createMockServer();

// ============================================================================
// MOCK API CLIENT
// ============================================================================

/**
 * Creates a mock API client for testing without network calls
 */
export const createMockApiClient = (overrides: Record<string, unknown> = {}) => ({
  get: vi.fn().mockResolvedValue({ success: true, data: {} }),
  post: vi.fn().mockResolvedValue({ success: true, data: {} }),
  put: vi.fn().mockResolvedValue({ success: true, data: {} }),
  delete: vi.fn().mockResolvedValue({ success: true, data: {} }),
  patch: vi.fn().mockResolvedValue({ success: true, data: {} }),
  batch: vi.fn().mockResolvedValue([]),
  getMetrics: vi.fn().mockReturnValue({
    averageResponseTime: 100,
    cacheHitRate: 0.8,
    totalRequests: 50,
  }),
  getCacheStats: vi.fn().mockReturnValue({
    size: 10,
    hitRate: 0.8,
  }),
  clearCache: vi.fn(),
  ...overrides,
});

/**
 * Creates mock specialized API clients
 */
export const createMockSpecializedClients = () => ({
  schema: {
    getSchemas: vi.fn().mockResolvedValue(Object.values(mockSchemas)),
    getApprovedSchemas: vi
      .fn()
      .mockResolvedValue(
        Object.values(mockSchemas).filter((s) => s.state === SCHEMA_STATES.APPROVED)
      ),
    getSchema: vi.fn().mockImplementation((name: string) =>
      Promise.resolve(mockSchemas[name] ?? null)
    ),
    approveSchema: vi.fn().mockResolvedValue({
      success: true,
      data: { schema: { state: SCHEMA_STATES.APPROVED } },
    }),
    blockSchema: vi.fn().mockResolvedValue({
      success: true,
      data: { schema: { state: SCHEMA_STATES.BLOCKED } },
    }),
  },

  mutation: {
    executeMutation: vi.fn().mockResolvedValue(mockMutationResults.create),
    validateMutation: vi.fn().mockReturnValue(null),
    validateSchemaForMutation: vi.fn().mockReturnValue(true),
  },

  query: {
    executeQuery: vi.fn().mockResolvedValue(mockQueryResults.basic),
  },

  security: {
    getSystemPublicKey: vi.fn().mockResolvedValue(mockAuthData),
  },
});

// ============================================================================
// UTILITY FUNCTIONS
// ============================================================================

/**
 * Sets up MSW server for tests. Call this in test setup.
 */
export const setupMockServer = (): void => {
  beforeAll(() => mockServer.listen({ onUnhandledRequest: 'error' }));
  afterEach(() => mockServer.resetHandlers());
  afterAll(() => mockServer.close());
};

/**
 * Temporarily override handlers for specific tests
 */
export const withMockHandlers = async (
  handlers: RequestHandler[],
  testFn: () => Promise<void> | void
): Promise<void> => {
  mockServer.use(...handlers);
  try {
    await testFn();
  } finally {
    mockServer.resetHandlers();
  }
};

// `PathParams` and `DefaultBodyType` re-exports keep prior consumers compiling;
// the wide MSW v2 generic surface is intentionally left available for callers
// that compose handlers outside this module.
export type { DefaultBodyType, PathParams };

// Export everything
export default {
  mockSchemas,
  mockPersistedStates,
  mockAvailableSchemas,
  mockAuthData,
  mockMutationResults,
  mockQueryResults,
  defaultHandlers,
  errorHandlers,
  createMockServer,
  mockServer,
  createMockApiClient,
  createMockSpecializedClients,
  setupMockServer,
  withMockHandlers,
};
