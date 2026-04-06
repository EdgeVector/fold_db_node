import { ApiClient, getSharedClient } from '../core/client';
import { API_TIMEOUTS, API_RETRIES } from '../../constants/api';
import type { EnhancedApiResponse } from '../core/types';

export interface OrgInviteBundle {
  org_hash: string;
  org_name?: string;
  org_e2e_secret: string;
  timestamp: string;
  invited_by: string;
  invited_role: string;
}

export class OrgClient {
  private readonly client: ApiClient;

  constructor(client?: ApiClient) {
    this.client = client || getSharedClient();
  }

  async getPendingInvites(): Promise<EnhancedApiResponse<OrgInviteBundle[]>> {
    return this.client.get('/org/invites/pending', {
      timeout: API_TIMEOUTS.STANDARD,
      retries: API_RETRIES.STANDARD,
    });
  }

  async joinOrg(bundle: OrgInviteBundle): Promise<EnhancedApiResponse<void>> {
    return this.client.post('/org/join', bundle, {
      timeout: API_TIMEOUTS.STANDARD,
    });
  }

  async declineInvite(orgHash: string): Promise<EnhancedApiResponse<void>> {
    return this.client.post(`/org/invites/${orgHash}/decline`, undefined, {
      timeout: API_TIMEOUTS.STANDARD,
    });
  }

  async listOrgs(): Promise<EnhancedApiResponse<Record<string, unknown>[]>> {
    return this.client.get('/org', {
      timeout: API_TIMEOUTS.STANDARD,
    });
  }
}

export const orgClient = new OrgClient();
export default orgClient;
