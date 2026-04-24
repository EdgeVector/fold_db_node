/**
 * Range Schema Utilities - Consolidated Implementation
 * TASK-008: Duplicate Code Detection and Elimination
 *
 * This module consolidates range schema utilities that were duplicated across
 * useRangeSchema.js and rangeSchemaUtils.js, providing a single source of truth
 * for range schema detection, validation, and formatting operations.
 *
 * Range schemas are specialized schemas designed for time-series and ordered data
 * with the following characteristics:
 * - Contains a designated range_key field for ordering
 * - All fields have field_type: "Range"
 * - Non-range_key fields are wrapped in objects for backend processing
 * - Supports efficient range-based queries and mutations
 */

import { RANGE_SCHEMA_CONFIG } from '../constants/schemas';
import { MUTATION_TYPE_API_MAP } from '../constants/ui';

export type SchemaType = 'Single' | 'Range' | 'HashRange';

export interface Schema {
  name?: string;
  fields?: unknown;
  schema_type?: SchemaType | string;
  range_key?: string;
  key?: {
    hash_field?: string;
    range_field?: string;
  };
}

export interface RangeSchemaInfo {
  isRangeSchema: true;
  rangeKey: string | null;
  rangeFields: Array<[string, unknown]>;
  nonRangeKeyFields: Record<string, Record<string, unknown>>;
  totalFields: number;
}

export interface HashRangeSchemaInfo {
  isHashRangeSchema: true;
  hashField: string | null;
  rangeField: string | null;
  totalFields: number;
}

export interface RangeMutation {
  type: 'mutation';
  schema: string | undefined;
  mutation_type: string;
  fields_and_values?: Record<string, unknown>;
  key_value?: { hash: string | null; range: string | null };
}

/**
 * Gets the schema type from a schema object.
 * Handles the simple string format from Rust: "Single" | "Range" | "HashRange"
 */
function getSchemaType(schema: Schema | null | undefined): SchemaType | null {
  if (!schema || typeof schema !== 'object') return null;

  const schemaType = schema.schema_type;

  if (schemaType === 'Single') return 'Single';
  if (schemaType === 'Range') return 'Range';
  if (schemaType === 'HashRange') return 'HashRange';

  return null;
}

/**
 * Detects if a schema is a HashRange schema.
 * Backend is authoritative — no front-end validation.
 */
export function isHashRangeSchema(schema: Schema | null | undefined): boolean {
  if (!schema || typeof schema !== 'object') {
    return false;
  }
  return getSchemaType(schema) === 'HashRange';
}

/** Gets the range field expression for a HashRange schema. */
export function getRangeField(schema: Schema | null | undefined): string | null {
  if (!schema || !isHashRangeSchema(schema)) return null;
  return schema.key?.range_field || null;
}

/** Returns the last segment of a dotted field expression. */
function lastSegment(expr: unknown): string | null {
  if (typeof expr !== 'string') return null;
  const parts = expr.split('.');
  return parts[parts.length - 1] || expr;
}

/**
 * Detects if a schema is a range schema.
 * Backend is authoritative — no front-end validation.
 */
export function isRangeSchema(schema: Schema | null | undefined): boolean {
  if (!schema || typeof schema !== 'object') {
    return false;
  }
  return getSchemaType(schema) === 'Range';
}

/** Gets the range key field name for a range schema. */
export function getRangeKey(schema: Schema | null | undefined): string | null {
  if (!schema || typeof schema !== 'object') return null;
  const rf = schema?.key?.range_field;
  return (typeof rf === 'string' && rf.trim()) ? lastSegment(rf) : null;
}

/**
 * Gets the hash key field name for any schema type (Single, Range, HashRange).
 * Returns the last segment of the expression if dotted, else the raw field.
 */
export function getHashKey(schema: Schema | null | undefined): string | null {
  if (!schema || typeof schema !== 'object') return null;
  const hf = schema?.key?.hash_field;
  return hf && typeof hf === 'string' && hf.trim() ? lastSegment(hf) : null;
}

/**
 * Enhanced range schema mutation formatter with better validation.
 * Range schemas require non-range_key fields to be JSON objects.
 */
