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
  /** First 8 hex chars of the fingerprint key — lets the UI
   *  distinguish otherwise-identical face embedding rows. */
  short_id: string;
  kind: string;
  /** Scalar rendered value (email, phone, name) or a collapsed
   *  placeholder for face embeddings. */
  display_value: string;
  first_seen: string | null;
  last_seen: string | null;
  /** Representative source record that references this fingerprint,
   *  formatted as `"<source_schema>:<source_key>"`. `null` when no
   *  Mention points at this fingerprint. */
  sample_source: string | null;
  sample_source_field: string | null;
  sample_mention_at: string | null;
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
  /** Raw exclusion lists from the Persona record — rendered in the
   *  collapsible exclusions panel so the user can Undo each one. */
  excluded_edge_ids: string[];
  excluded_mention_ids: string[];
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
 * Declarative patch shape mirroring `UpdatePersonaRequest` on the
 * backend. Every field is optional; callers populate only the ops
 * they need. Multiple ops may coexist in a single request and are
 * applied together within one read-modify-write cycle.
 */
export interface PersonaPatch {
  threshold?: number;
  add_excluded_edge_id?: string;
  remove_excluded_edge_id?: string;
  add_excluded_mention_id?: string;
  remove_excluded_mention_id?: string;
  /** Rename the persona. Rejected by the backend when built_in is true. */
  name?: string;
  /** Relationship category. One of
   *  self | family | colleague | friend | acquaintance | unknown. */
  relationship?: string;
  /** Replace the aliases array wholesale. */
  aliases?: string[];
  /** Set user_confirmed. Used by the Confirm action on tentative
   *  (auto-created) personas. The backend rejects true → false
   *  transitions; to reject a tentative persona, delete it. */
  user_confirmed?: boolean;
  /** Link this persona to a verified Identity (`id_<pub_key>`).
   *  Typically set by the import-identity-card flow, but exposed here
   *  so a future "link existing Identity" UI can reuse the PATCH. */
  link_identity_id?: string;
}

/** The canonical relationship list the backend accepts. Mirrors
 *  `ALLOWED_RELATIONSHIPS` in handlers/fingerprints/personas.rs. */
export const RELATIONSHIP_OPTIONS = [
  "self",
  "family",
  "colleague",
  "friend",
  "acquaintance",
  "unknown",
] as const;
export type Relationship = (typeof RELATIONSHIP_OPTIONS)[number];

/**
 * Apply a patch to an existing Persona and return the freshly-
 * resolved detail. The backend does a single read-modify-write so
 * the caller can swap the response straight into place without a
 * second GET.
 */
export async function updatePersona(
  id: string,
  patch: PersonaPatch,
): Promise<EnhancedApiResponse<PersonaDetailResponse>> {
  return client.patch<PersonaDetailResponse>(
    `/fingerprints/personas/${encodeURIComponent(id)}`,
    patch,
  );
}

/**
 * Compat wrapper — existing callers that only want to move the
 * threshold slider can keep using this. New call sites should prefer
 * `updatePersona` directly so they can batch multiple ops.
 */
export async function updatePersonaThreshold(
  id: string,
  threshold: number,
): Promise<EnhancedApiResponse<PersonaDetailResponse>> {
  return updatePersona(id, { threshold });
}

// ===== My Identity Card =====

export interface MyIdentityCardResponse {
  pub_key: string;
  display_name: string;
  birthday: string | null;
  /** The self-attested face embedding, if one was collected at
   *  signup. Today always `null` — reserved for a future "take a
   *  selfie" setup step. */
  face_embedding: number[] | null;
  node_id: string;
  card_signature: string;
  issued_at: string;
}

/**
 * Fetch the node owner's signed Identity Card. Returns 404 if the
 * self-Identity hasn't been bootstrapped yet (user hasn't completed
 * the setup wizard).
 *
 * The card is the verbatim payload that will be handed to a peer
 * over QR / NFC / messaging in Phase 3. The signature is
 * verifiable standalone.
 */
export async function getMyIdentityCard(): Promise<
  EnhancedApiResponse<MyIdentityCardResponse>
> {
  return client.get<MyIdentityCardResponse>(
    "/fingerprints/my-identity-card",
  );
}

// ===== Import Identity Card (Phase 3b) =====

/**
 * Incoming card shape matches `MyIdentityCardResponse` 1:1 so the
 * user can literally paste the JSON they got from another node's
 * `/my-identity-card` response. The backend verifies the Ed25519
 * signature over the canonical bytes before committing anything.
 */
