/**
 * Schema-related constants
 */

// Schema state constants
export const SCHEMA_STATES = {
  AVAILABLE: 'available',
  APPROVED: 'approved',
  BLOCKED: 'blocked',
  LOADING: 'loading',
  ERROR: 'error',
} as const;

export type SchemaState = (typeof SCHEMA_STATES)[keyof typeof SCHEMA_STATES];

// Range schema constants
export const RANGE_SCHEMA_CONFIG = {
  FIELD_TYPE: 'Range',
  MUTATION_WRAPPER_KEY: 'value',
} as const;