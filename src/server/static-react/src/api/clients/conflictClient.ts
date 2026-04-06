// Conflict API Client — sync merge conflict surfacing

import { createApiClient } from "../core/client";
import { API_ENDPOINTS } from "../endpoints";
import type { EnhancedApiResponse } from "../core/types";

export interface ConflictSummary {
  id: string;
  molecule_uuid: string;
  conflict_key: string;
  winner_atom: string;
  loser_atom: string;
  detected_at: string;
}

export interface ConflictsResponse {
  conflicts: ConflictSummary[];
}

export interface ResolveConflictResponse {
  resolved: string;
}

const client = createApiClient({
  enableCache: false,
  enableLogging: true,
  enableMetrics: true,
});

export async function getConflicts(
  moleculeUuid?: string,
): Promise<EnhancedApiResponse<ConflictsResponse>> {
  const params = moleculeUuid ? `?molecule_uuid=${encodeURIComponent(moleculeUuid)}` : "";
  return client.get<ConflictsResponse>(
    `${API_ENDPOINTS.LIST_CONFLICTS}${params}`,
  );
}

export async function resolveConflict(
  conflictId: string,
): Promise<EnhancedApiResponse<ResolveConflictResponse>> {
  return client.post<ResolveConflictResponse>(
    API_ENDPOINTS.RESOLVE_CONFLICT(conflictId),
  );
}
