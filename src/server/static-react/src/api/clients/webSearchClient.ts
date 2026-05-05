// Web Search API Client — manages the persisted Brave Search API key.
//
// Kept tiny on purpose. The endpoint set is just GET/POST `/web_search/key`
// (a save with an empty body deletes the saved key), so a one-off client
// is clearer than another method on UnifiedIngestionClient.

import { ApiClient, getSharedClient } from "../core/client";
import { API_TIMEOUTS, API_RETRIES } from "../../constants/api";
import type { EnhancedApiResponse } from "../core/types";

export interface WebSearchKeyStatus {
  has_key: boolean;
}

const WEB_SEARCH_KEY_PATH = "/web_search/key";

export class UnifiedWebSearchClient {
  private readonly client: ApiClient;

  constructor(client?: ApiClient) {
    this.client = client || getSharedClient();
  }

  /** Returns `{ has_key }` — true when a key is persisted on disk. */
  async getKeyStatus(): Promise<EnhancedApiResponse<WebSearchKeyStatus>> {
    return this.client.get<WebSearchKeyStatus>(WEB_SEARCH_KEY_PATH, {
      timeout: API_TIMEOUTS.QUICK,
      retries: API_RETRIES.STANDARD,
      cacheable: false,
    });
  }

  /**
   * Persist the key. Pass an empty string to delete the saved key.
   * Returns the new presence flag.
   */
  async saveKey(
    apiKey: string,
  ): Promise<EnhancedApiResponse<WebSearchKeyStatus>> {
    return this.client.post<WebSearchKeyStatus>(
      WEB_SEARCH_KEY_PATH,
      { api_key: apiKey },
      {
        timeout: API_TIMEOUTS.QUICK,
        retries: API_RETRIES.LIMITED,
      },
    );
  }
}

export const webSearchClient = new UnifiedWebSearchClient();
