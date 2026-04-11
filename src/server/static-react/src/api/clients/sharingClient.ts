// Sharing & Trust Management API Client

import { getSharedClient } from '../core/client';

// ===== Types =====

export type TrustTier = "Public" | "Outer" | "Trusted" | "Inner" | "Owner";

export interface TrustGrantEntry {
  public_key: string;
  tier: TrustTier;
}

export interface TrustGrantsResponse {
  grants: TrustGrantEntry[];
}

export interface TrustResolveResponse {
  public_key: string;
  tier: TrustTier | null;
}

export interface FieldAccessPolicy {
  trust_domain: string;
  min_read_tier: TrustTier;
  min_write_tier: TrustTier;
  capabilities: CapabilityConstraint[];
}

export interface CapabilityConstraint {
  public_key: string;
  remaining_quota: number;
  kind: 'Read' | 'Write';
}

export interface SecurityLabel {
  level: number;
  category: string;
}

export interface AuditEvent {
  id: string;
  timestamp: string;
  user_id: string;
  action: Record<string, unknown>;
  trust_tier: TrustTier | null;
  decision_granted: boolean;
}

export interface AuditLogResponse {
  events: AuditEvent[];
  count: number;
}

export interface NodeInfoResponse {
  public_key: string;
  node_id: string;
  shared_schemas: string[];
}

// ===== Client =====

const client = () => getSharedClient();

// --- Trust management ---

export async function grantTrust(public_key: string, role: string): Promise<void> {
  const resp = await client().post<{ granted: boolean }>('/trust/grant', { public_key, role });
  if (!resp.success) throw new Error(resp.error || 'Failed to grant trust');
}

export async function revokeTrust(public_key: string): Promise<void> {
  const resp = await client().delete<{ revoked: boolean }>(`/trust/revoke/${encodeURIComponent(public_key)}`);
  if (!resp.success) throw new Error(resp.error || 'Failed to revoke trust');
}

export async function listTrustGrants(): Promise<TrustGrantEntry[]> {
  const resp = await client().get<TrustGrantsResponse>('/trust/grants', { cacheable: false });
  if (!resp.success) throw new Error(resp.error || 'Failed to list trust grants');
  return resp.data?.grants ?? [];
}

export async function resolveTrust(public_key: string): Promise<TrustTier | null> {
  const resp = await client().get<TrustResolveResponse>(`/trust/resolve/${encodeURIComponent(public_key)}`);
  if (!resp.success) throw new Error(resp.error || 'Failed to resolve trust');
  return resp.data?.tier ?? null;
}

// --- Field policies ---

export async function setFieldPolicy(
  schemaName: string,
  fieldName: string,
  policy: FieldAccessPolicy
): Promise<void> {
  const resp = await client().put<{ policy_set: boolean }>(
    `/schema/${encodeURIComponent(schemaName)}/field/${encodeURIComponent(fieldName)}/policy`,
    { policy }
  );
  if (!resp.success) throw new Error(resp.error || 'Failed to set field policy');
}

export async function getFieldPolicy(
  schemaName: string,
  fieldName: string
): Promise<FieldAccessPolicy | null> {
  const resp = await client().get<{
    schema_name: string;
    field_name: string;
    policy: FieldAccessPolicy | null;
  }>(`/schema/${encodeURIComponent(schemaName)}/field/${encodeURIComponent(fieldName)}/policy`);
  if (!resp.success) throw new Error(resp.error || 'Failed to get field policy');
  return resp.data?.policy ?? null;
}

// --- Bulk field policies ---

export async function getAllFieldPolicies(
  schemaName: string
): Promise<Record<string, FieldAccessPolicy | null>> {
  const resp = await client().get<{
    schema_name: string;
    field_policies: Record<string, FieldAccessPolicy | null>;
  }>(`/schema/${encodeURIComponent(schemaName)}/policies`);
  if (!resp.success) throw new Error(resp.error || 'Failed to get field policies');
  return resp.data?.field_policies ?? {};
}

// --- Audit log ---

export async function getAuditLog(limit: number = 100): Promise<AuditEvent[]> {
  const resp = await client().get<AuditLogResponse>(`/trust/audit?limit=${limit}`);
  if (!resp.success) throw new Error(resp.error || 'Failed to get audit log');
  return resp.data?.events ?? [];
}

// --- Node info ---

export async function getNodeInfo(): Promise<NodeInfoResponse> {
  const resp = await client().get<NodeInfoResponse>('/remote/node-info');
  if (!resp.success) throw new Error(resp.error || 'Failed to get node info');
  return resp.data!;
}

