// Trust Graph API Client — manage trust relationships, identity, and contacts

import { createApiClient } from "../core/client";
import type { EnhancedApiResponse } from "../core/types";

// ===== Types =====

export type TrustTier = "Public" | "Outer" | "Trusted" | "Inner" | "Owner";

export interface IdentityCard {
  display_name: string;
  contact_hint?: string | null;
  birthday?: string | null;
}

export interface Contact {
  public_key: string;
  display_name: string;
  contact_hint?: string | null;
  direction: "outgoing" | "incoming" | "mutual";
  connected_at: string;
  pseudonym?: string | null;
  revoked: boolean;
  roles?: Record<string, string>;
}

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

export interface AuditLogResponse {
  events: AuditEvent[];
  count: number;
}

export interface AuditEvent {
  id: string;
  timestamp: string;
  user_id: string;
  action: Record<string, unknown>;
  trust_tier: TrustTier | null;
  decision_granted: boolean;
}

export interface TrustInvite {
  sender_pub_key: string;
  sender_identity: IdentityCard;
  proposed_role: string;
  nonce: string;
  created_at: string;
  signature: string;
}

export interface InvitePreview {
  valid: boolean;
  sender: {
    display_name: string;
    contact_hint?: string | null;
    public_key: string;
    fingerprint: string;
  };
  proposed_role: string;
  created_at: string;
}

export interface AcceptInviteResponse {
  accepted: boolean;
  sender: {
    display_name: string;
    contact_hint?: string | null;
    public_key: string;
  };
  reciprocal_invite?: TrustInvite | null;
  reciprocal_token?: string | null;
}

const client = createApiClient({
  enableCache: false,
  enableLogging: true,
  enableMetrics: true,
});

// ===== Identity Card =====

export async function getIdentityCard(): Promise<EnhancedApiResponse<{ identity_card: IdentityCard | null }>> {
  return client.get<{ identity_card: IdentityCard | null }>("/identity/card");
}

export async function setIdentityCard(
  displayName: string,
  contactHint?: string | null,
  birthday?: string | null,
): Promise<EnhancedApiResponse<{ saved: boolean }>> {
  return client.put<{ saved: boolean }>("/identity/card", {
    display_name: displayName,
    contact_hint: contactHint || null,
    birthday: birthday || null,
  });
}

// ===== Contacts =====

export async function listContacts(): Promise<EnhancedApiResponse<{ contacts: Contact[] }>> {
  return client.get<{ contacts: Contact[] }>("/contacts");
}

export async function getContact(
  publicKey: string,
): Promise<EnhancedApiResponse<{ contact: Contact | null }>> {
  return client.get<{ contact: Contact | null }>(`/contacts/${encodeURIComponent(publicKey)}`);
}

export async function revokeContact(
  publicKey: string,
): Promise<EnhancedApiResponse<{ revoked: boolean }>> {
  return client.delete<{ revoked: boolean }>(`/contacts/${encodeURIComponent(publicKey)}`);
}

// ===== Trust Graph =====

export async function listTrustGrants(): Promise<EnhancedApiResponse<TrustGrantsResponse>> {
  return client.get<TrustGrantsResponse>("/trust/grants");
}

export async function grantTrust(
  publicKey: string,
  role: string,
): Promise<EnhancedApiResponse<{ granted: boolean; role: string }>> {
  return client.post<{ granted: boolean; role: string }>("/trust/grant", {
    public_key: publicKey,
    role,
  });
}

export async function revokeTrust(
  publicKey: string,
): Promise<EnhancedApiResponse<{ revoked: boolean }>> {
  return client.delete<{ revoked: boolean }>(`/trust/revoke/${encodeURIComponent(publicKey)}`);
}

export async function resolveTrustTier(
  publicKey: string,
): Promise<EnhancedApiResponse<TrustResolveResponse>> {
  return client.get<TrustResolveResponse>(`/trust/resolve/${encodeURIComponent(publicKey)}`);
}

