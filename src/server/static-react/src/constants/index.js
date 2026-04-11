/**
 * Centralized Constants Index
 * TASK-005: Constants Extraction and Configuration Centralization
 *
 * This file provides a centralized export of all constants for easy importing
 * and maintains an organized namespace structure for different constant categories.
 *
 * Usage Examples:
 *
 * // Import specific constant categories
 * import { APP_CONFIG, VALIDATION_RULES } from '@/constants';
 *
 * // Import all constants as namespaced object
 * import Constants from '@/constants';
 * const tabId = Constants.APP_CONFIG.DEFAULT_TAB;
 *
 * // Import specific constants directly
 * import { DEFAULT_TAB, SCHEMA_STATES } from '@/constants';
 */

// ============================================================================
// CONFIGURATION EXPORTS
// ============================================================================

export {
  APP_CONFIG,
  BROWSER_CONFIG,
} from "./config.js";

// ============================================================================
// VALIDATION EXPORTS
// ============================================================================

export {
  VALIDATION_RULES,
  VALIDATION_PATTERNS,
  VALIDATION_MESSAGES,
  SUCCESS_MESSAGES,
  VALIDATION_CONFIG,
} from "./validation.js";

// ============================================================================
// API EXPORTS (from existing files)
// ============================================================================

export {
  API_REQUEST_TIMEOUT_MS,
  API_RETRY_ATTEMPTS,
  API_RETRY_DELAY_MS,
  API_BATCH_REQUEST_LIMIT,
  HTTP_STATUS_CODES,
  CONTENT_TYPES,
  REQUEST_HEADERS,
  ERROR_MESSAGES as API_ERROR_MESSAGES,
  CACHE_CONFIG,
  RETRY_CONFIG,
  API_CONFIG,
  SCHEMA_STATES as API_SCHEMA_STATES,
  SCHEMA_OPERATIONS,
} from "./api";

// ============================================================================
// SCHEMA EXPORTS (from existing files)
// ============================================================================

export {
  SCHEMA_FETCH_RETRY_COUNT,
  SCHEMA_CACHE_DURATION_MS,
  FORM_VALIDATION_DEBOUNCE_MS,
  RANGE_SCHEMA_FIELD_PREFIX,
  SCHEMA_STATES,
  SCHEMA_API_ENDPOINTS,
  RANGE_SCHEMA_CONFIG,
  FIELD_TYPES,
} from "./schemas";

// ============================================================================
// CONVENIENCE EXPORTS - FREQUENTLY USED CONSTANTS
// ============================================================================

export const DEFAULT_TAB = "agent";
