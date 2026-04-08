import { useState } from 'react'
import SchemaSelector from './mutation/SchemaSelector'
import MutationEditor from './mutation/MutationEditor'
import ResultViewer from './mutation/ResultViewer'
import TextField from '../form/TextField'
import { mutationClient } from '../../api'
import { MUTATION_TYPE_API_MAP, BUTTON_TEXT, FORM_LABELS, RANGE_SCHEMA_CONFIG } from '../../constants/ui.js'
import {
  isRangeSchema,
  isHashRangeSchema,
  formatRangeMutation,
  getRangeKey,
  getHashKey
} from '../../utils/rangeSchemaHelpers'
import { useAppSelector } from '../../store/hooks'
import { selectApprovedSchemas } from '../../store/schemaSlice'

function MutationTab({ onResult }) {
  // Redux state
  const schemas = useAppSelector(selectApprovedSchemas)
  const [selectedSchema, setSelectedSchema] = useState('')
  const [mutationData, setMutationData] = useState({})
  const [mutationType, setMutationType] = useState('Insert')
  const [result, setResult] = useState(null)
  const [rangeKeyValue, setRangeKeyValue] = useState('')
  const [hashKeyValue, setHashKeyValue] = useState('')

  const handleSchemaChange = (schemaName) => {
    setSelectedSchema(schemaName)
    setMutationData({})
    setMutationType('Insert')
    setRangeKeyValue('')
    setHashKeyValue('')
    setResult(null)
  }

  const handleFieldChange = (fieldName, value) => {
    setMutationData(prev => ({ ...prev, [fieldName]: value }))
  }

  const handleSubmit = async (e) => {
    e.preventDefault()
    if (!selectedSchema) return

    const selectedSchemaObj = schemas.find(s => s.name === selectedSchema)
    const normalizedMutationType = mutationType
      ? (MUTATION_TYPE_API_MAP[mutationType] || mutationType.toLowerCase())
      : ''
    if (!normalizedMutationType) return

    let mutation

    if (isHashRangeSchema(selectedSchemaObj)) {
      // HashRange: plain values with hash + range key
      mutation = {
        type: 'mutation',
        schema: selectedSchema,
        mutation_type: normalizedMutationType,
        fields_and_values: mutationData,
        key_value: {
          hash: hashKeyValue.trim() || null,
          range: rangeKeyValue.trim() || null
        }
      }
    } else if (isRangeSchema(selectedSchemaObj)) {
      mutation = formatRangeMutation(selectedSchemaObj, mutationType, rangeKeyValue, mutationData)
    } else {
      // Single schema — hash key only
      mutation = {
        type: 'mutation',
        schema: selectedSchema,
        mutation_type: normalizedMutationType,
        fields_and_values: mutationData,
        key_value: { hash: hashKeyValue.trim() || null, range: null }
      }
    }

    try {
      // Send the mutation directly to the API (no signing required)
      const response = await mutationClient.executeMutation(mutation)

      if (!response.success) {
        throw new Error(response.error || 'Mutation failed')
      }

      setResult({ success: true, data: response.data || response })
      onResult(response)
      setMutationData({})
      setRangeKeyValue('')
      setHashKeyValue('')
    } catch (error) {
      const errMsg = error instanceof Error ? error.message : String(error)
      const errData = { success: false, error: errMsg }
      setResult(errData)
      onResult(errData)
    }
  }

  const selectedSchemaObj = selectedSchema ? schemas.find(s => s.name === selectedSchema) : null
  const isCurrentSchemaRange = selectedSchemaObj ? isRangeSchema(selectedSchemaObj) : false
  const isCurrentSchemaHashRange = selectedSchemaObj ? isHashRangeSchema(selectedSchemaObj) : false
  const hashKey = selectedSchemaObj ? getHashKey(selectedSchemaObj) : null
  const rangeKey = selectedSchemaObj ? getRangeKey(selectedSchemaObj) : null

  // Convert declarative schema fields array to object format for MutationEditor
  // Filter out key fields (they're shown separately in Key Configuration)
  const getFieldsForMutation = () => {
    if (!selectedSchemaObj || !Array.isArray(selectedSchemaObj.fields)) return {}

    const keyFields = [hashKey, rangeKey].filter(Boolean)
    const fieldsToShow = selectedSchemaObj.fields.filter(field => !keyFields.includes(field))

    return fieldsToShow.reduce((acc, fieldName) => {
      acc[fieldName] = {}
      return acc
    }, {})
  }

  const selectedSchemaFields = getFieldsForMutation()

  const needsHashKey = isCurrentSchemaHashRange || (!isCurrentSchemaRange && hashKey)
  const needsRangeKey = isCurrentSchemaHashRange || isCurrentSchemaRange

  const isMutationDisabled =
    !selectedSchema ||
    !mutationType ||
    Object.keys(mutationData).length === 0 ||
    (needsRangeKey && !rangeKeyValue.trim())

  return (
    <div>
      <form onSubmit={handleSubmit} className="space-y-6">
        <SchemaSelector
          selectedSchema={selectedSchema}
          mutationType={mutationType}
          onSchemaChange={handleSchemaChange}
          onTypeChange={setMutationType}
        />

        {selectedSchema && (needsHashKey || needsRangeKey) && (
          <div className={RANGE_SCHEMA_CONFIG.backgroundColor}>
            <h3 className="text-lg font-medium text-primary mb-4">Key Configuration</h3>
            {needsHashKey && (
              <TextField
                name="hashKey"
                label={`${hashKey || 'hash'} (hash key)`}
                value={hashKeyValue}
                onChange={setHashKeyValue}
                placeholder={`Enter ${hashKey || 'hash key'} value`}
                required={false}
                error={undefined}
                helpText="Hash key for record grouping"
                debounced={true}
              />
            )}
            {needsRangeKey && (
              <TextField
                name="rangeKey"
                label={`${rangeKey || 'range'} (${RANGE_SCHEMA_CONFIG.label})`}
                value={rangeKeyValue}
                onChange={setRangeKeyValue}
                placeholder={`Enter ${rangeKey || 'range key'} value`}
                required={true}
                error={undefined}
                helpText={FORM_LABELS.rangeKeyRequired}
                debounced={true}
              />
            )}
          </div>
        )}

        {selectedSchema && (
          <MutationEditor
            fields={selectedSchemaFields}
            mutationType={mutationType}
            mutationData={mutationData}
            onFieldChange={handleFieldChange}
            isRangeSchema={isCurrentSchemaRange || isCurrentSchemaHashRange}
          />
        )}

        <div className="flex justify-end pt-4">
          <button type="submit" className="btn-primary btn-lg" disabled={isMutationDisabled}>
            → {BUTTON_TEXT.executeMutation}
          </button>
        </div>
      </form>

      <ResultViewer result={result} />
    </div>
  )
}

export default MutationTab
