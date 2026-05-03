/**
 * Custom hook for managing query state with Redux integration.
 *
 * Centralized query state management for the QueryTab component — handles
 * field selection, schema management, and filter state.
 */

import { useState, useCallback, useMemo } from 'react'
import { useAppSelector, useAppDispatch } from '../store/hooks'
import {
  selectApprovedSchemas,
  selectAllSchemas,
  selectFetchLoading,
  fetchSchemas,
} from '../store/schemaSlice'
import {
  isHashRangeSchema,
  isRangeSchema,
  getRangeKey,
} from '../utils/rangeSchemaHelpers'
import type { Schema } from '../types/schema'

interface RangeFilter {
  start?: string
  end?: string
  prefix?: string
  pattern?: string
  values?: string[]
  filter?: string
}

interface QueryStateShape {
  selectedSchema: string
  queryFields: string[]
  fieldValues: Record<string, string>
  rangeFilters: Record<string, RangeFilter>
  rangeSchemaFilter: RangeFilter
  rangeKeyValue: string
  hashKeyValue: string
}

function useQueryState() {
  const dispatch = useAppDispatch()
  const schemas = useAppSelector(selectAllSchemas)
  const schemasLoading = useAppSelector(selectFetchLoading)

  const [selectedSchema, setSelectedSchema] = useState('')
  const [queryFields, setQueryFields] = useState<string[]>([])
  const [fieldValues, setFieldValues] = useState<Record<string, string>>({})
  const [rangeFilters, setRangeFilters] = useState<Record<string, RangeFilter>>({})
  const [rangeKeyValue, setRangeKeyValue] = useState('')
  const [hashKeyValue, setHashKeyValue] = useState('')
  const [rangeSchemaFilter, setRangeSchemaFilter] = useState<RangeFilter>({})

  const approvedSchemas = useAppSelector(selectApprovedSchemas)

  const selectedSchemaObj = useMemo<Schema | null>(() => {
    return selectedSchema
      ? (schemas ?? []).find((s) => s.name === selectedSchema) ?? null
      : null
  }, [selectedSchema, schemas])

  const isCurrentSchemaRangeSchema = useMemo(() => {
    return selectedSchemaObj ? isRangeSchema(selectedSchemaObj) : false
  }, [selectedSchemaObj])

  const isCurrentSchemaHashRangeSchema = useMemo(() => {
    return selectedSchemaObj ? isHashRangeSchema(selectedSchemaObj) : false
  }, [selectedSchemaObj])

  const rangeKey = useMemo(() => {
    return selectedSchemaObj ? getRangeKey(selectedSchemaObj) : null
  }, [selectedSchemaObj])

  const handleSchemaChange = useCallback(
    (schemaName: string) => {
      setSelectedSchema(schemaName)

      if (schemaName) {
        const schemaObj = (schemas ?? []).find((s) => s.name === schemaName)
        // Handle both regular schemas (fields array/object) and transform schemas
        const schemaFields =
          (schemaObj as unknown as { fields?: unknown; transform_fields?: unknown })
            ?.fields ??
          (schemaObj as unknown as { transform_fields?: unknown })?.transform_fields ??
          []
        const allFieldNames: string[] = Array.isArray(schemaFields)
          ? (schemaFields as string[])
          : Object.keys(schemaFields as Record<string, unknown>)
        setQueryFields(allFieldNames)

        const initialFieldValues: Record<string, string> = {}
        allFieldNames.forEach((fieldName) => {
          initialFieldValues[fieldName] = ''
        })
        setFieldValues(initialFieldValues)
      } else {
        setQueryFields([])
        setFieldValues({})
      }

      setRangeFilters({})
      setRangeKeyValue('')
      setHashKeyValue('')
      setRangeSchemaFilter({})
    },
    [schemas],
  )

  const toggleField = useCallback((fieldName: string) => {
    setQueryFields((prev) => {
      if (prev.includes(fieldName)) {
        return prev.filter((f) => f !== fieldName)
      }
      return [...prev, fieldName]
    })

    setFieldValues((prev) => {
      if (prev[fieldName] !== undefined) {
        return prev
      }
      return {
        ...prev,
        [fieldName]: '',
      }
    })
  }, [])

  const handleRangeFilterChange = useCallback(
    (fieldName: string, filterType: keyof RangeFilter, value: unknown) => {
      setRangeFilters((prev) => ({
        ...prev,
        [fieldName]: {
          ...prev[fieldName],
          [filterType]: value,
        },
      }))
    },
    [],
  )

  const handleFieldValueChange = useCallback(
    (fieldName: string, value: string) => {
      setFieldValues((prev) => ({
        ...prev,
        [fieldName]: value,
      }))
    },
    [],
  )

  const clearState = useCallback(() => {
    setSelectedSchema('')
    setQueryFields([])
    setFieldValues({})
    setRangeFilters({})
    setRangeKeyValue('')
    setHashKeyValue('')
    setRangeSchemaFilter({})
  }, [])

  const refetchSchemas = useCallback(() => {
    dispatch(fetchSchemas({ forceRefresh: true }))
  }, [dispatch])

  const state: QueryStateShape = {
    selectedSchema,
    queryFields,
    fieldValues,
    rangeFilters,
    rangeSchemaFilter,
    rangeKeyValue,
    hashKeyValue,
  }

  return {
    state,
    setSelectedSchema,
    setQueryFields,
    setFieldValues,
    toggleField,
    handleFieldValueChange,
    setRangeFilters,
    setRangeSchemaFilter,
    setRangeKeyValue,
    setHashKeyValue,
    clearState,
    clearQuery: clearState,
    handleSchemaChange,
    handleRangeFilterChange,
    refetchSchemas,
    approvedSchemas,
    schemasLoading,
    selectedSchemaObj,
    isRangeSchema: isCurrentSchemaRangeSchema,
    isHashRangeSchema: isCurrentSchemaHashRangeSchema,
    rangeKey,
  }
}

export default useQueryState
export { useQueryState }
export type { QueryStateShape, RangeFilter }
