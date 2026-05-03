import { useMemo, type ReactNode } from 'react';
import { useQueryBuilder } from '../../hooks/useQueryBuilder';
import type { Schema } from '../../types/schema';

const detectRangeSchema = (schema: Schema | null | undefined): boolean => {
  return (schema as { schema_type?: string } | null | undefined)?.schema_type === 'Range';
};

const extractRangeKey = (schema: Schema | null | undefined): string | null => {
  return (schema as { key?: { range_field?: string } } | null | undefined)?.key?.range_field || null;
};

interface QueryBuilderResult {
  query: unknown;
  isValid: boolean;
  validationErrors: string[];
}

interface QueryBuilderProps {
  children: ((result: QueryBuilderResult) => ReactNode) | ReactNode;
  queryState?: { selectedSchema?: string } & Record<string, unknown>;
  schemas?: Record<string, Schema>;
  selectedSchemaObj?: Schema | null;
  isRangeSchema?: boolean;
  rangeKey?: string;
  schema?: string;
  [key: string]: unknown;
}

function QueryBuilder({
  children,
  queryState,
  schemas,
  selectedSchemaObj,
  isRangeSchema,
  rangeKey,
  schema,
  ...rest
}: QueryBuilderProps) {
  const resolvedSchema = useMemo(() => {
    if (schema) {
      return schema;
    }

    if (queryState?.selectedSchema) {
      return queryState.selectedSchema;
    }

    return (selectedSchemaObj as { name?: string } | null | undefined)?.name ?? null;
  }, [schema, queryState?.selectedSchema, selectedSchemaObj]);

  const resolvedSchemaObj = useMemo(() => {
    if (selectedSchemaObj) {
      return selectedSchemaObj;
    }

    if (resolvedSchema && schemas && schemas[resolvedSchema]) {
      return schemas[resolvedSchema];
    }

    return null;
  }, [resolvedSchema, schemas, selectedSchemaObj]);

  const resolvedSchemas = useMemo(() => {
    if (schemas) {
      return schemas;
    }

    if (resolvedSchema && resolvedSchemaObj) {
      return { [resolvedSchema]: resolvedSchemaObj } as Record<string, Schema>;
    }

    return undefined;
  }, [schemas, resolvedSchema, resolvedSchemaObj]);

  const resolvedIsRangeSchema = useMemo(() => {
    if (typeof isRangeSchema === 'boolean') {
      return isRangeSchema;
    }

    return detectRangeSchema(resolvedSchemaObj);
  }, [isRangeSchema, resolvedSchemaObj]);

  const resolvedRangeKey = useMemo(() => {
    if (rangeKey) {
      return rangeKey;
    }

    return extractRangeKey(resolvedSchemaObj);
  }, [rangeKey, resolvedSchemaObj]);

  const hookArguments = useMemo(() => ({
    ...rest,
    schema: resolvedSchema ?? undefined,
    queryState: queryState as Parameters<typeof useQueryBuilder>[0]['queryState'],
    schemas: resolvedSchemas,
    selectedSchemaObj: resolvedSchemaObj ?? undefined,
    isRangeSchema: resolvedIsRangeSchema,
    rangeKey: resolvedRangeKey as string | undefined,
  }), [rest, resolvedSchema, queryState, resolvedSchemas, resolvedSchemaObj, resolvedIsRangeSchema, resolvedRangeKey]);

  const queryBuilder = useQueryBuilder(hookArguments);

  if (typeof children === 'function') {
    return <>{(children as (result: QueryBuilderResult) => ReactNode)(queryBuilder)}</>;
  }

  return null;
}

export default QueryBuilder;
// eslint-disable-next-line react-refresh/only-export-components
export { useQueryBuilder, QueryBuilder };
