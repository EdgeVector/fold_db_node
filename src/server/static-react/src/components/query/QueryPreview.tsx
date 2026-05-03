import { useMemo } from 'react';

type FilterDescriptor = {
  exactKey?: string;
  keyRange?: string;
  keyPrefix?: string;
};

interface FormattedQuery {
  schema?: string;
  fields: string[];
  fieldValues: Record<string, unknown>;
  filters: Record<string, FilterDescriptor> | Array<{ field: string; operator: string; value: string }>;
  orderBy?: { field: string; direction: string };
  rangeKey?: string;
}

interface QueryLike {
  schema?: string;
  schema_name?: string;
  selectedSchema?: string;
  fields?: string[] | Record<string, unknown>;
  fieldValues?: Record<string, unknown>;
  queryFields?: string[];
  filters?: Record<string, FilterDescriptor> | Array<{ field: string; operator: string; value: string }>;
  orderBy?: { field: string; direction: string };
  rangeKey?: string;
  filter?: {
    field?: string;
    range_filter?: Record<string, unknown> | { Key?: string; KeyRange?: { start: string; end: string }; KeyPrefix?: string };
    [variantKey: string]: unknown;
  };
}

const formatQueryDisplay = (
  query: QueryLike | null | undefined,
  queryState: QueryLike | null | undefined,
): FormattedQuery | null => {
  if (!query && !queryState) return null;

  const combined: QueryLike = { ...query, ...queryState };

  let fields: string[] = [];
  let fieldValues: Record<string, unknown> = {};

  if (Array.isArray(combined.fields)) {
    fields = combined.fields;
  } else if (combined.fields && typeof combined.fields === 'object') {
    fields = Object.keys(combined.fields);
    fieldValues = combined.fields as Record<string, unknown>;
  } else if (combined.queryFields && Array.isArray(combined.queryFields)) {
    fields = combined.queryFields;
  }

  if (combined.fieldValues && typeof combined.fieldValues === 'object') {
    fieldValues = { ...fieldValues, ...combined.fieldValues };
  }

  const display: FormattedQuery = {
    schema: combined.schema || combined.schema_name || combined.selectedSchema,
    fields,
    fieldValues,
    filters: combined.filters || {},
    orderBy: combined.orderBy,
    rangeKey: combined.rangeKey,
  };

  if (query && query.filter) {
    const filtersObj = display.filters as Record<string, FilterDescriptor>;
    if (query.filter.field && query.filter.range_filter) {
      const fieldName = query.filter.field;
      const filter = query.filter.range_filter as { Key?: string; KeyRange?: { start: string; end: string }; KeyPrefix?: string };

      if (filter.Key) {
        filtersObj[fieldName] = { exactKey: filter.Key };
      } else if (filter.KeyRange) {
        filtersObj[fieldName] = {
          keyRange: `${filter.KeyRange.start} → ${filter.KeyRange.end}`
        };
      } else if (filter.KeyPrefix) {
        filtersObj[fieldName] = { keyPrefix: filter.KeyPrefix };
      }
    } else if (query.filter.range_filter) {
      Object.entries(query.filter.range_filter).forEach(([key, filter]) => {
        if (typeof filter === 'string') {
          filtersObj[key] = { exactKey: filter };
        } else if (filter && typeof filter === 'object') {
          const f = filter as { KeyRange?: { start: string; end: string }; KeyPrefix?: string };
          if (f.KeyRange) {
            filtersObj[key] = {
              keyRange: `${f.KeyRange.start} → ${f.KeyRange.end}`
            };
          } else if (f.KeyPrefix) {
            filtersObj[key] = { keyPrefix: f.KeyPrefix };
          }
        }
      });
    }
  }

  return display;
};

interface QueryPreviewProps {
  query?: QueryLike | null;
  queryState?: QueryLike | null;
  validationErrors?: string[];
  isExecuting?: boolean;
  showJson?: boolean;
  collapsible?: boolean;
  className?: string;
  title?: string;
}

