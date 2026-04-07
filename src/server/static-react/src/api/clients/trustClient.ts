// Trust Graph API Client — manage trust relationships, identity, and contacts

import { createApiClient } from "../core/client";
import type { EnhancedApiResponse } from "../core/types";

// ===== Types =====

export interface IdentityCard {
  display_name: string;
  contact_hint?: string | null;
}

export interface Contact {
  public_key: string;
  display_name: string;
  contact_hint?: string | null;
  trust_distance: number;
  direction: "outgoing" | "incoming" | "mutual";
  connected_at: string;
  pseudonym?: string | null;
  revoked: boolean;
}

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

export interface AuditLogResponse {
  events: AuditEvent[];
  count: number;
}

export interface AuditEvent {
  id: string;
  timestamp: string;
  user_id: string;
  action: Record<string, unknown>;
  trust_distance: number | null;
  decision_granted: boolean;
}

export interface TrustInvite {
  sender_pub_key: string;
  sender_identity: IdentityCard;
  proposed_distance: number;
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
  proposed_distance: number;
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
): Promise<EnhancedApiResponse<{ saved: boolean }>> {
  return client.put<{ saved: boolean }>("/identity/card", {
    display_name: displayName,
    contact_hint: contactHint || null,
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
  distance: number,
): Promise<EnhancedApiResponse<{ granted: boolean }>> {
  return client.post<{ granted: boolean }>("/trust/grant", {
    public_key: publicKey,
    distance,
  });
}

export async function revokeTrust(
  publicKey: string,
): Promise<EnhancedApiResponse<{ revoked: boolean }>> {
  return client.delete<{ revoked: boolean }>(`/trust/revoke/${encodeURIComponent(publicKey)}`);
}

export async function setTrustOverride(
  publicKey: string,
  distance: number,
): Promise<EnhancedApiResponse<{ override_set: boolean }>> {
  return client.put<{ override_set: boolean }>("/trust/override", {
    public_key: publicKey,
    distance,
  });
}

export async function resolveTrustDistance(
  publicKey: string,
): Promise<EnhancedApiResponse<TrustResolveResponse>> {
  return client.get<TrustResolveResponse>(`/trust/resolve/${encodeURIComponent(publicKey)}`);
}

// ===== Trust Invites =====

export async function createTrustInvite(
  proposedDistance: number,
): Promise<EnhancedApiResponse<{ invite: TrustInvite; token: string }>> {
  return client.post<{ invite: TrustInvite; token: string }>("/trust/invite", {
    proposed_distance: proposedDistance,
  });
}

export async function previewTrustInvite(
  token: string,
): Promise<EnhancedApiResponse<InvitePreview>> {
  return client.post<InvitePreview>("/trust/invite/preview", { token });
}

export async function acceptTrustInvite(
  token: string,
  acceptDistance?: number,
  trustBack: boolean = true,
): Promise<EnhancedApiResponse<AcceptInviteResponse>> {
  return client.post<AcceptInviteResponse>("/trust/invite/accept", {
    token,
    accept_distance: acceptDistance,
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

// ===== Audit Log =====

export async function getAuditLog(
  limit: number = 100,
): Promise<EnhancedApiResponse<AuditLogResponse>> {
  return client.get<AuditLogResponse>(`/trust/audit?limit=${limit}`);
}
