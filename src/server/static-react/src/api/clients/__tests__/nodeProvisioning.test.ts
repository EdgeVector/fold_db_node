import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { ApiError } from '../../core/errors';
import {
  installNodeProvisioningHandler,
  NODE_NOT_PROVISIONED_EVENT,
  __resetForTests,
} from '../nodeProvisioning';
import { getSharedClient } from '../../core/client';

describe('installNodeProvisioningHandler', () => {
  beforeEach(() => {
    __resetForTests();
  });

  afterEach(() => {
    // Avoid leaking the interceptor across files. The shared client is a
    // singleton; resetting the install flag lets the next test re-arm but
    // does NOT remove the previously-attached interceptor — that's fine for
    // the assertions below, which only care about dispatch behavior.
    __resetForTests();
  });

  it('emits the node-not-provisioned event when an interceptor sees a matching 503', async () => {
    installNodeProvisioningHandler();

    const handler = vi.fn();
    window.addEventListener(NODE_NOT_PROVISIONED_EVENT, handler);

    // Synthesize an ApiError with the canonical body that the future
    // backend follow-up will return.
    const err = new ApiError('node_not_provisioned', 503, {
      response: { error: 'node_not_provisioned' },
    });

    // Reach into the shared client and run all error interceptors. We do
    // this rather than spinning up a real fetch + 503 mock because
    // performRequest goes through fetch + handleResponse, and the goal is
    // to verify the interceptor logic — not the entire stack.
    const client = getSharedClient() as unknown as {
      errorInterceptors: Array<(_e: ApiError) => Promise<ApiError> | ApiError>;
    };
    for (const interceptor of client.errorInterceptors) {
      await interceptor(err);
    }

    expect(handler).toHaveBeenCalledTimes(1);
    window.removeEventListener(NODE_NOT_PROVISIONED_EVENT, handler);
  });

  it('does not emit on unrelated 503 bodies', async () => {
    installNodeProvisioningHandler();

    const handler = vi.fn();
    window.addEventListener(NODE_NOT_PROVISIONED_EVENT, handler);

    const err = new ApiError('node manager not initialized', 503, {
      response: { error: 'node_not_initialized' },
    });

    const client = getSharedClient() as unknown as {
      errorInterceptors: Array<(_e: ApiError) => Promise<ApiError> | ApiError>;
    };
    for (const interceptor of client.errorInterceptors) {
      await interceptor(err);
    }

    expect(handler).not.toHaveBeenCalled();
    window.removeEventListener(NODE_NOT_PROVISIONED_EVENT, handler);
  });

  it('is idempotent on repeat install', () => {
    const client = getSharedClient() as unknown as {
      errorInterceptors: unknown[];
    };
    const before = client.errorInterceptors.length;

    installNodeProvisioningHandler();
    installNodeProvisioningHandler();
    installNodeProvisioningHandler();

    // Exactly one interceptor added across the three calls.
    expect(client.errorInterceptors.length).toBe(before + 1);
  });
});
