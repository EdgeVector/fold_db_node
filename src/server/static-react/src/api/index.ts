/**
 * Unified API Client - Main Export Index
 * Central export point for all API clients and utilities
 * TASK-004: API Client Standardization and Unification
 */

// ============================================================================
// CORE API CLIENT EXPORTS
// ============================================================================

export { 
  ApiClient, 
  createApiClient, 
  defaultApiClient 
} from './core/client';

export type {
  ApiResponse,
  EnhancedApiResponse,
  RequestOptions,
  ApiClientConfig,
  RequestMetrics,
  BatchRequest,
  BatchResponse,
  ApiClientInstance
} from './core/types';

export {
  ApiError,
  AuthenticationError,
  SchemaStateError,
  NetworkError,
  TimeoutError,
  ValidationError,
  RateLimitError,
  ErrorFactory,
  isApiError,
  isNetworkError,
  isTimeoutError,
  isAuthenticationError,
  isSchemaStateError,
  isRetryableError,
  isValidationError,
  isRateLimitError
} from './core/errors';

// ============================================================================
// SPECIALIZED CLIENT EXPORTS
// ============================================================================

export {
  UnifiedSchemaClient,
  createSchemaClient,
  schemaClient,
  getSchemasByState,
  getAllSchemasWithState,
  getSchemaStatus,
  getSchema,
  approveSchema,
  blockSchema,
  getApprovedSchemas,
} from './clients/schemaClient';

export {
  UnifiedMutationClient,
  createMutationClient,
  mutationClient,
  MutationClient,
  executeMutation,
  executeQuery,
  validateSchemaForMutation
} from './clients/mutationClient';

export {
  UnifiedSecurityClient,
  createSecurityClient,
  securityClient,
  getSystemPublicKey,
  validatePublicKeyFormat,
  getSecurityStatus
} from './clients/securityClient';

// ============================================================================
// CONSTANTS
// ============================================================================

export {
  API_REQUEST_TIMEOUT_MS,
  API_RETRY_ATTEMPTS,
  API_RETRY_DELAY_MS,
  API_BATCH_REQUEST_LIMIT,
  HTTP_STATUS_CODES,
  CONTENT_TYPES,
  REQUEST_HEADERS,
  ERROR_MESSAGES,
  CACHE_CONFIG,
  RETRY_CONFIG,
  API_CONFIG,
  SCHEMA_STATES,
  SCHEMA_OPERATIONS
} from '../constants/api';

export { API_ENDPOINTS } from './endpoints';

// ============================================================================
// TYPE EXPORTS
// ============================================================================

export type { Schema } from '../types/schema';
export type { VerificationResponse } from '../types/api';

