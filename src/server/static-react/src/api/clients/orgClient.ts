import { ApiClient, getSharedClient } from '../core/client';
import { API_TIMEOUTS, API_RETRIES } from '../../constants/api';
import type { EnhancedApiResponse } from '../core/types';

export interface CloudMember {
  user_hash: string;
  role: string;
  status: string;
}

export interface OrgMemberInfo {
  node_public_key: string;
  display_name: string;
  added_at: number;
  added_by: string;
}

export interface OrgInviteBundle {
  org_name: string;
  org_hash: string;
  org_public_key: string;
  org_e2e_secret: string;
  members: OrgMemberInfo[];
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

  async leaveOrg(orgHash: string): Promise<EnhancedApiResponse<void>> {
    return this.client.post(`/org/${orgHash}/leave`, undefined, {
      timeout: API_TIMEOUTS.STANDARD,
    });
  }

  async deleteOrg(orgHash: string): Promise<EnhancedApiResponse<void>> {
    return this.client.delete(`/org/${orgHash}`, {
      timeout: API_TIMEOUTS.STANDARD,
    });
  }

  async getCloudMembers(orgHash: string): Promise<EnhancedApiResponse<CloudMember[]>> {
    return this.client.get(`/org/${orgHash}/cloud-members`, {
      timeout: API_TIMEOUTS.STANDARD,
      retries: API_RETRIES.STANDARD,
    });
  }
}

export const orgClient = new OrgClient();
export default orgClient;
