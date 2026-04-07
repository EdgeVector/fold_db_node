// Trust Graph API Client — manage trust relationships and access policies

import { createApiClient } from "../core/client";
import type { EnhancedApiResponse } from "../core/types";

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

const client = createApiClient({
  enableCache: false,
  enableLogging: true,
  enableMetrics: true,
});

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

export async function getAuditLog(
  limit: number = 100,
): Promise<EnhancedApiResponse<AuditLogResponse>> {
  return client.get<AuditLogResponse>(`/trust/audit?limit=${limit}`);
}
