import "server-only";
import {
  type AuditEvent,
  createProxySwitcherClient,
  type LocalProxy,
  type PoolStatistics,
  type ProxyCredential,
  type ProxyPolicy,
  type ProxyRecord,
  type RoutingInfo,
  type ScrapeRun,
  type Source,
  type Stats,
  type Summary,
} from "@proxy-switcher/sdk";
import { cookies } from "next/headers";
import { API_INTERNAL_URL, SESSION_COOKIE } from "./env";

/**
 * Server-side API client.
 *
 * The caller's session cookie is forwarded so the API sees the same identity the
 * browser has. The cookie is HttpOnly, so forwarding it is the only way a
 * server component can authenticate.
 */
async function serverClient() {
  const session = (await cookies()).get(SESSION_COOKIE)?.value;

  return createProxySwitcherClient({
    baseUrl: API_INTERNAL_URL,
    ...(session ? { headers: { cookie: `${SESSION_COOKIE}=${session}` } } : {}),
  });
}

/**
 * Every reader returns null instead of throwing when the session is missing or
 * the API is unreachable. Pages decide how to present that; the session itself
 * is validated in `lib/auth.ts`.
 */
async function read<T>(operation: () => Promise<T>, fallback: T): Promise<T> {
  try {
    return await operation();
  } catch {
    return fallback;
  }
}

export async function getSummary(): Promise<Summary | null> {
  return read(() => serverClient().then((api) => api.getSummary()), null);
}

export async function getStatistics(): Promise<PoolStatistics | null> {
  return read(() => serverClient().then((api) => api.getStatistics()), null);
}

export async function getRouting(): Promise<RoutingInfo | null> {
  return read(() => serverClient().then((api) => api.getRouting()), null);
}

export async function getStats(): Promise<Stats | null> {
  return read(() => serverClient().then((api) => api.getStats()), null);
}

export async function getLocalProxy(): Promise<LocalProxy | null> {
  return read(() => serverClient().then((api) => api.getLocalProxy()), null);
}

export async function getPolicies(): Promise<ProxyPolicy[]> {
  return read(() => serverClient().then((api) => api.listPolicies()), []);
}

export async function getCredentials(): Promise<ProxyCredential[]> {
  return read(() => serverClient().then((api) => api.listCredentials()), []);
}

export async function getProxies(limit = 100): Promise<ProxyRecord[]> {
  return read(() => serverClient().then((api) => api.listProxies({ limit })), []);
}

export async function getScrapeRun(): Promise<ScrapeRun | null> {
  return read(() => serverClient().then((api) => api.getCurrentScrapeRun()), null);
}

export async function getSources(): Promise<Source[]> {
  return read(() => serverClient().then((api) => api.listSources()), []);
}

export async function getAuditEvents(limit = 100): Promise<AuditEvent[]> {
  return read(() => serverClient().then((api) => api.listAuditEvents({ limit })), []);
}
