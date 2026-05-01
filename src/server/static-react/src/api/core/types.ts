/**
 * Core API Types for Unified Client
 * Standardized interfaces for all API operations
 */

import type { ApiResponse } from '../../types/api';
import type { SystemKeyResponse } from '../clients/securityClient';

// Re-export existing ApiResponse for backward compatibility
export type { ApiResponse };

// Enhanced API Response with metadata
export interface EnhancedApiResponse<T = unknown> extends ApiResponse<T> {
  status: number;
  headers?: Record<string, string>;
  meta?: {
    requestId?: string;
    timestamp: number;
    cached?: boolean;
    fromCache?: boolean;
  };
}

// Request Configuration Options
export interface RequestOptions {
  requiresAuth?: boolean;
  timeout?: number;
  retries?: number;
  validateSchema?: boolean;
  cacheable?: boolean;
  cacheKey?: string;
  cacheTtl?: number;
  requestId?: string;
  abortSignal?: AbortSignal;
  priority?: 'low' | 'normal' | 'high';
  headers?: Record<string, string>;
}

// API Client Configuration
export interface ApiClientConfig {
  baseUrl?: string;
  timeout?: number;
  retryAttempts?: number;
  retryDelay?: number;
  defaultHeaders?: Record<string, string>;
  enableCache?: boolean;
  enableLogging?: boolean;
  enableMetrics?: boolean;
}

// HTTP Method Types
export type HttpMethod = 'GET' | 'POST' | 'PUT' | 'DELETE' | 'PATCH';

// Request Interceptor Function
export type RequestInterceptor = (config: RequestConfig) => RequestConfig | Promise<RequestConfig>;

// Response Interceptor Function
export type ResponseInterceptor<T = unknown> = (response: EnhancedApiResponse<T>) => EnhancedApiResponse<T> | Promise<EnhancedApiResponse<T>>;

// Internal Request Configuration
export interface RequestConfig {
  url: string;
  method: HttpMethod;
  headers: Record<string, string>;
  body?: string | ArrayBuffer | Blob | FormData | URLSearchParams | ReadableStream<Uint8Array> | null;
  timeout: number;
  retries: number;
  validateSchema: boolean;
  requiresAuth: boolean;
  abortSignal?: AbortSignal;
  metadata: {
    requestId: string;
    timestamp: number;
    priority: 'low' | 'normal' | 'high';
  };
}

// Cache Entry Interface
export interface CacheEntry<T = unknown> {
  data: T;
  timestamp: number;
  ttl: number;
  key: string;
}

// Request Metrics Interface
export interface RequestMetrics {
  requestId: string;
  url: string;
  method: HttpMethod;
  startTime: number;
  endTime?: number;
  duration?: number;
  status?: number;
  cached?: boolean;
  retryCount?: number;
  error?: string;
}

// Batch Request Interface
export interface BatchRequest {
  id: string;
  method: HttpMethod;
  url: string;
  body?: string | ArrayBuffer | Blob | FormData | URLSearchParams | ReadableStream<Uint8Array> | null;
  options?: RequestOptions;
}

// Batch Response Interface
export interface BatchResponse<T = unknown> {
  id: string;
  success: boolean;
  data?: T;
  error?: string;
  status: number;
}

// Client Instance Interface
export interface ApiClientInstance {
  get<T>(endpoint: string, options?: RequestOptions): Promise<EnhancedApiResponse<T>>;
  post<T>(endpoint: string, data?: string | ArrayBuffer | Blob | FormData | URLSearchParams | ReadableStream<Uint8Array> | null, options?: RequestOptions): Promise<EnhancedApiResponse<T>>;
  put<T>(endpoint: string, data?: string | ArrayBuffer | Blob | FormData | URLSearchParams | ReadableStream<Uint8Array> | null, options?: RequestOptions): Promise<EnhancedApiResponse<T>>;
  delete<T>(endpoint: string, options?: RequestOptions): Promise<EnhancedApiResponse<T>>;
  patch<T>(endpoint: string, data?: string | ArrayBuffer | Blob | FormData | URLSearchParams | ReadableStream<Uint8Array> | null, options?: RequestOptions): Promise<EnhancedApiResponse<T>>;
  batch<T>(requests: BatchRequest[]): Promise<BatchResponse<T>[]>;
  
  // Interceptor management
  addRequestInterceptor(interceptor: RequestInterceptor): void;
  addResponseInterceptor<T>(interceptor: ResponseInterceptor<T>): void;
  addErrorInterceptor(interceptor: ErrorInterceptor): void;
  
  // Cache management
  clearCache(): void;
  getCacheStats(): { size: number; hitRate: number };
  
  // Metrics
  getMetrics(): RequestMetrics[];
  clearMetrics(): void;
}

// Domain-specific client interfaces for type safety
export interface SchemaApiClient {
  getSchemas(): Promise<EnhancedApiResponse<SchemaData[]>>;
  getSchema(name: string): Promise<EnhancedApiResponse<SchemaData>>;
  // Removed: getSchemasByState, getAllSchemasWithState, getSchemaStatus – compute client-side
  approveSchema(name: string): Promise<EnhancedApiResponse<void>>;
  blockSchema(name: string): Promise<EnhancedApiResponse<void>>;
}

export interface MutationApiClient {
  executeMutation(mutation: Record<string, unknown>): Promise<EnhancedApiResponse<Record<string, unknown>>>;
  executeQuery(query: Record<string, unknown>): Promise<EnhancedApiResponse<Record<string, unknown>>>;
  // validateMutation now a client-side noop; keep optional for compatibility
}

export interface SecurityApiClient {
  getSystemPublicKey(): Promise<EnhancedApiResponse<SystemKeyResponse>>;
}
