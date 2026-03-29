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

export interface LocalConnectionRequest {
  request_id: string;
  message_id: string;
  target_pseudonym: string;
  sender_pseudonym: string;
  sender_public_key: string;
  reply_public_key: string;
  message: string;
  status: string;
  created_at: string;
  responded_at: string | null;
}

export interface LocalSentRequest {
  request_id: string;
  target_pseudonym: string;
  sender_pseudonym: string;
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

export interface BrowseCategory {
  category: string;
  entry_count: number;
  user_count: number;
}

export interface BrowseCategoriesResponse {
  categories: BrowseCategory[];
}

// Calendar sharing types

export interface CalendarSharingStatus {
  opted_in: boolean;
  local_event_count: number;
  peer_count: number;
}

export interface CalendarEventInput {
  summary: string;
  start_time: string;
  end_time: string;
  location: string;
  calendar: string;
}

export interface SharedEvent {
  event_title: string;
  start_time: string;
  end_time: string;
  location: string;
  connection_count: number;
  connection_pseudonyms: string[];
  match_score: number;
}

export interface SharedEventsResponse {
  shared_events: SharedEvent[];
  connection_count: number;
}

// Photo Moment Detection types

export interface MomentOptIn {
  peer_pseudonym: string;
  peer_display_name: string | null;
  opted_in_at: string;
}

export interface SharedMoment {
  moment_id: string;
  peer_pseudonym: string;
  peer_display_name: string | null;
  time_bucket: string;
  geohash: string;
  location_name: string | null;
  our_record_id: string;
  our_timestamp: string;
  peer_timestamp: string | null;
  detected_at: string;
}

export interface PhotoMetadata {
  record_id: string;
  timestamp: string;
  latitude: number;
  longitude: number;
}

export interface MomentScanResult {
  photos_scanned: number;
  hashes_generated: number;
  peers_processed: number;
}

export interface MomentDetectResult {
  new_moments_found: number;
  moments: SharedMoment[];
}

// Weekly Digest types

export interface DigestSection {
  title: string;
  items: string[];
  icon: string;
}

export interface WeeklyDigest {
  digest_id: string;
  period_start: string;
  period_end: string;
  sections: DigestSection[];
  summary: string;
  generated_at: string;
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
    offset?: number,
  ): Promise<EnhancedApiResponse<{ results: SearchResult[] }>> {
    return this.client.post('/discovery/search', {
      query,
      top_k: top_k || 20,
      category_filter: category_filter || undefined,
      offset: offset || undefined,
    }, {
      timeout: API_TIMEOUTS.LONG,
    });
  }

  async browseCategories(): Promise<EnhancedApiResponse<BrowseCategoriesResponse>> {
    return this.client.get('/discovery/browse/categories', {
      timeout: API_TIMEOUTS.STANDARD,
      retries: API_RETRIES.STANDARD,
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
      timeout: API_TIMEOUTS.LONG,
    });
  }

  async getConnectionRequests(): Promise<EnhancedApiResponse<{ requests: LocalConnectionRequest[] }>> {
    return this.client.get('/discovery/connection-requests', {
      timeout: API_TIMEOUTS.LONG,
    });
  }

  async respondToRequest(
    request_id: string,
    action: 'accept' | 'decline',
    message?: string,
  ): Promise<EnhancedApiResponse<{ request: LocalConnectionRequest }>> {
    return this.client.post('/discovery/connection-requests/respond', {
      request_id,
      action,
      message,
    }, {
      timeout: API_TIMEOUTS.LONG,
    });
  }

