// Fingerprints / Personas API client.
//
// Consumes the backend routes exposed by
// `src/server/routes/fingerprints/personas.rs`:
//
//   GET   /api/fingerprints/personas       → list with resolved counts
//   GET   /api/fingerprints/personas/:id   → detail with resolved cluster
//   PATCH /api/fingerprints/personas/:id   → update mutable fields (threshold)
//
// The backend handler translates descriptive schema names through
// `canonical_names::lookup()` internally, so this client just uses
// stable routes and plain strings. See
// `docs/designs/fingerprints.md` (exemem-workspace) for the model.

import { createApiClient } from "../core/client";
import type { EnhancedApiResponse } from "../core/types";

// ===== Types =====

export interface PersonaSummary {
  id: string;
  name: string;
  /** True when the persona is linked to a verified Identity. */
  identity_linked: boolean;
  threshold: number;
  relationship: string;
  trust_tier: number;
  built_in: boolean;
  user_confirmed: boolean;
  fingerprint_count: number;
  edge_count: number;
  mention_count: number;
}

export interface ListPersonasResponse {
  personas: PersonaSummary[];
}

/**
 * Diagnostics surfaced by the resolver. Zero on every field means a
 * clean resolve. Any non-zero count or non-empty vector means the
 * UI should show a warning explaining what was missing, filtered,
 * or excluded.
 */
export interface ResolveDiagnostics {
  missing_seed_fingerprint_ids: string[];
  excluded_edge_count: number;
  forbidden_edge_count: number;
  below_threshold_edge_count: number;
  excluded_mention_count: number;
  dangling_edge_ids: string[];
}

export interface FingerprintView {
  id: string;
  kind: string;
  /** Scalar rendered value (email, phone, name) or a collapsed
   *  placeholder for face embeddings. */
  display_value: string;
  first_seen: string | null;
  last_seen: string | null;
}

export interface EdgeView {
  id: string;
  a: string;
  b: string;
  kind: string;
  weight: number;
  created_at: string | null;
}

export interface MentionView {
  id: string;
  source_schema: string;
  source_key: string;
  source_field: string;
  extractor: string;
  confidence: number;
  created_at: string | null;
}

export interface PersonaDetailResponse {
  id: string;
  name: string;
  threshold: number;
  relationship: string;
  trust_tier: number;
  built_in: boolean;
  user_confirmed: boolean;
  identity_id: string | null;
  seed_fingerprint_ids: string[];
  aliases: string[];
  fingerprint_ids: string[];
  edge_ids: string[];
  mention_ids: string[];
  /** Enriched records for the resolved cluster. Same order as the
   *  corresponding `*_ids` arrays; entries may be fewer than IDs
   *  when records are dangling. */
  fingerprints: FingerprintView[];
  edges: EdgeView[];
  mentions: MentionView[];
  /** None when the resolve was clean. */
  diagnostics: ResolveDiagnostics | null;
}

// ===== Client =====

const client = createApiClient({
  enableCache: false,
  enableLogging: true,
  enableMetrics: true,
});

export async function listPersonas(): Promise<
  EnhancedApiResponse<ListPersonasResponse>
> {
  return client.get<ListPersonasResponse>("/fingerprints/personas");
}

export async function getPersona(
  id: string,
): Promise<EnhancedApiResponse<PersonaDetailResponse>> {
  return client.get<PersonaDetailResponse>(
    `/fingerprints/personas/${encodeURIComponent(id)}`,
  );
}

/**
 * Update the threshold on an existing Persona. The backend does a
 * read-modify-write against the Persona record and returns the
 * freshly-resolved detail with updated counts + diagnostics, so the
 * UI can swap the response straight into place without a second GET.
 */
export async function updatePersonaThreshold(
  id: string,
  threshold: number,
): Promise<EnhancedApiResponse<PersonaDetailResponse>> {
  return client.patch<PersonaDetailResponse>(
    `/fingerprints/personas/${encodeURIComponent(id)}`,
    { threshold },
  );
}
