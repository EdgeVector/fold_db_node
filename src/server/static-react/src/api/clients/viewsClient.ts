// Views API Client

import { getSharedClient } from '../core/client';

export interface ViewState {
  Available: undefined;
  Approved: undefined;
  Blocked: undefined;
}

export interface TransformView {
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

export interface ViewListResponse {
  views: ViewWithState[];
  count: number;
}

const client = () => getSharedClient();

export async function listViews(): Promise<ViewWithState[]> {
  const resp = await client().get<ViewListResponse>('/api/views');
  if (!resp.ok) throw new Error(resp.error || 'Failed to list views');
  return resp.data?.views ?? [];
}

export async function getView(name: string): Promise<TransformView> {
  const resp = await client().get<{ view: TransformView }>(`/api/view/${encodeURIComponent(name)}`);
  if (!resp.ok) throw new Error(resp.error || `Failed to get view: ${name}`);
  return resp.data!.view;
}

export interface CreateViewRequest {
  name: string;
  schema_type: string;
  key_config?: { hash_field?: string; range_field?: string } | null;
  input_queries: Array<{
    schema_name: string;
    fields: string[];
  }>;
  wasm_transform?: string | null; // base64
  output_fields: Record<string, string>;
}

export async function createView(req: CreateViewRequest): Promise<void> {
  const resp = await client().post<{ success: boolean }>('/api/view', req);
  if (!resp.ok) throw new Error(resp.error || 'Failed to create view');
}

export async function approveView(name: string): Promise<void> {
  const resp = await client().post<{ approved: boolean }>(`/api/view/${encodeURIComponent(name)}/approve`, {});
  if (!resp.ok) throw new Error(resp.error || `Failed to approve view: ${name}`);
}

export async function blockView(name: string): Promise<void> {
  const resp = await client().post<{ success: boolean }>(`/api/view/${encodeURIComponent(name)}/block`, {});
  if (!resp.ok) throw new Error(resp.error || `Failed to block view: ${name}`);
}

export async function deleteView(name: string): Promise<void> {
  const resp = await client().delete<{ success: boolean }>(`/api/view/${encodeURIComponent(name)}`);
  if (!resp.ok) throw new Error(resp.error || `Failed to delete view: ${name}`);
}