// ===== Trust Invites =====

export async function createTrustInvite(
  proposedRole: string,
): Promise<EnhancedApiResponse<{ invite: TrustInvite; token: string }>> {
  return client.post<{ invite: TrustInvite; token: string }>("/trust/invite", {
    proposed_role: proposedRole,
  });
}

export async function previewTrustInvite(
  token: string,
): Promise<EnhancedApiResponse<InvitePreview>> {
  return client.post<InvitePreview>("/trust/invite/preview", { token });
}

export async function acceptTrustInvite(
  token: string,
  acceptRole?: string,
  trustBack: boolean = true,
): Promise<EnhancedApiResponse<AcceptInviteResponse>> {
  return client.post<AcceptInviteResponse>("/trust/invite/accept", {
    token,
    accept_role: acceptRole,
    trust_back: trustBack,
  });
}

// ===== Trust Invite Relay (via Exemem) =====

export async function shareTrustInvite(
  token: string,
): Promise<EnhancedApiResponse<{ invite_id: string; shared: boolean }>> {
  return client.post<{ invite_id: string; shared: boolean }>("/trust/invite/share", { token });
}

export async function fetchSharedInvite(
  inviteId: string,
): Promise<EnhancedApiResponse<{ ok: boolean; token: string }>> {
  return client.get<{ ok: boolean; token: string }>(`/trust/invite/fetch?id=${encodeURIComponent(inviteId)}`);
}

// ===== Email-Verified Trust Invites =====

// NOTE: sender_name is intentionally resolved server-side from the local
// identity card — the client must not pass it. This prevents display-name
// spoofing in outgoing verification emails.
export async function sendVerifiedInvite(
  token: string,
  recipientEmail: string,
): Promise<EnhancedApiResponse<{ ok: boolean; invite_id: string }>> {
  return client.post<{ ok: boolean; invite_id: string }>("/trust/invite/send-verified", {
    token,
    recipient_email: recipientEmail,
  });
}

export async function verifyInviteCode(
  inviteId: string,
  code: string,
): Promise<EnhancedApiResponse<{ ok: boolean; token: string }>> {
  return client.post<{ ok: boolean; token: string }>("/trust/invite/verify", {
    invite_id: inviteId,
    code,
  });
}

// ===== Sharing Roles =====

export interface SharingRole {
  name: string;
  domain: string;
  tier: TrustTier;
  description: string;
}

export interface AccessibleSchema {
  schema_name: string;
  trust_domain: string;
  readable_fields: string[];
  writable_fields: string[];
}

export interface SharingAuditResult {
  contact_public_key: string;
  contact_display_name: string;
  domain_tiers: Record<string, TrustTier>;
  domain_roles: Record<string, string>;
  accessible_schemas: AccessibleSchema[];
  total_readable: number;
  total_writable: number;
}

export async function listSharingRoles(): Promise<EnhancedApiResponse<{ roles: Record<string, SharingRole> }>> {
  return client.get<{ roles: Record<string, SharingRole> }>("/sharing/roles");
}

export async function assignRoleToContact(
  publicKey: string,
  roleName: string,
): Promise<EnhancedApiResponse<{ assigned: boolean; role: string }>> {
  return client.post<{ assigned: boolean; role: string }>(
    `/sharing/assign/${encodeURIComponent(publicKey)}`,
    { role_name: roleName },
  );
}

export async function removeRoleFromContact(
  publicKey: string,
  domain: string,
): Promise<EnhancedApiResponse<{ removed: boolean; domain: string }>> {
  return client.delete<{ removed: boolean; domain: string }>(
    `/sharing/remove/${encodeURIComponent(publicKey)}/${encodeURIComponent(domain)}`,
  );
}

