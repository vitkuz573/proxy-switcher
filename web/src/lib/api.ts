import 'server-only';
import { cookies } from 'next/headers';
import {
  createProxySwitcherClient,
  type AuditEntry,
  type Proxy,
  type ScrapeStatus,
  type Source,
  type Stats,
  type Status,
} from '@proxy-switcher/sdk';
import { API_INTERNAL_URL, SESSION_COOKIE } from './env';

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
export async function getStatus(): Promise<Status | null> {
  const { data, error } = await (await serverClient()).GET('/api/v1/status');
  return error ? null : (data ?? null);
}

export async function getStats(): Promise<Stats | null> {
  const { data, error } = await (await serverClient()).GET('/api/v1/stats');
  return error ? null : (data ?? null);
}

export async function getProxies(): Promise<Proxy[]> {
  const { data, error } = await (await serverClient()).GET('/api/v1/proxies');
  return error ? [] : ((data as Proxy[] | undefined) ?? []);
}

export async function getScrapeStatus(): Promise<ScrapeStatus | null> {
  const { data, error } = await (await serverClient()).GET('/api/v1/scrape/status');
  return error ? null : (data ?? null);
}

export async function getSources(): Promise<Source[]> {
  const { data, error } = await (await serverClient()).GET('/api/v1/sources');
  return error ? [] : ((data as Source[] | undefined) ?? []);
}

export async function getAudit(limit = 100): Promise<AuditEntry[]> {
  const { data, error } = await (await serverClient()).GET('/api/v1/audit', {
    params: { query: { limit } },
  });
  return error ? [] : ((data as AuditEntry[] | undefined) ?? []);
}