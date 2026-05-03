/**
 * Custom hook for managing approved schemas with SCHEMA-002 compliance.
 *
 * Centralized interface for accessing and managing approved schemas, enforcing
 * SCHEMA-002 compliance by only allowing mutations and queries on approved
 * schemas. Backed by Redux for consistent state across the application.
 */

import { useEffect, useCallback } from 'react'
import { useAppSelector, useAppDispatch } from '../store/hooks'
import {
  fetchSchemas,
  selectApprovedSchemas,
  selectAllSchemas,
  selectFetchLoading,
  selectFetchError,
  selectCacheInfo,
} from '../store/schemaSlice'
import { SCHEMA_STATES } from '../constants/redux'
import { normalizeSchemaState } from '../utils/rangeSchemaHelpers'
import type { Schema } from '../types/schema'

interface UseApprovedSchemasOpts {
  enabled?: boolean
}

interface UseApprovedSchemasResult {
  approvedSchemas: Schema[]
  allSchemas: Schema[]
  isLoading: boolean
  error: string | null
  refetch: () => Promise<void>
  getSchemaByName: (name: string) => Schema | null
  isSchemaApproved: (name: string) => boolean
}

export function useApprovedSchemas(
  { enabled = true }: UseApprovedSchemasOpts = {},
): UseApprovedSchemasResult {
  const dispatch = useAppDispatch()
  const approvedSchemas = useAppSelector(selectApprovedSchemas)
  const allSchemas = useAppSelector(selectAllSchemas)
  const isLoading = useAppSelector(selectFetchLoading)
  const error = useAppSelector(selectFetchError)
  const cacheInfo = useAppSelector(selectCacheInfo)

  const refetch = useCallback(async (): Promise<void> => {
    if (enabled) {
      dispatch(fetchSchemas({ forceRefresh: true }))
    }
  }, [dispatch, enabled])

  const getSchemaByName = useCallback(
    (name: string): Schema | null => {
      return allSchemas.find((schema) => schema.name === name) ?? null
    },
    [allSchemas],
  )

  const isSchemaApproved = useCallback(
    (name: string): boolean => {
      const schema = getSchemaByName(name)
      if (!schema) return false
      const normalizedState = normalizeSchemaState(schema.state)
      return normalizedState === SCHEMA_STATES.APPROVED
    },
    [getSchemaByName],
  )

  useEffect(() => {
    if (enabled && !cacheInfo.isValid) {
      dispatch(fetchSchemas())
    }
  }, [dispatch, enabled, cacheInfo.isValid])

  return {
    approvedSchemas,
    allSchemas,
    isLoading,
    error,
    refetch,
    getSchemaByName,
    isSchemaApproved,
  }
}

export default useApprovedSchemas
