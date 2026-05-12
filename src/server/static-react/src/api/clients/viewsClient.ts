// Views API Client

import { getSharedClient } from '../core/client';

interface TransformView {
  name: string;
  schema_type: string;
  key_config?: { hash_field?: string; range_field?: string } | null;
  input_queries: Array<{
    schema_name: string;
    fields: string[];
    filter?: unknown;
  }>;
  wasm_transform?: number[] | null;
  output_fields: Record<string, unknown>;
}

export type ViewWithState = [TransformView, string];

interface ViewListResponse {
  views: ViewWithState[];
  count: number;
}

const client = () => getSharedClient();

export async function listViews(): Promise<ViewWithState[]> {
  const resp = await client().get<ViewListResponse>('/views');
  if (!resp.success) throw new Error(resp.error || 'Failed to list views');
  return resp.data?.views ?? [];
}

export async function approveView(name: string): Promise<void> {
  const resp = await client().post<{ approved: boolean }>(`/view/${encodeURIComponent(name)}/approve`, {});
  if (!resp.success) throw new Error(resp.error || `Failed to approve view: ${name}`);
}

export async function blockView(name: string): Promise<void> {
  const resp = await client().post<{ success: boolean }>(`/view/${encodeURIComponent(name)}/block`, {});
  if (!resp.success) throw new Error(resp.error || `Failed to block view: ${name}`);
}

export async function deleteView(name: string): Promise<void> {
  const resp = await client().delete<{ success: boolean }>(`/view/${encodeURIComponent(name)}`);
  if (!resp.success) throw new Error(resp.error || `Failed to delete view: ${name}`);
}