export interface IncomingIdentityCard {
  pub_key: string;
  display_name: string;
  birthday: string | null;
  face_embedding: number[] | null;
  node_id: string;
  card_signature: string;
  issued_at: string;
}

export interface ImportIdentityCardRequest {
  card: IncomingIdentityCard;
  /** Optional: a persona id on this node to link to the imported
   *  Identity so the verified badge renders immediately. */
  link_persona_id?: string;
}

export interface ImportIdentityCardResponse {
  identity_id: string;
  verified: boolean;
  /** True when the Identity was already on this node. Lets the UI
   *  distinguish "first-time import" from "re-paste of the same
   *  card". The backend is always idempotent either way. */
  was_already_present: boolean;
  /** Populated when `link_persona_id` was supplied and the link
   *  succeeded. Same shape as `PersonaDetailResponse`. */
  linked_persona: PersonaDetailResponse | null;
}

/**
 * Verify an incoming Identity Card and, on success, commit it as a
 * local Identity record. Optionally links it to an existing Persona
 * so the UI can flip the verified badge without a second round trip.
 *
 * Failure modes (all surface as 400):
 * - Malformed base64 on `pub_key` or `card_signature`.
 * - Signature doesn't verify against `pub_key`.
 * - `link_persona_id` was passed but the persona isn't on this node.
 */
export async function importIdentityCard(
  req: ImportIdentityCardRequest,
): Promise<EnhancedApiResponse<ImportIdentityCardResponse>> {
  return client.post<ImportIdentityCardResponse>(
    "/fingerprints/identity-cards/import",
    req,
  );
}

// ===== IngestionError types + client =====

export interface IngestionErrorView {
  id: string;
  source_schema: string;
  source_key: string;
  extractor: string;
  error_class: string;
  error_msg: string;
  retry_count: number;
  resolved: boolean;
  created_at: string;
  last_retry_at: string | null;
}

export interface ListIngestionErrorsResponse {
  errors: IngestionErrorView[];
}

/**
 * List every IngestionError row. By default resolved rows are hidden
 * — the Failed panel is a to-do list of live extractor failures.
 * Pass `includeResolved: true` to pull the archive too.
 */
export async function listIngestionErrors(
  includeResolved = false,
): Promise<EnhancedApiResponse<ListIngestionErrorsResponse>> {
  const qs = includeResolved ? "?include_resolved=true" : "";
  return client.get<ListIngestionErrorsResponse>(
    `/fingerprints/ingestion-errors${qs}`,
  );
}

/**
 * Set the resolved flag on a single IngestionError row.
 * Pass `resolved: true` (default) to dismiss, `false` to restore
 * a previously-dismissed row back into the active Failed panel.
 */
export async function resolveIngestionError(
  id: string,
  resolved = true,
): Promise<EnhancedApiResponse<IngestionErrorView>> {
  return client.patch<IngestionErrorView>(
    `/fingerprints/ingestion-errors/${encodeURIComponent(id)}`,
    { resolved },
  );
}

// ===== Suggested Personas =====

export interface SuggestedPersonaView {
  suggested_id: string;
  suggested_name: string;
  fingerprint_ids: string[];
  fingerprint_count: number;
  edge_count: number;
  mention_count: number;
  sample_fingerprints: FingerprintView[];
}

export interface ListSuggestedResponse {
  suggestions: SuggestedPersonaView[];
}

export interface AcceptSuggestedRequest {
  fingerprint_ids: string[];
  name: string;
  relationship?: string;
}

/**
 * Run the dense-subgraph sweep and return candidate clusters that
 * pass the MIN_FINGERPRINTS / MIN_MENTIONS gates and are not
 * already covered by an existing Persona.
 */
export async function listSuggestedPersonas(): Promise<
  EnhancedApiResponse<ListSuggestedResponse>
> {
  return client.get<ListSuggestedResponse>("/fingerprints/suggestions");
}

/**
 * Promote a suggested cluster into a real Persona record. Returns
 * the freshly-resolved PersonaDetailResponse so the UI can redirect
 * the user into the new Persona without a second round trip.
 */
export async function acceptSuggestedPersona(
  req: AcceptSuggestedRequest,
): Promise<EnhancedApiResponse<PersonaDetailResponse>> {
  return client.post<PersonaDetailResponse>(
    "/fingerprints/suggestions/accept",
    req,
  );
}