export async function auditContactAccess(
  publicKey: string,
): Promise<EnhancedApiResponse<SharingAuditResult>> {
  return client.get<SharingAuditResult>(`/sharing/audit/${encodeURIComponent(publicKey)}`);
}

export interface SharingPosture {
  domains: string[];
  schemas_per_domain: Record<string, number>;
  contacts_per_domain: Record<string, number>;
  total_policy_fields: number;
  total_unprotected_fields: number;
}

export async function getSharingPosture(): Promise<EnhancedApiResponse<SharingPosture>> {
  return client.get<SharingPosture>("/sharing/posture");
}

export async function applyDefaultPolicies(): Promise<EnhancedApiResponse<{ schemas_updated: number; fields_updated: number }>> {
  return client.post<{ schemas_updated: number; fields_updated: number }>("/sharing/apply-defaults");
}

export async function setFieldPolicy(
  schemaName: string,
  fieldName: string,
  policy: Record<string, unknown>,
): Promise<EnhancedApiResponse<{ policy_set: boolean }>> {
  return client.put<{ policy_set: boolean }>(
    `/sharing/policy/${encodeURIComponent(schemaName)}/${encodeURIComponent(fieldName)}`,
    { policy },
  );
}

export async function getSchemaFieldPolicies(
  schemaName: string,
): Promise<EnhancedApiResponse<{ schema_name: string; field_policies: Record<string, unknown> }>> {
  return client.get<{ schema_name: string; field_policies: Record<string, unknown> }>(
    `/sharing/policies/${encodeURIComponent(schemaName)}`,
  );
}

// ===== Sent Invites =====

export interface SentInvite {
  nonce: string;
  recipient_hint: string;
  proposed_role: string;
  created_at: string;
  status: "pending" | "accepted" | "expired";
}

export async function listSentInvites(): Promise<EnhancedApiResponse<{ sent_invites: SentInvite[] }>> {
  return client.get<{ sent_invites: SentInvite[] }>("/trust/invite/sent");
}

// ===== Declined Invites =====

export interface DeclinedInvite {
  sender_pub_key: string;
  sender_display_name: string;
  sender_contact_hint?: string | null;
  proposed_role: string;
  declined_at: string;
  nonce: string;
}

export async function declineTrustInvite(
  token: string,
): Promise<EnhancedApiResponse<{ declined: boolean; sender: string }>> {
  return client.post<{ declined: boolean; sender: string }>("/trust/invite/decline", { token });
}

export async function listDeclinedInvites(): Promise<EnhancedApiResponse<{ declined_invites: DeclinedInvite[] }>> {
  return client.get<{ declined_invites: DeclinedInvite[] }>("/trust/invite/declined");
}

export async function undeclineInvite(
  nonce: string,
): Promise<EnhancedApiResponse<{ undeclined: boolean }>> {
  return client.delete<{ undeclined: boolean }>(`/trust/invite/declined/${encodeURIComponent(nonce)}`);
}

// ===== Remote Query =====

export interface RemoteNodeInfo {
  public_key: string;
  node_id: string;
  shared_schemas: string[];
}

export interface RemoteQueryResult {
  results: Record<string, unknown>[];
}

export async function browseRemoteNode(
  remoteUrl: string,
): Promise<EnhancedApiResponse<RemoteNodeInfo>> {
  return client.post<RemoteNodeInfo>("/remote/browse", { remote_url: remoteUrl });
}

export async function proxyQueryRemote(
  remoteUrl: string,
  schemaName: string,
  fields: string[],
): Promise<EnhancedApiResponse<RemoteQueryResult>> {
  return client.post<RemoteQueryResult>("/remote/proxy-query", {
    remote_url: remoteUrl,
    schema_name: schemaName,
    fields,
  });
}

// ===== Audit Log =====

export async function getAuditLog(
  limit: number = 100,
): Promise<EnhancedApiResponse<AuditLogResponse>> {
  return client.get<AuditLogResponse>(`/trust/audit?limit=${limit}`);
}
