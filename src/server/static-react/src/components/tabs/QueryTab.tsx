/**
 * QueryTab Component
 * Orchestrates query form, actions, preview, and execution.
 */

import { useCallback, useState, useEffect } from 'react';
import { mutationClient } from '../../api/clients/mutationClient';
import { useQueryState } from '../../hooks/useQueryState';
import { useQueryBuilder } from '../../hooks/useQueryBuilder';
import QueryForm from '../query/QueryForm';
import QueryActions from '../query/QueryActions';
import QueryPreview from '../query/QueryPreview';
import { getSchemaDisplayName } from '../../utils/schemaUtils';
import { useOrgNames } from '../../hooks/useOrgNames';

function QueryTab({ onResult }) {
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

  // Use the extracted query builder for query construction
  const { query, isValid } = useQueryBuilder({
    schema: queryState.selectedSchema,
    queryState,
    schemas: { [queryState.selectedSchema]: selectedSchemaObj }
  });

  /**
   * Handle query execution - follows original QueryTab pattern
   */
  const handleExecuteQuery = useCallback(async (queryData) => {
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
      
      // Pass the actual query data from response.data
      // API returns { ok: true, results: [...] } in data, extract results array
      onResult({
        success: true,
        data: response.data?.results || response.data
      });
    } catch (error) {
      console.error('Failed to execute query:', error);
      onResult({
        error: `Network error: ${error instanceof Error ? error.message : String(error)}`,
        details: error
      });
    } finally {
      setIsExecuting(false);
    }
  }, [onResult]);


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
            onFieldValueChange={handleFieldValueChange}
            onRangeFilterChange={handleRangeFilterChange}
            onRangeSchemaFilterChange={setRangeSchemaFilter}
            onHashKeyChange={setHashKeyValue}
            approvedSchemas={approvedSchemas}
            orgNames={orgNames}
            schemasLoading={schemasLoading}
            isRangeSchema={isRangeSchema}
            isHashRangeSchema={isHashRangeSchema}
            rangeKey={rangeKey}
          />

          {/* Query Actions */}
          <QueryActions
            onExecute={() => handleExecuteQuery(query)}
            onClear={clearState}
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