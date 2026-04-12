/**
 * API Clients Index
 * Centralized exports for all API clients
 */

// Schema Client
export {
  schemaClient,
  UnifiedSchemaClient,
  createSchemaClient,
  getSchemasByState,
  getAllSchemasWithState,
  getSchemaStatus,
  getSchema,
  approveSchema,
  blockSchema,
  loadSchema,
  unloadSchema,
  getApprovedSchemas,
} from "./schemaClient";

// Security Client
export {
  securityClient,
  UnifiedSecurityClient,
  createSecurityClient,
  getSystemPublicKey,
  validatePublicKeyFormat,
  getSecurityStatus,
} from "./securityClient";

// System Client
export {
  systemClient,
  getLogs,
  resetDatabase,
  getSystemStatus,
  createLogStream,
} from "./systemClient";

// Mutation Client
export * from "./mutationClient";

// Ingestion Client
export {
  ingestionClient,
} from "./ingestionClient";

// LLM Query Client
export { llmQueryClient } from "./llmQueryClient";
// Native Index Client
export { nativeIndexClient, NativeIndexClient } from "./nativeIndexClient";
// Indexing Status Client
export { getIndexingStatus } from "./indexingClient";
export type { IndexingStatus } from "./indexingClient";
// Discovery Client
export { discoveryClient, DiscoveryClient } from "./discoveryClient";
export type {
  DiscoveryOptIn,
  OptInRequest,
  PublishResult,
  SearchResult,
  ConnectionRequest,
  FaceEntry,
  FaceSearchResult,
} from "./discoveryClient";

export type {
  ResetDatabaseRequest,
  ResetDatabaseResponse,
} from "./systemClient";

export type {
  OllamaConfig,
  IngestionConfig,
} from "./ingestionClient";
