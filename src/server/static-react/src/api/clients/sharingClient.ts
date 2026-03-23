// Sharing & Trust Management API Client

import { getSharedClient } from '../core/client';

// ===== Types =====

export interface TrustGrantEntry {
  public_key: string;
  distance: number;
}

export interface TrustGrantsResponse {
  grants: TrustGrantEntry[];
}

export interface TrustResolveResponse {
  public_key: string;
  distance: number | null;
}

export interface FieldAccessPolicy {
  trust_distance: {
    read_max: number;
    write_max: number;
  };
  capabilities: CapabilityConstraint[];
  security_label: SecurityLabel | null;
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

export interface PaymentGate {
  Linear?: { base: number; per_distance: number };
  Exponential?: { base: number; growth: number };
  Fixed?: number;
}

export interface AuditEvent {
  id: string;
  timestamp: string;
  user_id: string;
  action: Record<string, unknown>;
  trust_distance: number | null;
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

export async function grantTrust(public_key: string, distance: number): Promise<void> {
  const resp = await client().post<{ granted: boolean }>('/api/trust/grant', { public_key, distance });
  if (!resp.ok) throw new Error(resp.error || 'Failed to grant trust');
}

export async function revokeTrust(public_key: string): Promise<void> {
  const resp = await client().delete<{ revoked: boolean }>(`/api/trust/revoke/${encodeURIComponent(public_key)}`);
  if (!resp.ok) throw new Error(resp.error || 'Failed to revoke trust');
}

export async function listTrustGrants(): Promise<TrustGrantEntry[]> {
  const resp = await client().get<TrustGrantsResponse>('/api/trust/grants');
  if (!resp.ok) throw new Error(resp.error || 'Failed to list trust grants');
  return resp.data?.grants ?? [];
}

export async function setTrustOverride(public_key: string, distance: number): Promise<void> {
  const resp = await client().put<{ override_set: boolean }>('/api/trust/override', { public_key, distance });
  if (!resp.ok) throw new Error(resp.error || 'Failed to set trust override');
}

export async function resolveTrust(public_key: string): Promise<number | null> {
  const resp = await client().get<TrustResolveResponse>(`/api/trust/resolve/${encodeURIComponent(public_key)}`);
  if (!resp.ok) throw new Error(resp.error || 'Failed to resolve trust');
  return resp.data?.distance ?? null;
}

// --- Field policies ---

export async function setFieldPolicy(
  schemaName: string,
  fieldName: string,
  policy: FieldAccessPolicy
): Promise<void> {
  const resp = await client().put<{ policy_set: boolean }>(
    `/api/schema/${encodeURIComponent(schemaName)}/field/${encodeURIComponent(fieldName)}/policy`,
    { policy }
  );
  if (!resp.ok) throw new Error(resp.error || 'Failed to set field policy');
}

export async function getFieldPolicy(
  schemaName: string,
  fieldName: string
): Promise<FieldAccessPolicy | null> {
  const resp = await client().get<{
    schema_name: string;
    field_name: string;
    policy: FieldAccessPolicy | null;
  }>(`/api/schema/${encodeURIComponent(schemaName)}/field/${encodeURIComponent(fieldName)}/policy`);
  if (!resp.ok) throw new Error(resp.error || 'Failed to get field policy');
  return resp.data?.policy ?? null;
}

// --- Bulk field policies ---

export async function getAllFieldPolicies(
  schemaName: string
): Promise<Record<string, FieldAccessPolicy | null>> {
  const resp = await client().get<{
    schema_name: string;
    field_policies: Record<string, FieldAccessPolicy | null>;
  }>(`/api/schema/${encodeURIComponent(schemaName)}/policies`);
  if (!resp.ok) throw new Error(resp.error || 'Failed to get field policies');
  return resp.data?.field_policies ?? {};
}

// --- Payment gates ---

export async function setPaymentGate(schemaName: string, gate: PaymentGate): Promise<void> {
  const resp = await client().put<{ payment_gate_set: boolean }>(
    `/api/schema/${encodeURIComponent(schemaName)}/payment-gate`,
    { gate }
  );
  if (!resp.ok) throw new Error(resp.error || 'Failed to set payment gate');
}

export async function getPaymentGate(schemaName: string): Promise<PaymentGate | null> {
  const resp = await client().get<{ schema_name: string; payment_gate: PaymentGate | null }>(
    `/api/schema/${encodeURIComponent(schemaName)}/payment-gate`
  );
  if (!resp.ok) throw new Error(resp.error || 'Failed to get payment gate');
  return resp.data?.payment_gate ?? null;
}

// --- Audit log ---

export async function getAuditLog(limit: number = 100): Promise<AuditEvent[]> {
  const resp = await client().get<AuditLogResponse>(`/api/trust/audit?limit=${limit}`);
  if (!resp.ok) throw new Error(resp.error || 'Failed to get audit log');
  return resp.data?.events ?? [];
}

// --- Node info ---

export async function getNodeInfo(): Promise<NodeInfoResponse> {
  const resp = await client().get<NodeInfoResponse>('/api/remote/node-info');
  if (!resp.ok) throw new Error(resp.error || 'Failed to get node info');
  return resp.data!;
}

// --- Capabilities ---

export async function issueCapability(
  schema_name: string,
  field_name: string,
  public_key: string,
  kind: 'Read' | 'Write',
  quota: number
): Promise<void> {
  const resp = await client().post<{ issued: boolean }>('/api/capabilities/issue', {
    schema_name,
    field_name,
    public_key,
    kind,
    quota,
  });
  if (!resp.ok) throw new Error(resp.error || 'Failed to issue capability');
}

export async function listCapabilities(
  schemaName: string,
  fieldName: string
): Promise<CapabilityConstraint[]> {
  const resp = await client().get<CapabilityConstraint[]>(
    `/api/capabilities/list/${encodeURIComponent(schemaName)}/${encodeURIComponent(fieldName)}`
  );
  if (!resp.ok) throw new Error(resp.error || 'Failed to list capabilities');
  return resp.data ?? [];
}
