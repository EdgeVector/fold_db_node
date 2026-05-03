
import SelectField from '../../form/SelectField'
import { FORM_LABELS, MUTATION_TYPES } from '../../../constants/ui.js'
import { useAppSelector } from '../../../store/hooks'
import { selectApprovedSchemas } from '../../../store/schemaSlice'
import { buildSchemaOptions } from '../../../utils/schemaUtils'

interface SchemaSelectorProps {
  selectedSchema: string
  mutationType: string
  onSchemaChange: (value: string) => void
  onTypeChange: (value: string) => void
}

function SchemaSelector({ selectedSchema, mutationType, onSchemaChange, onTypeChange }: SchemaSelectorProps) {
  // Redux state
  const approvedSchemas = useAppSelector(selectApprovedSchemas)

  return (
    <div className="grid grid-cols-2 gap-4">
      <SelectField
        name="schema"
        label={FORM_LABELS.schema}
        value={selectedSchema}
        onChange={onSchemaChange}
        options={buildSchemaOptions(approvedSchemas)}
        helpText={FORM_LABELS.schemaHelp}
      />

      <SelectField
        name="operationType"
        label={FORM_LABELS.operationType}
        value={mutationType}
        onChange={onTypeChange}
        options={[...MUTATION_TYPES]}
        helpText={FORM_LABELS.operationHelp}
      />
    </div>
  )
}

export default SchemaSelector
