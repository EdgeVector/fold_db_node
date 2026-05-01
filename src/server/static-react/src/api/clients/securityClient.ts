// Security API Client — public key and security status operations

import { ApiClient, getSharedClient } from "../core/client";
import { API_ENDPOINTS } from "../endpoints";
import {
  API_TIMEOUTS,
  API_RETRIES,
  API_CACHE_TTL,
  CACHE_KEYS,
} from "../../constants/api";
import type { EnhancedApiResponse } from "../core/types";

// Security-specific response types
// Mirrors `fold_db::security::PublicKeyInfo` (default serde, snake_case fields).
// Keep field names and JSON shapes in lockstep with the Rust struct.
export interface KeyInfo {
  id: string;
  public_key: string;
  owner_id: string;
  created_at: number;
  expires_at: number | null;
  is_active: boolean;
  permissions: string[];
  metadata: Record<string, string>;
}

// Wire shape returned by `GET /api/security/system-key`
// (see src/server/routes/security.rs). Success path sets `success: true` and
// `key`; 404/500 set `success: false` and `error`.
export interface SystemKeyResponse {
  success: boolean;
  key?: KeyInfo;
  error?: string;
}

export interface SecurityStatus {
  systemKeyRegistered: boolean;
  systemKeyId?: string;
  authenticationRequired: boolean;
  encryptionEnabled: boolean;
  lastKeyRotation?: string;
}

// Unified Security API Client Implementation
export class UnifiedSecurityClient {
  private readonly client: ApiClient;

  constructor(client?: ApiClient) {
    this.client = client || getSharedClient();
  }

  // Get the system's public key (no auth required)
  async getSystemPublicKey(): Promise<EnhancedApiResponse<SystemKeyResponse>> {
    return this.client.get<SystemKeyResponse>(
      API_ENDPOINTS.GET_SYSTEM_PUBLIC_KEY,
      {
        requiresAuth: false, // System public key is public
        timeout: API_TIMEOUTS.QUICK,
        retries: API_RETRIES.CRITICAL, // Multiple retries for critical system data
        cacheable: true, // Cache system public key
        cacheTtl: API_CACHE_TTL.SYSTEM_PUBLIC_KEY, // Cache for 1 hour (system key doesn't change often)
        cacheKey: CACHE_KEYS.SYSTEM_PUBLIC_KEY,
      },
    );
  }

  // Validate a public key's format and cryptographic properties (client-side)
  validatePublicKeyFormat(publicKey: string): {
    isValid: boolean;
    format?: string;
    length?: number;
    error?: string;
  } {
    try {
      if (!publicKey || typeof publicKey !== "string") {
        return {
          isValid: false,
          error: "Public key must be a non-empty string",
        };
      }

      const cleanKey = publicKey.trim();

      // Base64 character check (no decode)
      const base64Regex =
        /^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/;
      if (!base64Regex.test(cleanKey)) {
        return {
          isValid: false,
          error: "Invalid base64 encoding",
        };
      }

      // Ed25519 public keys are 32 bytes → base64 length should be 44
      if (cleanKey.length !== 44) {
        return {
          isValid: false,
          format: "Unknown",
          length: Math.ceil((cleanKey.length / 4) * 3),
          error: "Invalid key length: expected 44 base64 chars for Ed25519",
        };
      }

      return {
        isValid: true,
        format: "Ed25519",
        length: 32,
      };
    } catch (error: unknown) {
      const message = error instanceof Error ? error.message : String(error);
      return {
        isValid: false,
        error: `Validation error: ${message}`,
      };
    }
  }

  // Get security status and configuration (no auth required)
  async getSecurityStatus(): Promise<EnhancedApiResponse<SecurityStatus>> {
    return this.client.get<SecurityStatus>(API_ENDPOINTS.GET_SYSTEM_STATUS, {
      timeout: API_TIMEOUTS.QUICK,
      retries: API_RETRIES.STANDARD,
      cacheable: true,
      cacheTtl: API_CACHE_TTL.SECURITY_STATUS,
      cacheKey: CACHE_KEYS.SECURITY_STATUS,
    });
  }

  // Get API metrics for security operations
  getMetrics() {
    return this.client
      .getMetrics()
      .filter((metric) => metric.url.includes("/security"));
  }

  // Clear security-related cache
  clearCache(): void {
    this.client.clearCache();
  }
}

// Create default instance
export const securityClient = new UnifiedSecurityClient();

// Export factory function for custom instances
export function createSecurityClient(
  client?: ApiClient,
): UnifiedSecurityClient {
  return new UnifiedSecurityClient(client);
}

export const getSystemPublicKey =
  securityClient.getSystemPublicKey.bind(securityClient);
export const validatePublicKeyFormat =
  securityClient.validatePublicKeyFormat.bind(securityClient);
export const getSecurityStatus =
  securityClient.getSecurityStatus.bind(securityClient);

export default securityClient;
