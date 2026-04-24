/**
 * Application-wide Configuration Constants
 */

/**
 * Browser compatibility and device settings
 */
export const BROWSER_CONFIG = {
  // Local storage keys (single source of truth)
  STORAGE_KEYS: {
    USER_ID: 'fold_user_id',
    USER_HASH: 'fold_user_hash',
  },
} as const;