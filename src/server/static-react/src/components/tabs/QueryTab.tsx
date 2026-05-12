/**
 * QueryTab Component
 * Orchestrates query form, actions, preview, and execution.
 */

import { useCallback, useState, useEffect } from 'react';
import type { OperationResultPayload } from '../../types/api';
import { mutationClient } from '../../api/clients/mutationClient';
import { ApiError } from '../../api/core/errors';
import { useQueryState } from '../../hooks/useQueryState';
import { useQueryBuilder } from '../../hooks/useQueryBuilder';
import QueryForm from '../query/QueryForm';
import QueryActions from '../query/QueryActions';
import QueryPreview from '../query/QueryPreview';
import { getSchemaDisplayName } from '../../utils/schemaUtils';
import { useOrgNames } from '../../hooks/useOrgNames';

interface QueryTabProps {
  onResult: (result: OperationResultPayload & { details?: unknown }) => void;
}

interface UnknownFieldsError {
  message: string;
  schemaName: string;
  unknownFields: string[];
  availableFields: string[];
}

// The backend returns `{ ok:false, error:'unknown_fields', message, schema_name,
// unknown_fields, available_fields }` as a structured 400 when the request
// names fields that don't exist on the schema. Pull the structured shape out
// so the UI can render an actionable chip row instead of just the message.
function extractUnknownFieldsError(error: unknown): UnknownFieldsError | null {
  if (!(error instanceof ApiError) || error.status !== 400) return null;
  const payload = error.response;
  if (!payload || payload instanceof Response) return null;
  const body = payload as Record<string, unknown>;
  if (body.error !== 'unknown_fields') return null;
  const onlyStrings = (v: unknown): string[] =>
    Array.isArray(v) ? v.filter((x): x is string => typeof x === 'string') : [];
  return {
    message: error.message,
    schemaName: typeof body.schema_name === 'string' ? body.schema_name : '',
    unknownFields: onlyStrings(body.unknown_fields),
    availableFields: onlyStrings(body.available_fields),
  };
}

