import { describe, it, expect } from 'vitest'
import {
  isRangeSchema,
  getRangeKey,
  getRangeSchemaInfo
} from '../rangeSchemaHelpers'

describe('rangeSchemaUtils', () => {
  describe('isRangeSchema', () => {
    it('should return false for null or undefined schema', () => {
      expect(isRangeSchema(null)).toBe(false)
      expect(isRangeSchema(undefined)).toBe(false)
    })

    it('should return true for schema with Range schema_type even without fields', () => {
      const schema = {
        name: 'TestSchema',
        schema_type: 'Range',
        key: { range_field: 'test_id' }
      }
      expect(isRangeSchema(schema)).toBe(true)
    })

    it('should return true for schema with Range schema_type even with empty fields', () => {
      const schema = {
        name: 'TestSchema',
        schema_type: 'Range',
        key: { range_field: 'test_id' },
        fields: {}
      }
      expect(isRangeSchema(schema)).toBe(true)
    })

    it('should return false for schema without range_key', () => {
      const schema = {
        name: 'TestSchema',
        fields: {
          field1: { field_type: 'Range' },
          field2: { field_type: 'Range' }
        }
      }
      expect(isRangeSchema(schema)).toBe(false)
    })

    it('should return true when schema_type is Range (backend is authoritative)', () => {
      const schema = {
        name: 'TestSchema',
        schema_type: 'Range',
        key: { range_field: 'test_id' },
        fields: {
          test_id: { field_type: 'Range' },
          field1: { field_type: 'Range' },
          field2: { field_type: 'Single' }
        }
      }
      expect(isRangeSchema(schema)).toBe(true)
    })

    it('should return false for old format without schema_type (backend is authoritative)', () => {
      const schema = {
        name: 'UserScores',
        range_key: 'user_id',
        fields: {
          user_id: { field_type: 'Range' },
          game_scores: { field_type: 'Range' },
          achievements: { field_type: 'Range' }
        }
      }
      expect(isRangeSchema(schema)).toBe(false)
    })
  })

  describe('getRangeKey', () => {
    it('should return null for null or undefined schema', () => {
      expect(getRangeKey(null)).toBe(null)
      expect(getRangeKey(undefined)).toBe(null)
    })

    it('should return range_key from new format', () => {
      const schema = {
        name: 'TestSchema',
        schema_type: 'Range',
        key: { range_field: 'test_id' }
      }
      expect(getRangeKey(schema)).toBe('test_id')
    })

    it('should return null for old format (no longer supported)', () => {
      const schema = {
        name: 'TestSchema',
        range_key: 'test_id'
      }
      expect(getRangeKey(schema)).toBe(null)
    })

    it('should prioritize new format over old format', () => {
      const schema = {
        name: 'TestSchema',
        range_key: 'old_key',
        schema_type: 'Range',
        key: { range_field: 'new_key' }
      }
      expect(getRangeKey(schema)).toBe('new_key')
    })

    it('should return null when no range_key is found', () => {
      const schema = {
        name: 'TestSchema',
        schema_type: { Single: {} }
      }
      expect(getRangeKey(schema)).toBe(null)
    })
  })

  describe('getRangeSchemaInfo', () => {
    it('should return null for non-range schema', () => {
      const schema = {
        name: 'TestSchema',
        fields: {
          field1: { field_type: 'Single' }
        }
      }
      expect(getRangeSchemaInfo(schema)).toBe(null)
    })

    it('should return comprehensive info for range schema', () => {
      const schema = {
        name: 'UserScores',
        schema_type: 'Range',
        key: { range_field: 'user_id' },
        fields: ['user_id', 'game_scores', 'achievements']
      }

      const result = getRangeSchemaInfo(schema)

      expect(result).toEqual({
        isRangeSchema: true,
        rangeKey: 'user_id',
        rangeFields: [],
        nonRangeKeyFields: {
          game_scores: {},
          achievements: {}
        },
        totalFields: 3
      })
    })

    it('should handle schema with mixed field types', () => {
      const schema = {
        name: 'MixedSchema',
        range_key: 'key_field',
        fields: {
          key_field: { field_type: 'Range' },
          range_field: { field_type: 'Range' },
          single_field: { field_type: 'Single' }
        }
      }
      expect(getRangeSchemaInfo(schema)).toBe(null)
    })
  })

  describe('Edge Cases and Integration', () => {
    it('should handle schema with malformed schema_type', () => {
      const schema = {
        name: 'MalformedSchema',
        schema_type: { InvalidType: { some_key: 'value' } },
        fields: {
          field1: { field_type: 'Range' }
        }
      }

      expect(isRangeSchema(schema)).toBe(false)
      expect(getRangeKey(schema)).toBe(null)
    })

    it('should work with real declarative schema structure', () => {
      const blogPostSchema = {
        name: 'BlogPost',
        schema_type: 'Range',
        key: { range_field: 'publish_date' },
        fields: ['author', 'content', 'publish_date', 'tags', 'title']
      }

      expect(isRangeSchema(blogPostSchema)).toBe(true)
      expect(getRangeKey(blogPostSchema)).toBe('publish_date')
    })
  })
})
