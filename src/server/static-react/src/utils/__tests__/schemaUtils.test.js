import { describe, it, expect } from 'vitest'
import {
  getSchemaDisplayName,
  isIdentityHash,
  buildSchemaOptions,
} from '../schemaUtils'

const HASH = '04bc93e1986aac6d624eef2a0e5340947d8fe78baab3c908a113a3602fc6b3e8'

describe('isIdentityHash', () => {
  it('accepts 64-char lowercase hex', () => {
    expect(isIdentityHash(HASH)).toBe(true)
  })

  it('rejects mixed case, wrong length, or non-hex', () => {
    expect(isIdentityHash(HASH.toUpperCase())).toBe(false)
    expect(isIdentityHash(HASH.slice(0, 63))).toBe(false)
    expect(isIdentityHash('user_profiles')).toBe(false)
    expect(isIdentityHash(null)).toBe(false)
    expect(isIdentityHash(undefined)).toBe(false)
  })
})

describe('getSchemaDisplayName', () => {
  it('prefers descriptive_name when present', () => {
    expect(
      getSchemaDisplayName({ name: HASH, descriptive_name: 'Dogfood Test Files' }),
    ).toBe('Dogfood Test Files')
  })

  it('falls back to "Schema <short>" when descriptive_name is missing and name is a hash', () => {
    expect(getSchemaDisplayName({ name: HASH })).toBe('Schema 04bc93e1')
    expect(getSchemaDisplayName({ name: HASH, descriptive_name: '' })).toBe(
      'Schema 04bc93e1',
    )
    expect(getSchemaDisplayName({ name: HASH, descriptive_name: '   ' })).toBe(
      'Schema 04bc93e1',
    )
  })

  it('returns the raw name when it is already human-readable', () => {
    expect(getSchemaDisplayName({ name: 'user_profiles' })).toBe('user_profiles')
  })

  it('returns empty string when schema is null/undefined', () => {
    expect(getSchemaDisplayName(null)).toBe('')
    expect(getSchemaDisplayName(undefined)).toBe('')
  })
})

describe('buildSchemaOptions', () => {
  it('uses the human-readable label for hash-named schemas without descriptive_name', () => {
    const options = buildSchemaOptions([{ name: HASH }])
    expect(options).toEqual([{ value: HASH, label: 'Schema 04bc93e1' }])
  })
})