export function formatRangeMutation(
  schema: Schema,
  mutationType: string,
  rangeKeyValue: string,
  fieldData: Record<string, unknown>,
): RangeMutation {
  const normalizedMutationType = typeof mutationType === 'string'
    ? ((MUTATION_TYPE_API_MAP as Record<string, string>)[mutationType] || mutationType.toLowerCase())
    : '';
  const mutation: RangeMutation = {
    type: 'mutation',
    schema: schema.name,
    mutation_type: normalizedMutationType,
  };

  const rangeKeyFieldName = getRangeKey(schema);

  const fieldsAndValues: Record<string, unknown> = {};

  if (rangeKeyValue && rangeKeyValue.trim() && rangeKeyFieldName) {
    fieldsAndValues[rangeKeyFieldName] = rangeKeyValue.trim();
  }

  Object.entries(fieldData).forEach(([fieldName, fieldValue]) => {
    if (fieldName !== rangeKeyFieldName) {
      const wrapperKey = (RANGE_SCHEMA_CONFIG as { MUTATION_WRAPPER_KEY?: string }).MUTATION_WRAPPER_KEY || 'value';

      if (typeof fieldValue === 'string' || typeof fieldValue === 'number' || typeof fieldValue === 'boolean') {
        fieldsAndValues[fieldName] = { [wrapperKey]: fieldValue };
      } else if (typeof fieldValue === 'object' && fieldValue !== null) {
        fieldsAndValues[fieldName] = fieldValue;
      } else {
        fieldsAndValues[fieldName] = { [wrapperKey]: fieldValue };
      }
    }
  });

  mutation.fields_and_values = fieldsAndValues;
  mutation.key_value = {
    hash: null,
    range: rangeKeyValue && rangeKeyValue.trim() ? rangeKeyValue.trim() : null,
  };

  return mutation;
}

function getNonRangeKeyFields(schema: Schema): Record<string, Record<string, unknown>> {
  if (!isRangeSchema(schema)) return {};
  const rangeKey = getRangeKey(schema);
  if (!Array.isArray(schema.fields)) {
    throw new Error(`Expected schema.fields to be an array for range schema "${schema.name}", got ${typeof schema.fields}`);
  }
  return (schema.fields as string[]).reduce<Record<string, Record<string, unknown>>>((acc, fieldName) => {
    if (fieldName !== rangeKey) acc[fieldName] = {};
    return acc;
  }, {});
}

/** Gets comprehensive range schema display information. */
export function getRangeSchemaInfo(schema: Schema | null | undefined): RangeSchemaInfo | null {
  if (!isRangeSchema(schema)) {
    return null;
  }

  const s = schema as Schema;
  return {
    isRangeSchema: true,
    rangeKey: getRangeKey(s),
    rangeFields: [],
    nonRangeKeyFields: getNonRangeKeyFields(s),
    totalFields: Array.isArray(s.fields) ? s.fields.length : 0,
  };
}

/**
 * Normalizes schema state to lowercase string for consistent comparison.
 * Addresses duplication in schema state checking across multiple files.
 */
export function normalizeSchemaState(state: unknown): string {
  if (typeof state === 'string') return state.toLowerCase();
  if (typeof state === 'object' && state !== null) {
    const s = state as { state?: unknown };
    if (s.state) {
      return String(s.state).toLowerCase();
    }
    return String(state).toLowerCase();
  }
  return String(state || '').toLowerCase();
}

/**
 * Checks if a value is considered empty.
 * Minimal check — backend handles detailed validation.
 */
export function isValueEmpty(value: unknown): boolean {
  return value === null || value === undefined;
}

/** Gets HashRange schema display information. */
export function getHashRangeSchemaInfo(schema: Schema | null | undefined): HashRangeSchemaInfo | null {
  if (!isHashRangeSchema(schema)) {
    return null;
  }

  const s = schema as Schema;
  return {
    isHashRangeSchema: true,
    hashField: getHashKey(s),
    rangeField: getRangeKey(s),
    totalFields: Array.isArray(s.fields) ? s.fields.length : 0,
  };
}
