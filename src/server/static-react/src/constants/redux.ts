/**
 * Redux Constants for Schema State Management
 * TASK-003: State Management Consolidation with Redux
 *
 * This file contains all constants required for Redux schema state management
 * as per Section 2.1.12 of .cursorrules compliance requirements.
 */

import { SCHEMA_STATES } from "./schemas";

// ============================================================================
// SCHEMA CACHE CONFIGURATION CONSTANTS
// ============================================================================

/**
 * Schema cache time-to-live in milliseconds (5 minutes)
 * Used for determining when to refetch schemas from the backend
 */
export const SCHEMA_CACHE_TTL_MS = 300000; // 5 minutes

/**
 * Number of retry attempts for schema fetch operations
 * Applied to network failures and transient errors
 */
export const SCHEMA_FETCH_RETRY_ATTEMPTS = 3;

/**
 * Timeout for individual schema operations in milliseconds
 * Applied to approve, block, and unload operations
 */
export const SCHEMA_OPERATION_TIMEOUT_MS = 10000;

// ============================================================================
// REDUX ACTION TYPE CONSTANTS
// ============================================================================

/**
 * Base action types for schema slice
 */
export const SCHEMA_ACTION_TYPES = {
  // Async thunk action types
  FETCH_SCHEMAS: "schemas/fetchSchemas",
  APPROVE_SCHEMA: "schemas/approveSchema",
  BLOCK_SCHEMA: "schemas/blockSchema",
  UNLOAD_SCHEMA: "schemas/unloadSchema",
  LOAD_SCHEMA: "schemas/loadSchema",

  // Synchronous action types
  SET_ACTIVE_SCHEMA: "schemas/setActiveSchema",
  UPDATE_SCHEMA_STATUS: "schemas/updateSchemaStatus",
  SET_LOADING: "schemas/setLoading",
  SET_ERROR: "schemas/setError",
  CLEAR_ERROR: "schemas/clearError",
  CLEAR_OPERATION_ERROR: "schemas/clearOperationError",
  INVALIDATE_CACHE: "schemas/invalidateCache",
  RESET_SCHEMAS: "schemas/resetSchemas",
} as const;

export type SchemaActionType = (typeof SCHEMA_ACTION_TYPES)[keyof typeof SCHEMA_ACTION_TYPES];

// ============================================================================
// DEFAULT STATE VALUES
// ============================================================================

export interface SchemaCacheMeta {
  ttl: number;
  version: string;
  lastUpdated: number | null;
}

export interface DefaultSchemaState {
  schemas: Record<string, unknown>;
  loading: {
    fetch: boolean;
    operations: Record<string, boolean>;
  };
  errors: {
    fetch: string | null;
    operations: Record<string, string>;
  };
  lastFetched: number | null;
  cache: SchemaCacheMeta;
  activeSchema: string | null;
}

/**
 * Complete default schema state
 */
export const DEFAULT_SCHEMA_STATE: DefaultSchemaState = {
  schemas: {},
  loading: {
    fetch: false,
    operations: {},
  },
  errors: {
    fetch: null,
    operations: {},
  },
  lastFetched: null,
  cache: {
    ttl: SCHEMA_CACHE_TTL_MS,
    version: "1.0.0",
    lastUpdated: null,
  },
  activeSchema: null,
};

// ============================================================================
// ERROR MESSAGE CONSTANTS
// ============================================================================

/**
 * Standard error messages for schema operations
 */
export const SCHEMA_ERROR_MESSAGES = {
  // Network and API errors
  FETCH_FAILED: "Failed to fetch schemas from server",
  NETWORK_ERROR: "Network error occurred while fetching schemas",
  API_TIMEOUT: "Request timed out - please try again",
  UNAUTHORIZED: "Not authorized to perform this operation",

  // Schema operation errors
  APPROVE_FAILED: "Failed to approve schema",
  BLOCK_FAILED: "Failed to block schema",
  UNLOAD_FAILED: "Failed to unload schema",
  LOAD_FAILED: "Failed to load schema",

  // Validation errors
  SCHEMA_NOT_FOUND: "Schema not found",
  INVALID_SCHEMA_STATE: "Invalid schema state for operation",
  SCHEMA_ALREADY_APPROVED: "Schema is already approved",
  SCHEMA_ALREADY_BLOCKED: "Schema is already blocked",

  // Cache and persistence errors
  CACHE_INVALIDATION_FAILED: "Failed to invalidate schema cache",
  PERSISTENCE_ERROR: "Failed to persist schema state",

  // General errors
  UNKNOWN_ERROR: "An unknown error occurred",
  OPERATION_CANCELLED: "Operation was cancelled by user",
} as const;

// ============================================================================
// SCHEMA STATES AND VALIDATION CONSTANTS
// ============================================================================

export { SCHEMA_STATES };

/**
 * Schema operations that require specific states
 */
export const SCHEMA_OPERATION_REQUIREMENTS: Record<string, string[]> = {
  [SCHEMA_ACTION_TYPES.APPROVE_SCHEMA]: [
    SCHEMA_STATES.AVAILABLE,
    SCHEMA_STATES.BLOCKED,
  ],
  [SCHEMA_ACTION_TYPES.BLOCK_SCHEMA]: [
    SCHEMA_STATES.AVAILABLE,
    SCHEMA_STATES.APPROVED,
  ],
  [SCHEMA_ACTION_TYPES.UNLOAD_SCHEMA]: [
    SCHEMA_STATES.APPROVED,
    SCHEMA_STATES.BLOCKED,
  ],
};

/**
 * Schema states that allow read operations (SCHEMA-002 compliance)
 */
export const READABLE_SCHEMA_STATES: string[] = [SCHEMA_STATES.APPROVED];