function QueryTab({ onResult }: QueryTabProps) {
  const orgNames = useOrgNames()
  // Query state management
  const {
    state: queryState,
    handleSchemaChange,
    toggleField: handleFieldToggle,
    handleFieldValueChange,
    handleRangeFilterChange,
    setRangeSchemaFilter,
    setHashKeyValue,
    clearState,
    refetchSchemas,
    approvedSchemas,
    schemasLoading,
    selectedSchemaObj,
    isRangeSchema,
    isHashRangeSchema,
    rangeKey
  } = useQueryState();

  // Fetch schema states from backend when tab is activated
  useEffect(() => {
    refetchSchemas();
  }, [refetchSchemas]);

  // Parse schema parameter from URL hash on mount (set by SchemaTab Query button)
  useEffect(() => {
    const hash = window.location.hash;
    const match = hash.match(/[?&]schema=([^&]+)/);
    if (match) {
      const schemaName = decodeURIComponent(match[1]);
      handleSchemaChange(schemaName);
      // Clean up the URL
      window.location.hash = 'query';
    }
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  // Execution state management
  const [isExecuting, setIsExecuting] = useState(false);
  const [unknownFieldsError, setUnknownFieldsError] = useState<UnknownFieldsError | null>(null);

  // Use the extracted query builder for query construction
  const { query, isValid } = useQueryBuilder({
    schema: queryState.selectedSchema,
    queryState,
    schemas: selectedSchemaObj
      ? { [queryState.selectedSchema]: selectedSchemaObj }
      : {},
  });

  /**
   * Handle query execution - follows original QueryTab pattern
   */
  const handleExecuteQuery = useCallback(async (queryData: Record<string, unknown> | null) => {
    // Backend handles all validation
    if (!queryData) {
      onResult({
        error: 'No query data provided'
      });
      return;
    }

    setIsExecuting(true);
    try {
      const response = await mutationClient.executeQuery(queryData);

      if (!response.success) {
        console.error('Query failed:', response.error);
        onResult({
          error: response.error || 'Query execution failed',
          details: response
        });
        return;
      }

      setUnknownFieldsError(null);
      // Pass the actual query data from response.data
      // API returns { ok: true, results: [...] } in data, extract results array
      const data = response.data as { results?: unknown } | undefined
      onResult({
        success: true,
        data: data?.results ?? response.data
      });
    } catch (error) {
      console.error('Failed to execute query:', error);
      const ufe = extractUnknownFieldsError(error);
      if (ufe) {
        setUnknownFieldsError(ufe);
        onResult({
          error: ufe.message,
          details: error instanceof ApiError ? error.response : error
        });
      } else if (error instanceof ApiError) {
        setUnknownFieldsError(null);
        // Distinguish HTTP errors from real network failures — the previous
        // "Network error: ..." prefix mis-categorized every 4xx/5xx response.
        const message = error.isNetworkError
          ? `Network error: ${error.message}`
          : error.toUserMessage();
        onResult({ error: message, details: error });
      } else {
        setUnknownFieldsError(null);
        onResult({
          error: `Network error: ${error instanceof Error ? error.message : String(error)}`,
          details: error
        });
      }
    } finally {
      setIsExecuting(false);
    }
  }, [onResult]);

  const handleAvailableFieldClick = useCallback((fieldName: string) => {
    handleFieldToggle(fieldName);
  }, [handleFieldToggle]);

  const handleClearWithReset = useCallback(() => {
    setUnknownFieldsError(null);
    clearState();
  }, [clearState]);


  // UI does not require authentication

  // Collapse the Query Preview pane until a schema is picked. Before any
  // selection the preview was a 350px-wide skeleton ("Schema [empty bar]
  // / Fields (0): No fields selected") which read as broken. After
  // selection the form has real content to mirror, so the side-by-side
  // layout earns its keep.
  const hasSchema = !!selectedSchemaObj
  return (
    <div>
      <div className={`grid grid-cols-1 ${hasSchema ? 'lg:grid-cols-3' : ''} gap-6`}>
        {/* Main Query Form */}
        <div className={hasSchema ? 'lg:col-span-2 space-y-6' : 'space-y-6'}>
          <QueryForm
            queryState={queryState}
            onSchemaChange={handleSchemaChange}
            onFieldToggle={handleFieldToggle}
            onFieldValueChange={handleFieldValueChange as (field: string, value: unknown) => void}
            onRangeFilterChange={handleRangeFilterChange as unknown as (filter: unknown) => void}
            onRangeSchemaFilterChange={setRangeSchemaFilter as unknown as (filter: unknown) => void}
            onHashKeyChange={setHashKeyValue}
            approvedSchemas={approvedSchemas}
            orgNames={orgNames}
            schemasLoading={schemasLoading}
            isRangeSchema={isRangeSchema}
            isHashRangeSchema={isHashRangeSchema}
            rangeKey={rangeKey}
          />

          {unknownFieldsError && (
            <div
              className="p-3 bg-gruvbox-red/10 border border-gruvbox-red/20 rounded-lg"
              data-testid="unknown-fields-error"
              role="alert"
            >
              <div className="text-sm text-gruvbox-red">{unknownFieldsError.message}</div>
              {unknownFieldsError.availableFields.length > 0 && (
                <div className="mt-2">
                  <div className="text-xs text-secondary mb-1">Click to add:</div>
                  <div className="flex flex-wrap gap-2" data-testid="available-fields-chips">
                    {unknownFieldsError.availableFields.map(field => (
                      <button
                        key={field}
                        type="button"
                        onClick={() => handleAvailableFieldClick(field)}
                        className="px-2 py-0.5 text-xs rounded-full border border-border hover:border-gruvbox-blue hover:bg-gruvbox-blue/10 transition-colors bg-transparent text-primary cursor-pointer"
                        data-testid={`available-field-chip-${field}`}
                      >
                        + {field}
                      </button>
                    ))}
                  </div>
                </div>
              )}
            </div>
          )}

          {/* Query Actions */}
          <QueryActions
            onExecute={() => handleExecuteQuery(query)}
            onClear={handleClearWithReset}
            queryData={query}
            disabled={!isValid}
            isExecuting={isExecuting}
            showValidation={false}
            showSave={false}
            showClear={true}
          />
        </div>

        {/* Query Preview Sidebar — only render once a schema is chosen */}
        {hasSchema && (
          <div className="lg:col-span-1">
            <QueryPreview
              query={query}
              queryState={{ schema: getSchemaDisplayName(selectedSchemaObj) }}
              showJson={false} // Can be toggled for debugging
              title="Query Preview"
            />
          </div>
        )}
      </div>
    </div>
  );
}

export default QueryTab