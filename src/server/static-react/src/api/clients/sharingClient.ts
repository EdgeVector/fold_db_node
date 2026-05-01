// Sharing & Trust Management API Client

import { createApiClient, getSharedClient } from '../core/client';
import type { EnhancedApiResponse } from '../core/types';
import type {
  TrustTier,
  TrustGrantEntry,
  TrustGrantsResponse,
  TrustResolveResponse,
  AuditEvent,
  AuditLogResponse,
} from './trustClient';

export type { TrustTier, TrustGrantEntry, TrustGrantsResponse, TrustResolveResponse, AuditEvent, AuditLogResponse };

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

// ===== Cross-user sharing (rules, invites, subscriptions) =====
//
// Mirrors the Rust structs in fold_db::sharing::types. The HTTP layer
// (src/handlers/sharing.rs + src/server/routes/sharing.rs) wraps the
// payloads in the standard { ok, <field>, user_hash } envelope.

/**
 * Scope of a share rule. Matches Rust's `ShareScope` enum, which is
 * serialized with serde's default enum tagging:
 *   - `"AllSchemas"`                               (unit variant)
 *   - `{ "Schema": "schema_name" }`                (tuple variant, 1 arg)
 *   - `{ "SchemaField": ["schema", "field"] }`     (tuple variant, 2 args)
 */
export type ShareScope =
  | 'AllSchemas'
  | { Schema: string }
  | { SchemaField: [string, string] };

export interface ShareRule {
  rule_id: string;
  recipient_pubkey: string;
  recipient_display_name: string;
  scope: ShareScope;
  share_prefix: string;
  /** Raw E2E secret bytes (serde serializes Vec<u8> as number[]). */
  share_e2e_secret: number[];
  active: boolean;
  created_at: number;
  writer_pubkey: string;
  signature: string;
}

export interface ShareInvite {
  sender_pubkey: string;
  sender_display_name: string;
  share_prefix: string;
  share_e2e_secret: number[];
  scope_description: string;
}

export interface ShareSubscription {
  sender_pubkey: string;
  share_prefix: string;
  share_e2e_secret: number[];
  accepted_at: number;
  active: boolean;
}

export interface CreateRuleRequest {
  recipient_pubkey: string;
  recipient_display_name: string;
  scope: ShareScope;
}

export interface GenerateInviteRequest {
  rule_id: string;
  scope_description: string;
}

export interface ListShareRulesResponse {
  ok: boolean;
  rules: ShareRule[];
  user_hash?: string;
}

export interface ShareRuleResponse {
  ok: boolean;
  rule: ShareRule;
  user_hash?: string;
}

export interface OkResponse {
  ok: boolean;
  user_hash?: string;
}

export interface ShareInviteResponse {
  ok: boolean;
  invite: ShareInvite;
  user_hash?: string;
}

export interface AcceptShareInviteResponse {
  ok: boolean;
  subscription: ShareSubscription;
  user_hash?: string;
}

export interface PendingInvitesResponse {
  ok: boolean;
  invites: ShareInvite[];
  user_hash?: string;
}

// Use the same client factory as other per-domain clients.
const sharingApi = createApiClient({
  enableCache: false,
  enableLogging: true,
  enableMetrics: true,
});

export async function listShareRules(): Promise<
  EnhancedApiResponse<ListShareRulesResponse>
> {
  return sharingApi.get<ListShareRulesResponse>('/sharing/rules');
}

export async function createShareRule(
  req: CreateRuleRequest,
): Promise<EnhancedApiResponse<ShareRuleResponse>> {
  return sharingApi.post<ShareRuleResponse>('/sharing/rules', req);
}

export async function deactivateShareRule(
  ruleId: string,
): Promise<EnhancedApiResponse<OkResponse>> {
  return sharingApi.delete<OkResponse>(
    `/sharing/rules/${encodeURIComponent(ruleId)}`,
  );
}

export async function generateShareInvite(
  req: GenerateInviteRequest,
): Promise<EnhancedApiResponse<ShareInviteResponse>> {
  return sharingApi.post<ShareInviteResponse>('/sharing/invite', req);
}

export async function acceptShareInvite(
  invite: ShareInvite,
): Promise<EnhancedApiResponse<AcceptShareInviteResponse>> {
  return sharingApi.post<AcceptShareInviteResponse>('/sharing/accept', {
    invite,
  });
}

export async function listPendingShareInvites(): Promise<
  EnhancedApiResponse<PendingInvitesResponse>
> {
  return sharingApi.get<PendingInvitesResponse>('/sharing/pending-invites');
}
