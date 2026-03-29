import { ApiClient, getSharedClient } from '../core/client';
import { API_TIMEOUTS, API_RETRIES } from '../../constants/api';
import type { EnhancedApiResponse } from '../core/types';

// Discovery types

export interface DiscoveryOptIn {
  schema_name: string;
  category: string;
  include_preview: boolean;
  preview_max_chars: number;
  preview_excluded_fields: string[];
  opted_in_at: string;
}

export interface OptInRequest {
  schema_name: string;
  category: string;
  include_preview?: boolean;
  preview_max_chars?: number;
  preview_excluded_fields?: string[];
}

export interface PublishResult {
  accepted: number;
  quarantined: number;
  total: number;
  skipped: number;
}

export interface SearchResult {
  pseudonym: string;
  similarity: number;
  category: string;
  content_preview?: string | null;
}

export interface ConnectionRequest {
  request_id: string;
  target_pseudonym: string;
  requester_pseudonym: string;
  message: string;
  status: string;
  created_at: string;
}

export interface InterestCategory {
  name: string;
  count: number;
  avg_similarity: number;
  enabled: boolean;
}

export interface InterestProfile {
  categories: InterestCategory[];
  total_embeddings_scanned: number;
  unmatched_count: number;
  detected_at: string;
  seed_version: number;
}

export interface SimilarProfile {
  pseudonym: string;
  match_percentage: number;
  shared_categories: string[];
  top_similarity: number;
}

export interface SimilarProfilesResponse {
  profiles: SimilarProfile[];
  user_categories_used: number;
}

export class DiscoveryClient {
  private readonly client: ApiClient;

  constructor(client?: ApiClient) {
    this.client = client || getSharedClient();
  }

  async listOptIns(): Promise<EnhancedApiResponse<{ configs: DiscoveryOptIn[] }>> {
    return this.client.get('/discovery/opt-ins', {
      timeout: API_TIMEOUTS.STANDARD,
      retries: API_RETRIES.STANDARD,
    });
  }

  async optIn(req: OptInRequest): Promise<EnhancedApiResponse<{ configs: DiscoveryOptIn[] }>> {
    return this.client.post('/discovery/opt-in', req, {
      timeout: API_TIMEOUTS.STANDARD,
    });
  }

  async optOut(schema_name: string): Promise<EnhancedApiResponse<{ configs: DiscoveryOptIn[] }>> {
    return this.client.post('/discovery/opt-out', { schema_name }, {
      timeout: API_TIMEOUTS.STANDARD,
    });
  }

  async publish(): Promise<EnhancedApiResponse<PublishResult>> {
    return this.client.post('/discovery/publish', {}, {
      timeout: API_TIMEOUTS.LONG,
    });
  }

  async search(
    query: string,
    top_k?: number,
    category_filter?: string,
  ): Promise<EnhancedApiResponse<{ results: SearchResult[] }>> {
    return this.client.post('/discovery/search', {
      query,
      top_k: top_k || 20,
      category_filter: category_filter || undefined,
    }, {
      timeout: API_TIMEOUTS.LONG,
    });
  }

  async connect(
    target_pseudonym: string,
    message: string,
  ): Promise<EnhancedApiResponse<void>> {
    return this.client.post('/discovery/connect', {
      target_pseudonym,
      message,
    }, {
      timeout: API_TIMEOUTS.STANDARD,
    });
  }

  async pollRequests(): Promise<EnhancedApiResponse<{ requests: ConnectionRequest[] }>> {
    return this.client.get('/discovery/requests', {
      timeout: API_TIMEOUTS.STANDARD,
    });
  }

  async getInterests(): Promise<EnhancedApiResponse<InterestProfile>> {
    return this.client.get('/discovery/interests', {
      timeout: API_TIMEOUTS.STANDARD,
    });
  }

  async toggleInterest(category: string, enabled: boolean): Promise<EnhancedApiResponse<InterestProfile>> {
    return this.client.post('/discovery/interests/toggle', { category, enabled }, {
      timeout: API_TIMEOUTS.STANDARD,
    });
  }

  async detectInterests(): Promise<EnhancedApiResponse<InterestProfile>> {
    return this.client.post('/discovery/interests/detect', {}, {
      timeout: API_TIMEOUTS.LONG,
    });
  }

  async getSimilarProfiles(): Promise<EnhancedApiResponse<SimilarProfilesResponse>> {
    return this.client.get('/discovery/similar-profiles', {
      timeout: API_TIMEOUTS.LONG,
      retries: API_RETRIES.STANDARD,
    });
  }
}

export const discoveryClient = new DiscoveryClient();
export default discoveryClient;
