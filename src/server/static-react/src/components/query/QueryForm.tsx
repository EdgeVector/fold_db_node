import { useState, useCallback } from 'react';
import FieldWrapper from '../form/FieldWrapper';
import SelectField from '../form/SelectField';
import RangeField from '../form/RangeField';
import { FORM_LABELS } from '../../constants/ui';
import { getHashKey, getRangeKey } from '../../utils/rangeSchemaHelpers';
import { buildSchemaOptions, getFieldNames } from '../../utils/schemaUtils';
import type { Schema } from '../../types/schema';

interface QueryStateLike {
  selectedSchema?: string;
  queryFields?: string[];
  hashKeyValue?: string;
  rangeKeyValue?: string;
  rangeSchemaFilter?: { start?: string; end?: string; key?: string; keyPrefix?: string };
}

interface QueryFormProps {
  queryState?: QueryStateLike;
  onSchemaChange: (value: string) => void;
  onFieldToggle: (fieldName: string) => void;
  onFieldValueChange?: (field: string, value: unknown) => void;
  onRangeFilterChange?: (filter: unknown) => void;
  onRangeSchemaFilterChange: (value: unknown) => void;
  onHashKeyChange: (value: string) => void;
  approvedSchemas: Schema[];
  orgNames?: Record<string, string>;
  schemasLoading?: boolean;
  isRangeSchema?: boolean;
  isHashRangeSchema?: boolean;
  rangeKey?: string | null;
  className?: string;
}

function QueryForm({
  queryState,
  onSchemaChange,
  onFieldToggle,
  onFieldValueChange: _onFieldValueChange,
  onRangeFilterChange: _onRangeFilterChange,
  onRangeSchemaFilterChange,
  onHashKeyChange,
  approvedSchemas,
  orgNames,
  schemasLoading,
  isRangeSchema,
  isHashRangeSchema,
  rangeKey,
  className = ''
}: QueryFormProps) {
  const [validationErrors, setValidationErrors] = useState<Record<string, string>>({});

  const handleSchemaChange = useCallback((value: string) => {
    onSchemaChange(value);
    setValidationErrors(prev => {
      const { schema: _schema, ...rest } = prev;
      return rest;
    });
  }, [onSchemaChange]);

  const handleFieldToggle = useCallback((fieldName: string) => {
    onFieldToggle(fieldName);
    setValidationErrors(prev => {
      const { fields: _fields, ...rest } = prev;
      return rest;
    });
  }, [onFieldToggle]);

  const selectedSchema = queryState?.selectedSchema && approvedSchemas
    ? approvedSchemas.find(s => s.name === queryState.selectedSchema)
    : null;

  const fieldNames = getFieldNames(selectedSchema);
  const labels = FORM_LABELS as Record<string, string>;

  return (
    <div className={`space-y-6 ${className}`}>
      <FieldWrapper
        label={labels.schema || 'Schema'}
        name="schema"
        required
        error={validationErrors.schema}
        helpText={labels.schemaHelp || 'Select a schema to work with'}
      >
        <SelectField
          name="schema"
          value={queryState?.selectedSchema || ''}
          onChange={handleSchemaChange}
          options={buildSchemaOptions(approvedSchemas, orgNames)}
        />
      </FieldWrapper>

      {queryState?.selectedSchema && fieldNames.length > 0 && (
        <FieldWrapper
          label="Field Selection"
          name="fields"
          required
          error={validationErrors.fields}
          helpText="Select fields to include in your query"
        >
          <div className="card p-4">
            <div className="space-y-3">
              {fieldNames.map(fieldName => (
                <label key={fieldName} className="relative flex items-start">
                  <div className="flex items-center h-5">
                    <input
                      type="checkbox"
                      className="h-4 w-4 text-primary rounded border border-border focus:ring-primary"
                      checked={queryState?.queryFields?.includes(fieldName) || false}
                      onChange={() => handleFieldToggle(fieldName)}
                    />
                  </div>
                  <div className="ml-3 flex items-center">
                    <span className="text-sm font-medium text-primary">{fieldName}</span>
                  </div>
                </label>
              ))}
            </div>
          </div>
        </FieldWrapper>
      )}

      {isHashRangeSchema && (
        <FieldWrapper
          label="HashRange Filter"
          name="hashRangeFilter"
          helpText="Filter data by hash and range key values"
        >
          <div className="bg-gruvbox-elevated border border-border p-4 space-y-4">
            <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
              <div className="space-y-2">
                <label className="label">Hash Key</label>
                <input
                  type="text"
                  placeholder="Enter hash key value"
                  className="input"
                  value={queryState?.hashKeyValue || ''}
                  onChange={(e) => onHashKeyChange(e.target.value)}
                />
                <p className="text-xs text-secondary">
                  Hash field: {getHashKey(approvedSchemas.find(s => s.name === queryState?.selectedSchema)) || 'N/A'}
                </p>
              </div>
              <div className="space-y-2">
                <label className="label">Range Key</label>
                <input
                  type="text"
                  placeholder="Enter range key value"
                  className="input"
                  value={queryState?.rangeKeyValue || ''}
                  onChange={(e) => onRangeSchemaFilterChange({ key: e.target.value })}
                />
                <p className="text-xs text-secondary">
                  Range field: {getRangeKey(approvedSchemas.find(s => s.name === queryState?.selectedSchema)) || 'N/A'}
                </p>
              </div>
            </div>
            <p className="text-xs text-secondary">
              <strong>Hash Key:</strong> Partitions data. <strong>Range Key:</strong> Orders data within partitions.
            </p>
          </div>
        </FieldWrapper>
      )}

      {isRangeSchema && rangeKey && (
        <FieldWrapper
          label="Range Filter"
          name="rangeSchemaFilter"
          error={validationErrors.rangeFilter}
          helpText="Filter data by range key values"
        >
          <RangeField
            name="rangeSchemaFilter"
            value={queryState?.rangeSchemaFilter || {}}
            onChange={(value) => {
              onRangeSchemaFilterChange(value);
              setValidationErrors(prev => {
                const { rangeFilter: _rangeFilter, ...rest } = prev;
                return rest;
              });
            }}
            config={{ rangeKeyName: rangeKey, mode: 'all' }}
          />
        </FieldWrapper>
      )}
    </div>
  );
}

export default QueryForm;
export { QueryForm };