function QueryPreview({
  query,
  queryState,
  validationErrors = [],
  isExecuting = false,
  showJson = false,
  collapsible: _collapsible = true,
  className = '',
  title = 'Query Preview'
}: QueryPreviewProps) {
  const formattedQuery = useMemo(() => formatQueryDisplay(query, queryState), [query, queryState]);

  if (!query && !queryState) {
    return (
      <div className={`card p-4 ${className}`}>
        <h3 className="text-sm font-medium text-secondary mb-2">{title}</h3>
        <p className="text-sm text-tertiary italic">No query to preview</p>
      </div>
    );
  }

  return (
    <div className={`card ${className}`}>
      <div className="px-4 py-3 border-b border-border">
        <h3 className="text-sm font-medium text-primary">{title}</h3>
      </div>

      <div className="p-4 space-y-4">
        {validationErrors && validationErrors.length > 0 && (
          <div className="card card-error p-3">
            <div className="flex items-center mb-2">
              <svg className="h-4 w-4 text-gruvbox-red mr-2" fill="currentColor" viewBox="0 0 20 20">
                <path fillRule="evenodd" d="M18 10a8 8 0 11-16 0 8 8 0 0116 0zm-7 4a1 1 0 11-2 0 1 1 0 012 0zm-1-9a1 1 0 00-1 1v4a1 1 0 102 0V6a1 1 0 00-1-1z" clipRule="evenodd" />
              </svg>
              <span className="text-sm font-medium text-gruvbox-red">Validation Errors</span>
            </div>
            <ul className="space-y-1">
              {validationErrors.map((error) => (
                <li key={error} className="text-sm text-gruvbox-red">
                  {error}
                </li>
              ))}
            </ul>
          </div>
        )}

        {isExecuting && (
          <div className="card card-info p-3">
            <div className="flex items-center">
              <svg className="animate-spin h-4 w-4 text-gruvbox-blue mr-2" fill="none" viewBox="0 0 24 24">
                <circle className="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="4"></circle>
                <path className="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4zm2 5.291A7.962 7.962 0 014 12H0c0 3.042 1.135 5.824 3 7.938l3-2.647z"></path>
              </svg>
              <span className="text-sm font-medium text-gruvbox-blue">Executing query...</span>
            </div>
          </div>
        )}

        <div className="space-y-3">
          <div>
            <label className="block text-xs font-medium text-secondary mb-1">
              Schema
            </label>
            <div className="inline-flex items-center badge badge-info text-sm font-medium">
              {formattedQuery?.schema || ''}
            </div>
          </div>

          <div>
            <label className="block text-xs font-medium text-secondary mb-1">
              Fields ({formattedQuery?.fields ? formattedQuery.fields.length : 0})
            </label>
            <div className="flex flex-wrap gap-1">
              {formattedQuery?.fields && formattedQuery.fields.length > 0 ? (
                formattedQuery.fields.map((field) => {
                  const fieldValue = formattedQuery.fieldValues?.[field];
                  return (
                    <div key={field} className="inline-flex flex-col items-start">
                      <span className="inline-flex items-center badge badge-success text-sm">
                        {field}
                      </span>
                      {typeof fieldValue !== 'undefined' && fieldValue !== null && (
                        <span className="text-xs text-secondary mt-1 px-2">
                          {String(fieldValue)}
                        </span>
                      )}
                    </div>
                  );
                })
              ) : (
                <span className="text-sm text-secondary italic">No fields selected</span>
              )}
            </div>
          </div>

          {formattedQuery && (
            (Array.isArray(formattedQuery.filters) && formattedQuery.filters.length > 0) ||
            (!Array.isArray(formattedQuery.filters) && Object.keys(formattedQuery.filters).length > 0)
          ) && (
            <div>
              <label className="block text-xs font-medium text-secondary mb-1">
                Filters
              </label>
              <div className="space-y-2">
                {Array.isArray(formattedQuery.filters) ? (
                  formattedQuery.filters.map((filter) => (
                    <div key={`${filter.field}-${filter.operator}-${filter.value}`} className="card card-warning p-3">
                      <div className="text-sm text-gruvbox-yellow">
                        {filter.field} {filter.operator} &quot;{filter.value}&quot;
                      </div>
                    </div>
                  ))
                ) : (
                  Object.entries(formattedQuery.filters).map(([fieldName, filter]) => (
                    <div key={fieldName} className="card card-warning p-3">
                      <div className="font-medium text-sm text-gruvbox-yellow mb-1">
                        {fieldName}
                      </div>
                      <div className="text-sm text-gruvbox-yellow">
                        {filter.exactKey && (
                          <span>Exact key: <code className="bg-gruvbox-elevated px-1 rounded">{filter.exactKey}</code></span>
                        )}
                        {filter.keyRange && (
                          <span>Key range: <code className="bg-gruvbox-elevated px-1 rounded">{filter.keyRange}</code></span>
                        )}
                        {filter.keyPrefix && (
                          <span>Key prefix: <code className="bg-gruvbox-elevated px-1 rounded">{filter.keyPrefix}</code></span>
                        )}
                      </div>
                    </div>
                  ))
                )}
              </div>
            </div>
          )}

          {formattedQuery?.orderBy && (
            <div>
              <label className="block text-xs font-medium text-secondary mb-1">
                OrderBy
              </label>
              <div className="card card-info p-3">
                <div className="text-sm text-gruvbox-blue">
                  {formattedQuery.orderBy.field} {formattedQuery.orderBy.direction}
                </div>
              </div>
            </div>
          )}

          {formattedQuery?.rangeKey && (
            <div>
              <label className="block text-xs font-medium text-secondary mb-1">
                RangeKey
              </label>
              <div className="bg-gruvbox-elevated border border-border p-3">
                <div className="text-sm text-gruvbox-blue">
                  <code className="bg-gruvbox-elevated px-1">{formattedQuery.rangeKey}</code>
                </div>
              </div>
            </div>
          )}
        </div>

        {showJson && (
          <div className="pt-4 border-t border-border">
            <label className="block text-xs font-medium text-secondary uppercase tracking-wide mb-2">
              Raw JSON
            </label>
            <pre className="text-xs p-3 overflow-x-auto bg-surface-secondary text-primary">
              {JSON.stringify(query, null, 2)}
            </pre>
          </div>
        )}
      </div>
    </div>
  );
}

export default QueryPreview;
// eslint-disable-next-line react-refresh/only-export-components
export { QueryPreview, formatQueryDisplay };
