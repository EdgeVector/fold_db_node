// Subscription API Client — calls the Exemem cloud API directly for storage tier management

export interface StorageInfo {
  used_bytes: number;
  quota_bytes: number;
  plan: string;
}

export interface SubscriptionStatus {
  ok: boolean;
  plan: string;
  storage: {
    used_bytes: number;
    quota_bytes: number;
  };
  has_subscription: boolean;
}

interface CloudApiConfig {
  apiUrl: string;
  apiKey: string;
}

function getCloudConfig(): CloudApiConfig | null {
  const apiUrl = localStorage.getItem("exemem_api_url");
  const apiKey = localStorage.getItem("exemem_api_key");
  if (!apiUrl || !apiKey) return null;
  return { apiUrl, apiKey };
}

async function cloudFetch(
  path: string,
  options: RequestInit = {},
): Promise<any> {
  const config = getCloudConfig();
  if (!config) {
    throw new Error("Not connected to Exemem cloud");
  }

  const url = `${config.apiUrl.replace(/\/$/, "")}${path}`;
  const resp = await fetch(url, {
    ...options,
    headers: {
      "Content-Type": "application/json",
      "X-API-Key": config.apiKey,
      ...(options.headers || {}),
    },
  });

  if (!resp.ok) {
    const body = await resp.text();
    throw new Error(`Cloud API error (${resp.status}): ${body}`);
  }

  return resp.json();
}

export async function getSubscriptionStatus(): Promise<SubscriptionStatus> {
  return cloudFetch("/api/subscription/status", { method: "GET" });
}

export async function getStorageInfo(): Promise<StorageInfo> {
  const result = await cloudFetch("/api/sync/storage", {
    method: "POST",
    body: JSON.stringify({ action: "get_storage" }),
  });
  if (!result.ok) throw new Error(result.error || "Failed to get storage info");
  return result.storage;
}

export async function createCheckoutSession(
  successUrl?: string,
  cancelUrl?: string,
): Promise<string> {
  const result = await cloudFetch("/api/subscription/create-checkout", {
    method: "POST",
    body: JSON.stringify({
      success_url: successUrl || `${window.location.origin}?subscription=success`,
      cancel_url: cancelUrl || `${window.location.origin}?subscription=cancelled`,
    }),
  });
  if (!result.ok) throw new Error(result.error || "Failed to create checkout");
  return result.url;
}

export async function createPortalSession(
  returnUrl?: string,
): Promise<string> {
  const result = await cloudFetch("/api/subscription/portal", {
    method: "POST",
    body: JSON.stringify({
      return_url: returnUrl || window.location.origin,
    }),
  });
  if (!result.ok) throw new Error(result.error || "Failed to create portal session");
  return result.url;
}

export function formatBytes(bytes: number): string {
  if (bytes <= 0) return "0 B";
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024)
    return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(1)} GB`;
}

export function usagePercent(used: number, quota: number): number {
  if (quota <= 0) return 0;
  return Math.min(100, (used / quota) * 100);
}
