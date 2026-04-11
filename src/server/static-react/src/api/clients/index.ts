/**
 * API Clients Index
 * Centralized exports for all API clients
 * Part of API-STD-1 standardization
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
  UnifiedSystemClient,
  createSystemClient,
  getLogs,
  resetDatabase,
  getSystemStatus,
  createLogStream,
  validateResetRequest,
} from "./systemClient";

// Mutation Client (if exists)
export * from "./mutationClient";

// Ingestion Client
export {
  ingestionClient,
  UnifiedIngestionClient,
  createIngestionClient,
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

// Type exports for convenience
export type {
  SchemasByStateResponse,
  SchemasWithStateResponse,
  SchemaStatusResponse,
} from "./schemaClient";

export type {
  SystemKeyResponse,
  SecurityStatus,
} from "./securityClient";

export type {
  LogsResponse,
  ResetDatabaseRequest,
  ResetDatabaseResponse,
  SystemStatusResponse,
} from "./systemClient";

export type {
  IngestionStatus,
  IngestionProgress,
  IngestionResults,
  OllamaConfig,
  AnthropicConfig,
  IngestionConfig,
  ValidationRequest,
  ValidationResponse,
  ProcessIngestionRequest,
  ProcessIngestionResponse,
  FileRecommendation,
  SmartFolderScanResponse,
  SmartFolderIngestResponse,
} from "./ingestionClient";