  async getSentRequests(): Promise<EnhancedApiResponse<{ requests: LocalSentRequest[] }>> {
    return this.client.get('/discovery/sent-requests', {
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

  // Calendar sharing methods

  async getCalendarSharingStatus(): Promise<EnhancedApiResponse<CalendarSharingStatus>> {
    return this.client.get('/discovery/calendar-sharing/status', {
      timeout: API_TIMEOUTS.STANDARD,
    });
  }

  async calendarSharingOptIn(): Promise<EnhancedApiResponse<CalendarSharingStatus>> {
    return this.client.post('/discovery/calendar-sharing/opt-in', {}, {
      timeout: API_TIMEOUTS.STANDARD,
    });
  }

  async calendarSharingOptOut(): Promise<EnhancedApiResponse<CalendarSharingStatus>> {
    return this.client.post('/discovery/calendar-sharing/opt-out', {}, {
      timeout: API_TIMEOUTS.STANDARD,
    });
  }

  async syncCalendarEvents(events: CalendarEventInput[]): Promise<EnhancedApiResponse<{ synced_count: number }>> {
    return this.client.post('/discovery/calendar-sharing/sync', { events }, {
      timeout: API_TIMEOUTS.LONG,
    });
  }

  async getSharedEvents(): Promise<EnhancedApiResponse<SharedEventsResponse>> {
    return this.client.get('/discovery/shared-events', {
      timeout: API_TIMEOUTS.STANDARD,
      retries: API_RETRIES.STANDARD,
    });
  }

  // Photo Moment Detection

  async listMomentOptIns(): Promise<EnhancedApiResponse<{ opt_ins: MomentOptIn[] }>> {
    return this.client.get('/discovery/moments/opt-ins', {
      timeout: API_TIMEOUTS.STANDARD,
    });
  }

  async momentOptIn(
    peer_pseudonym: string,
    peer_display_name?: string,
  ): Promise<EnhancedApiResponse<{ opt_ins: MomentOptIn[] }>> {
    return this.client.post('/discovery/moments/opt-in', {
      peer_pseudonym,
      peer_display_name: peer_display_name || undefined,
    }, {
      timeout: API_TIMEOUTS.STANDARD,
    });
  }

  async momentOptOut(
    peer_pseudonym: string,
  ): Promise<EnhancedApiResponse<{ opt_ins: MomentOptIn[] }>> {
    return this.client.post('/discovery/moments/opt-out', {
      peer_pseudonym,
    }, {
      timeout: API_TIMEOUTS.STANDARD,
    });
  }

  async momentScan(
    photos: PhotoMetadata[],
  ): Promise<EnhancedApiResponse<MomentScanResult>> {
    return this.client.post('/discovery/moments/scan', photos, {
      timeout: API_TIMEOUTS.LONG,
    });
  }

  async momentReceiveHashes(
    sender_pseudonym: string,
    hashes: string[],
  ): Promise<EnhancedApiResponse<void>> {
    return this.client.post('/discovery/moments/receive', {
      sender_pseudonym,
      hashes,
    }, {
      timeout: API_TIMEOUTS.STANDARD,
    });
  }

  async momentDetect(): Promise<EnhancedApiResponse<MomentDetectResult>> {
    return this.client.post('/discovery/moments/detect', {}, {
      timeout: API_TIMEOUTS.LONG,
    });
  }

  async listSharedMoments(): Promise<EnhancedApiResponse<{ moments: SharedMoment[] }>> {
    return this.client.get('/discovery/moments', {
      timeout: API_TIMEOUTS.STANDARD,
      retries: API_RETRIES.STANDARD,
    });
  }

  // Weekly Digest

  async getLatestDigest(): Promise<EnhancedApiResponse<{ digest: WeeklyDigest | null }>> {
    return this.client.get('/discovery/digest', {
      timeout: API_TIMEOUTS.STANDARD,
      retries: API_RETRIES.STANDARD,
    });
  }

  async listDigests(): Promise<EnhancedApiResponse<{ digests: WeeklyDigest[] }>> {
    return this.client.get('/discovery/digest/history', {
      timeout: API_TIMEOUTS.STANDARD,
      retries: API_RETRIES.STANDARD,
    });
  }

  async generateDigest(): Promise<EnhancedApiResponse<{ digest: WeeklyDigest }>> {
    return this.client.post('/discovery/digest/generate', {}, {
      timeout: API_TIMEOUTS.LONG,
    });
  }
}

export const discoveryClient = new DiscoveryClient();
export default discoveryClient;
