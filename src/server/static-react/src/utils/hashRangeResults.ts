/**
 * Utilities for detecting and working with hash->range->fields shaped results.
 *
 * Expected shape:
 * {
 *   [hashKey]: {
 *     [rangeKey]: { fieldName: value, ... }
 *   },
 *   ...
 * }
 */

export type FieldMap = Record<string, unknown>;
export type RangeMap = Record<string, FieldMap>;
export type HashRangeData = Record<string, RangeMap>;

export interface QueryRecord {
  fields?: FieldMap;
  key?: { hash?: string; range?: string };
  metadata?: unknown;
}

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value);
}

/**
 * Detect whether the provided data matches the hash->range->fields shape.
 * Performs a shallow, sample-based validation to keep it fast for large datasets.
 */
export function isHashRangeFieldsShape(maybeData: unknown): boolean {
  const data = extractData(maybeData);
  if (!isPlainObject(data)) return false;

  // Sample up to 3 hash entries
  const hashKeys = Object.keys(data);
  if (hashKeys.length === 0) return false;

  for (let i = 0; i < Math.min(3, hashKeys.length); i++) {
    const hashVal = data[hashKeys[i]];
    if (!isPlainObject(hashVal)) return false;

    const rangeKeys = Object.keys(hashVal);
    if (rangeKeys.length === 0) {
      // Allow empty hash buckets
      continue;
    }

    for (let j = 0; j < Math.min(3, rangeKeys.length); j++) {
      const fieldsVal = hashVal[rangeKeys[j]];
      if (!isPlainObject(fieldsVal)) return false;
      const fieldNames = Object.keys(fieldsVal);
      if (fieldNames.length === 0) {
        continue;
      }
    }
  }

  return true;
}

/**
 * Transform an array of query results [{fields, key: {hash, range}, metadata}]
 * into the nested hash->range->fields shape expected by the structured view.
 */
function transformQueryResultsArray(results: QueryRecord[]): HashRangeData {
  const nested: HashRangeData = {};
  for (const record of results) {
    const hash = record.key?.hash || '_default';
    const range = record.key?.range || '_default';
    if (!nested[hash]) nested[hash] = {};
    nested[hash][range] = record.fields || {};
  }
  return nested;
}

/**
 * Extracts the underlying data object whether caller passed results or results.data.
 * Also handles the array-of-records format returned by queries by transforming it
 * into the nested hash->range->fields shape.
 */
export function extractData(maybeData: unknown): unknown {
  let data: unknown = maybeData;
  if (data && isPlainObject(data) && Object.prototype.hasOwnProperty.call(data, 'data')) {
    data = (data as Record<string, unknown>).data;
  }
  if (Array.isArray(data) && data.length > 0 && data[0] && isPlainObject(data[0]) && ('key' in data[0] || 'fields' in data[0])) {
    return transformQueryResultsArray(data as QueryRecord[]);
  }
  return data;
}

/** Counts the total number of hashes and ranges across the dataset. */
export function summarizeCounts(maybeData: unknown): { hashes: number; ranges: number } {
  const data = extractData(maybeData) || {};
  if (!isPlainObject(data)) return { hashes: 0, ranges: 0 };
  const hashes = Object.keys(data).length;
  let ranges = 0;
  for (const hashKey of Object.keys(data)) {
    const rangeObj = data[hashKey];
    if (isPlainObject(rangeObj)) {
      ranges += Object.keys(rangeObj).length;
    }
  }
  return { hashes, ranges };
}

/** Returns a sorted list of hash keys for stable rendering. */
export function getSortedHashKeys(maybeData: unknown): string[] {
  const data = extractData(maybeData) || {};
  if (!isPlainObject(data)) return [];
  return Object.keys(data).sort(naturalComparator);
}

/** Returns a sorted list of range keys under a given hash key. */
export function getSortedRangeKeys(maybeData: unknown, hashKey: string): string[] {
  const data = extractData(maybeData) || {};
  const ranges = isPlainObject(data) && isPlainObject(data[hashKey])
    ? (data[hashKey] as Record<string, unknown>)
    : {};
  return Object.keys(ranges).sort(naturalComparator);
}

/** Natural-ish comparator for stable ordering of mixed numeric/string keys. */
function naturalComparator(a: string, b: string): number {
  const an = toNumberOrNaN(a);
  const bn = toNumberOrNaN(b);
  if (!Number.isNaN(an) && !Number.isNaN(bn)) {
    return an - bn;
  }
  return String(a).localeCompare(String(b));
}

function toNumberOrNaN(v: unknown): number {
  const n = Number(v);
  return Number.isFinite(n) ? n : Number.NaN;
}

/** Safely get fields object at given hash and range. */
export function getFieldsAt(maybeData: unknown, hashKey: string, rangeKey: string): FieldMap | null {
  const data = extractData(maybeData) || {};
  if (!isPlainObject(data)) return null;
  const rangeObj = data[hashKey];
  if (!isPlainObject(rangeObj)) return null;
  const fields = (rangeObj as Record<string, unknown>)[rangeKey];
  return isPlainObject(fields) ? (fields as FieldMap) : null;
}

/** Slice helpers for lazy rendering. */
export function sliceKeys<T>(keys: T[], start: number, count: number): T[] {
  return keys.slice(start, Math.min(start + count, keys.length));
}

export default {
  isHashRangeFieldsShape,
  extractData,
  summarizeCounts,
  getSortedHashKeys,
  getSortedRangeKeys,
  getFieldsAt,
  sliceKeys,
};
