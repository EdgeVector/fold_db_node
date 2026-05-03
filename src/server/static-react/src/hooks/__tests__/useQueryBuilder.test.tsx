/**
 * useQueryBuilder Hook Tests
 * Tests for UCR-1-5: Create QueryBuilder hook for complex query construction
 * Part of UTC-1 Test Coverage Enhancement - Hook Testing
 */

import React from 'react';
import { describe, it, expect, beforeEach, afterEach, vi, type Mock } from 'vitest';
import { renderHook } from '@testing-library/react';
import { Provider } from 'react-redux';
import { useQueryBuilder } from '../useQueryBuilder';
import { createTestStore } from '../../test/utils/testUtilities';
import { useAppSelector } from '../../store/hooks';

// Mock the Redux store hooks
vi.mock('../../store/hooks', () => ({
  useAppSelector: vi.fn()
}));

describe('useQueryBuilder Hook', () => {
  let mockStore: ReturnType<typeof createTestStore>;
  let mockUseAppSelector: Mock;

  const mockSchemas = {
    UserSchema: {
      name: 'UserSchema',
      schema_type: 'Regular',
      fields: {
        id: { field_type: 'String', required: true },
        name: { field_type: 'String', required: false },
        age: { field_type: 'Number', required: false },
        email: { field_type: 'String', required: true }
      }
    },
    RangeSchema: {
      name: 'RangeSchema',
      schema_type: 'Range',
      fields: {
        range_key: { field_type: 'Range', required: true },
        data: { field_type: 'String', required: false }
      }
    }
  };

  const mockApprovedSchemas = [
    mockSchemas.UserSchema,
    mockSchemas.RangeSchema
  ];

  beforeEach(() => {
    mockStore = createTestStore();
    mockUseAppSelector = vi.mocked(useAppSelector);

    // Set up default mock implementation
    mockUseAppSelector.mockImplementation((selector) => {
      if (selector.toString().includes('selectApprovedSchemas') || selector.toString().includes('approved')) {
        return mockApprovedSchemas;
      }
      return undefined;
    });
  });

  afterEach(() => {
    vi.clearAllMocks();
  });

  type BuilderOptions = Parameters<typeof useQueryBuilder>[0];

  const renderUseQueryBuilder = (options: Partial<BuilderOptions> = {}) => {
    const defaultOptions = {
      schema: '',
      queryState: {
        queryFields: [],
        fieldValues: {},
        rangeFilters: {},
        filters: [],
      },
      schemas: mockSchemas as unknown as BuilderOptions['schemas'],
      ...options
    } as BuilderOptions;

    return renderHook(() => useQueryBuilder(defaultOptions), {
      wrapper: ({ children }: { children?: React.ReactNode }) => (
        <Provider store={mockStore}>
          {children}
        </Provider>
      )
    });
  };

  describe('initialization', () => {
    it('should initialize with empty query and validation error when no schema selected', () => {
      const { result } = renderUseQueryBuilder();

      expect(result.current.query).toEqual({});
      expect(result.current.validationErrors).toEqual(['No schema selected']);
      expect(result.current.isValid).toBe(false); // Invalid without a schema
    });

    it('should not provide build and validate functions (removed)', () => {
      const { result } = renderUseQueryBuilder();

      expect((result.current as unknown as Record<string, unknown>).buildQuery).toBeUndefined();
      expect((result.current as unknown as Record<string, unknown>).validateQuery).toBeUndefined();
    });
  });

  describe('schema validation (removed - backend validates)', () => {
    it('should be invalid when schema is not selected', () => {
      const { result } = renderUseQueryBuilder({
        schema: '',
        queryState: {
          queryFields: ['id'],
          fieldValues: { id: 'test' }
        }
      });

      expect(result.current.validationErrors).toEqual(['No schema selected']);
      expect(result.current.isValid).toBe(false); // Invalid without a schema
    });

    it('should always be valid when selected schema is not found (no frontend validation)', () => {
      const { result } = renderUseQueryBuilder({
        schema: 'NonExistentSchema',
        queryState: {
          queryFields: ['id'],
          fieldValues: { id: 'test' }
        }
      });

      expect(result.current.validationErrors).toEqual([]);
      expect(result.current.isValid).toBe(true); // Always valid - backend validates
    });

    it('should be valid with correct schema and fields', () => {
      const { result } = renderUseQueryBuilder({
        schema: 'UserSchema',
        queryState: {
          queryFields: ['id', 'name'],
          fieldValues: { id: 'user123', name: 'John Doe' }
        }
      });

      expect(result.current.validationErrors).toEqual([]);
      expect(result.current.isValid).toBe(true);
    });
  });

  describe('field validation', () => {
    it('should not validate required fields (backend handles validation)', () => {
      const { result } = renderUseQueryBuilder({
        schema: 'UserSchema',
        queryState: {
          queryFields: ['id', 'email'],
          fieldValues: { id: 'user123' } // Missing required email
        }
      });

      expect(result.current.validationErrors).toEqual([]);
      expect(result.current.isValid).toBe(true);
    });

    it('should not validate required fields that are not selected', () => {
      const { result } = renderUseQueryBuilder({
        schema: 'UserSchema',
        queryState: {
          queryFields: ['name'], // Only non-required field
          fieldValues: { name: 'John' }
        }
      });

      expect(result.current.isValid).toBe(true);
    });

    it('should not validate field types (backend handles validation)', () => {
      const { result } = renderUseQueryBuilder({
        schema: 'UserSchema',
        queryState: {
          queryFields: ['age'],
          fieldValues: { age: 'not_a_number' }
        }
      });

      expect(result.current.validationErrors).toEqual([]);
      expect(result.current.isValid).toBe(true);
    });

    it('should accept valid numbers as strings', () => {
      const { result } = renderUseQueryBuilder({
        schema: 'UserSchema',
        queryState: {
          queryFields: ['age'],
          fieldValues: { age: '25' }
        }
      });

      expect(result.current.isValid).toBe(true);
    });

    it('should accept actual numbers', () => {
      const { result } = renderUseQueryBuilder({
        schema: 'UserSchema',
        queryState: {
          queryFields: ['age'],
          fieldValues: { age: 25 }
        }
      });

      expect(result.current.isValid).toBe(true);
    });

    it('should not validate empty required field values (backend handles validation)', () => {
      const { result } = renderUseQueryBuilder({
        schema: 'UserSchema',
        queryState: {
          queryFields: ['id'],
          fieldValues: { id: '' } // Empty required field
        }
      });

      expect(result.current.validationErrors).toEqual([]);
      expect(result.current.isValid).toBe(true);
    });

    it('should not validate null required field values (backend handles validation)', () => {
      const { result } = renderUseQueryBuilder({
        schema: 'UserSchema',
        queryState: {
          queryFields: ['id'],
          fieldValues: { id: null } // Null required field
        }
      });

      expect(result.current.validationErrors).toEqual([]);
      expect(result.current.isValid).toBe(true);
    });
  });

  describe('range schema validation', () => {
    it('should not require range key for range schemas (backend handles validation)', () => {
      const { result } = renderUseQueryBuilder({
        schema: 'RangeSchema',
        queryState: {
          queryFields: ['data'],
          fieldValues: { data: 'test' },
          rangeFilters: {} // No range key provided
        }
      });

      expect(result.current.validationErrors).toEqual([]);
      expect(result.current.isValid).toBe(true);
    });

    it('should be valid when range key is provided for range schemas', () => {
      const { result } = renderUseQueryBuilder({
        schema: 'RangeSchema',
        queryState: {
          queryFields: ['data'],
          fieldValues: { data: 'test' },
          rangeFilters: { range_key: { key: 'range_value' } }
        }
      });

      expect(result.current.isValid).toBe(true);
    });

    it('should not require range key for range schemas with no fields selected', () => {
      const { result } = renderUseQueryBuilder({
        schema: 'RangeSchema',
        queryState: {
          queryFields: [], // No fields selected
          fieldValues: {},
          rangeFilters: {}
        }
      });

      expect(result.current.isValid).toBe(true);
    });
  });

  describe('filter validation', () => {
    it('should not validate filters against schema fields (backend handles validation)', () => {
      const { result } = renderUseQueryBuilder({
        schema: 'UserSchema',
        queryState: {
          queryFields: ['id'],
          fieldValues: { id: 'test' },
          filters: [{ field: 'nonexistent_field', operator: 'eq', value: 'test' }]
        }
      });

      expect(result.current.validationErrors).toEqual([]);
      expect(result.current.isValid).toBe(true);
    });

    it('should be valid when filters reference existing fields', () => {
      const { result } = renderUseQueryBuilder({
        schema: 'UserSchema',
        queryState: {
          queryFields: ['id'],
          fieldValues: { id: 'test' },
          filters: [{ field: 'name', operator: 'eq', value: 'John' }]
        }
      });

      expect(result.current.isValid).toBe(true);
    });
  });

  describe('query building', () => {
    it('should build basic query', () => {
      const { result } = renderUseQueryBuilder({
        schema: 'UserSchema',
        queryState: {
          queryFields: ['id', 'name'],
          fieldValues: { id: 'user123', name: 'John' }
        }
      });

      expect(result.current.query).toEqual({
        schema_name: 'UserSchema', // Backend expects schema_name
        fields: ['id', 'name'] // Array of field names
      });
    });

    it('should not include filters (not in backend Query struct)', () => {
      const filters = [{ field: 'age', operator: 'gt', value: 18 }];
      
      const { result } = renderUseQueryBuilder({
        schema: 'UserSchema',
        queryState: {
          queryFields: ['id'],
          fieldValues: { id: 'test' },
          filters
        }
      });

      // Backend Query struct doesn't have filters field
      expect((result.current.query as Record<string, unknown>).filters).toBeUndefined();
    });

    it('should not include orderBy (not in backend Query struct)', () => {
      const orderBy = { field: 'name', direction: 'asc' };

      const { result } = renderUseQueryBuilder({
        schema: 'UserSchema',
        queryState: {
          queryFields: ['id'],
          fieldValues: { id: 'test' },
          orderBy
        }
      });

      // Backend Query struct doesn't have orderBy field
      expect((result.current.query as Record<string, unknown>).orderBy).toBeUndefined();
    });

    it('should build query with range key for range schemas', () => {
      const { result } = renderUseQueryBuilder({
        schema: 'RangeSchema',
        queryState: {
          queryFields: ['data'],
          fieldValues: { data: 'test' },
          rangeFilters: { range_key: { key: 'user:123' } }
        }
      });

      expect(result.current.query.filter).toEqual({ "RangeKey": "user:123" });
    });

    it('should not include rangeKey for non-range schemas', () => {
      const { result } = renderUseQueryBuilder({
        schema: 'UserSchema',
        queryState: {
          queryFields: ['id'],
          fieldValues: { id: 'test' },
          rangeFilters: { some_field: { key: 'test' } }
        }
      });

      expect((result.current.query as Record<string, unknown>).rangeKey).toBeUndefined();
    });

    it('should handle empty query state', () => {
      const { result } = renderUseQueryBuilder({
        schema: 'UserSchema',
        queryState: undefined
      });

      expect(result.current.query).toEqual({});
    });
  });

  describe('query format matching backend Query struct', () => {
    it('should use schema_name not type or schema', () => {
      const { result } = renderUseQueryBuilder({
        schema: 'UserSchema',
        queryState: {
          queryFields: ['id'],
          fieldValues: { id: 'test' }
        }
      });

      expect(result.current.query).toHaveProperty('schema_name', 'UserSchema');
      expect(result.current.query).not.toHaveProperty('type');
      expect(result.current.query).not.toHaveProperty('schema');
    });

    it('should format fields as array matching backend Query struct', () => {
      const { result } = renderUseQueryBuilder({
        schema: 'UserSchema',
        queryState: {
          queryFields: ['id', 'name'],
          fieldValues: { id: 'test', name: 'John' }
        }
      });

      expect(result.current.query.fields).toEqual(['id', 'name']);
      expect(Array.isArray(result.current.query.fields)).toBe(true);
    });

    it('should not include fieldValues property (not in backend Query struct)', () => {
      const fieldValues = { id: 'user123', name: 'John Doe' };
      const { result } = renderUseQueryBuilder({
        schema: 'UserSchema',
        queryState: {
          queryFields: ['id', 'name'],
          fieldValues
        }
      });

      expect(result.current.query).not.toHaveProperty('fieldValues');
      expect(result.current.query.fields).toEqual(['id', 'name']);
    });

    it('should only include schema_name, fields, and filter (Query struct)', () => {
      const { result } = renderUseQueryBuilder({
        schema: 'UserSchema',
        queryState: {
          queryFields: ['id', 'name'],
          fieldValues: { id: 'user123', name: 'John' },
          filters: [{ field: 'age', operator: 'gt', value: '18' }]
        }
      });

      const query = result.current.query;
      
      // Query struct has exactly: schema_name, fields, filter (optional)
      expect(query).toHaveProperty('schema_name', 'UserSchema');
      expect(query).toHaveProperty('fields');
      expect(Array.isArray(query.fields)).toBe(true);
      // Should not have these fields
      expect(query).not.toHaveProperty('type');
      expect(query).not.toHaveProperty('schema');
      expect(query).not.toHaveProperty('queryFields');
      expect(query).not.toHaveProperty('fieldValues');
    });
  });

  describe('manual functions (removed)', () => {
    it('should not provide buildQuery function (removed - use query directly)', () => {
      const { result } = renderUseQueryBuilder({
        schema: 'UserSchema',
        queryState: {
          queryFields: ['id'],
          fieldValues: { id: 'test' }
        }
      });

      expect((result.current as unknown as Record<string, unknown>).buildQuery).toBeUndefined();
      expect(result.current.query).toBeDefined(); // Query is available directly
    });

    it('should not provide validateQuery function (removed - backend validates)', () => {
      const { result } = renderUseQueryBuilder({
        schema: 'UserSchema',
        queryState: {
          queryFields: ['id'],
          fieldValues: { id: '' } // Backend will validate
        }
      });

      expect((result.current as unknown as Record<string, unknown>).validateQuery).toBeUndefined();
      expect(result.current.isValid).toBe(true); // Always true - no frontend validation
      expect(result.current.validationErrors).toEqual([]);
    });
  });

  describe('edge cases', () => {
    it('should handle schema without fields', () => {
      const schemasWithoutFields = {
        EmptySchema: {
          name: 'EmptySchema',
          schema_type: 'Regular'
          // No fields property
        }
      };

      const { result } = renderUseQueryBuilder({
        schema: 'EmptySchema',
        queryState: {
          queryFields: [],
          fieldValues: {}
        },
        schemas: schemasWithoutFields
      });

      expect(result.current.isValid).toBe(true);
    });

    it('should handle missing queryState properties gracefully', () => {
      const { result } = renderUseQueryBuilder({
        schema: 'UserSchema',
        queryState: {} // Missing all expected properties
      });

      expect(result.current.query.schema_name).toBe('UserSchema');
      expect(result.current.query.fields).toEqual([]); // Should be empty array
    });

    it('should handle undefined schema in queryState', () => {
      const { result } = renderUseQueryBuilder({
        schema: undefined,
        queryState: {
          queryFields: ['id'],
          fieldValues: { id: 'test' }
        }
      });

      expect(result.current.validationErrors).toEqual(['No schema selected']);
    });

    it('should handle null filters and orderBy', () => {
      const { result } = renderUseQueryBuilder({
        schema: 'UserSchema',
        queryState: {
          queryFields: ['id'],
          fieldValues: { id: 'test' },
          filters: null as unknown as Record<string, unknown>[],
          orderBy: null as unknown as Record<string, unknown>
        }
      });

      expect((result.current.query as Record<string, unknown>).filters).toBeUndefined();
      expect((result.current.query as Record<string, unknown>).orderBy).toBeUndefined();
    });
  });

  describe('schema source priority', () => {
    it('should prefer provided schemas over Redux approved schemas', () => {
      const localSchemas = {
        LocalSchema: {
          name: 'LocalSchema',
          fields: { local_field: { field_type: 'String' } }
        }
      };

      const { result } = renderUseQueryBuilder({
        schema: 'LocalSchema',
        queryState: {
          queryFields: ['local_field'],
          fieldValues: { local_field: 'test' }
        },
        schemas: localSchemas
      });

      expect(result.current.isValid).toBe(true);
      expect(result.current.query.schema_name).toBe('LocalSchema');
    });

    it('should fall back to Redux approved schemas when not in provided schemas', () => {
      // Override the mock for this specific test
      mockUseAppSelector.mockReturnValue(mockApprovedSchemas);
      
      const { result } = renderUseQueryBuilder({
        schema: 'UserSchema', // Not in local schemas, should come from Redux
        queryState: {
          queryFields: ['id'],
          fieldValues: { id: 'test' }
        },
        schemas: {} // Empty local schemas
      });

      expect(result.current.isValid).toBe(true);
      expect(result.current.query.schema_name).toBe('UserSchema');
    });
  });
});