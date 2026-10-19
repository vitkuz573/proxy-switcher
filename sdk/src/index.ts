import createClient from 'openapi-fetch';
import type { components, paths } from './schema.js';

/* Re-exported contract types so consumers never import the raw schema. */
export type { components, paths };

export type Proxy = components['schemas']['Proxy'];
export type Status = components['schemas']['Status'];
export type Stats = components['schemas']['Stats'];
export type ScrapeStatus = components['schemas']['ScrapeStatus'];
export type Source = components['schemas']['Source'];
export type User = components['schemas']['User'];
export type ApiToken = components['schemas']['ApiToken'];
export type CreatedApiToken = components['schemas']['CreatedApiToken'];
export type AuditEntry = components['schemas']['AuditEntry'];
export type ApiError = components['schemas']['Error'];

export interface ClientOptions {
  /** Origin of the API service, e.g. `http://127.0.0.1:8088`. */
  baseUrl: string;
  /**
   * Bearer token for CLI use. Omit in the browser, where the HttpOnly session
   * cookie authenticates the request instead.
   */
  token?: string;
  /**
   * Extra headers. Used on the server to forward the caller's session cookie
   * during SSR.
   */
  headers?: Record<string, string>;
  /** Injection point for tests and server-side rendering. */
  fetch?: typeof globalThis.fetch;
}

/**
 * Typed client shared by the web app and the CLI.
 *
 * Both client models hit the same endpoints; only the credential differs. The
 * browser sends its session cookie, the CLI sends a bearer token.
 */
export function createProxySwitcherClient(options: ClientOptions) {
  const { baseUrl, token, headers, fetch: fetchImpl } = options;

  return createClient<paths>({
    baseUrl,
    credentials: 'include',
    ...(token ? { headers: { authorization: `Bearer ${token}`, ...headers } } : headers ? { headers } : {}),
    ...(fetchImpl ? { fetch: fetchImpl } : {}),
  });
}

export type ProxySwitcherClient = ReturnType<typeof createProxySwitcherClient>;

/**
 * Unwraps an `openapi-fetch` result, throwing on API errors so callers get a
 * plain value-or-throw surface instead of checking `{ data, error }` everywhere.
 */
export function unwrap<T>(result: { data?: T; error?: unknown; response: Response }): T {
  if (result.error !== undefined) {
    const body = result.error as { message?: string } | undefined;
    throw new Error(body?.message ?? `API request failed with ${result.response.status}`);
  }
  return result.data as T;
}