import type {
  ApiToken,
  AuditEvent,
  CreatedApiToken,
  PoolStatistics,
  Problem,
  ProxyProtocol,
  ProxyRecord,
  RoutingInfo,
  ScrapeRun,
  Source,
  Stats,
  Summary,
  TokenExchange,
  User,
} from "./schemas.js";

export * from "./schemas.js";

/** Media type for RFC 9457 problem documents. */
export const PROBLEM_CONTENT_TYPE = "application/problem+json";

/**
 * A failed API call, carrying the server's problem document.
 *
 * `detail` is written for humans; `problem.status` is the code to branch on.
 * Never stringify this into logs without checking `problem` first — it can
 * contain client-supplied input.
 */
export class ApiError extends Error {
  constructor(
    readonly status: number,
    readonly problem: Problem,
    readonly instance: string,
  ) {
    super(problem.detail || problem.title);
    this.name = "ApiError";
  }
}

export interface ClientOptions {
  /** Origin of the API service, e.g. `http://127.0.0.1:8088`. */
  baseUrl: string;
  /**
   * Bearer token for CLI use. Omit in the browser, where the HttpOnly session
   * cookie authenticates the request instead.
   */
  token?: string;
  /** Extra headers, e.g. the caller's cookie forwarded during SSR. */
  headers?: Record<string, string>;
  /** Injection point for tests and server-side rendering. */
  fetch?: typeof globalThis.fetch;
}

export interface ListProxiesQuery {
  limit?: number;
  offset?: number;
  sort?: "score" | "latency" | "host";
  search?: string;
}

export interface ListPage {
  limit?: number;
  offset?: number;
}

const DEFAULT_LIMIT = 50;

/**
 * Typed client over the public REST contract.
 *
 * Written against `fetch` rather than generated: the contract is defined in
 * `schemas.ts`, so the types here are checked at compile time against the same
 * definitions the server validates against. There is no generated layer to fall
 * out of sync.
 */
export class ProxySwitcherClient {
  private readonly baseUrl: string;
  private readonly extraHeaders: Record<string, string>;
  private readonly fetchImpl: typeof globalThis.fetch;

  constructor(options: ClientOptions) {
    this.baseUrl = options.baseUrl.replace(/\/$/, "");
    this.fetchImpl = options.fetch ?? globalThis.fetch;
    this.extraHeaders = {
      ...(options.token ? { authorization: `Bearer ${options.token}` } : {}),
      ...options.headers,
    };
  }

  private path(path: string, query?: Record<string, string | number | undefined>): string {
    const params = new URLSearchParams();
    for (const [key, value] of Object.entries(query ?? {})) {
      if (value !== undefined) {
        params.set(key, String(value));
      }
    }
    const search = params.size > 0 ? `?${params}` : "";
    return `${this.baseUrl}${path}${search}`;
  }

  private async request<T>(
    method: string,
    path: string,
    options: { query?: Record<string, string | number | undefined>; body?: unknown } = {},
  ): Promise<T> {
    const hasBody = options.body !== undefined;

    let response: Response;
    try {
      response = await this.fetchImpl(this.path(path, options.query), {
        method,
        credentials: "include",
        headers: {
          ...this.extraHeaders,
          ...(hasBody ? { "content-type": "application/json" } : {}),
        },
        ...(hasBody ? { body: JSON.stringify(options.body) } : {}),
      });
    } catch (cause) {
      throw new ApiError(
        0,
        {
          type: "about:blank",
          title: "Service unreachable",
          status: 0,
          detail: cause instanceof Error ? cause.message : "Request failed",
          instance: path,
        },
        path,
      );
    }

    // 204 and 201-without-body are acknowledgements, not payloads.
    if (response.status === 204 || response.status === 201) {
      const created = await readJson(response);
      return (created ?? undefined) as T;
    }

    if (!response.ok) {
      const body = (await readJson(response)) as Partial<Problem> | null;
      throw new ApiError(
        response.status,
        {
          type: body?.type ?? "about:blank",
          title: body?.title ?? response.statusText,
          status: body?.status ?? response.status,
          detail: body?.detail ?? `Request failed with ${response.status}`,
          ...(body?.instance ? { instance: body.instance } : {}),
        },
        path,
      );
    }

    return (await readJson(response)) as T;
  }

  /* ── Operations ─────────────────────────────────────────────────────── */

  healthz(): Promise<{ status: string }> {
    return this.request("GET", "/healthz");
  }

  readyz(): Promise<{ status: string; engine: "up" | "down" }> {
    return this.request("GET", "/readyz");
  }

  /* ── Sessions ───────────────────────────────────────────────────────── */

  createSession(credentials: { email: string; password: string }): Promise<{ user: User }> {
    return this.request("POST", "/api/v1/sessions", { body: credentials });
  }

  getSession(): Promise<{ user: User }> {
    return this.request("GET", "/api/v1/sessions/current");
  }

  deleteSession(): Promise<void> {
    return this.request("DELETE", "/api/v1/sessions/current");
  }

  /* ── Tokens ─────────────────────────────────────────────────────────── */

  /** Credential grant for clients with no cookie jar, such as the CLI. */
  exchangeToken(credentials: {
    email: string;
    password: string;
    name?: string;
  }): Promise<TokenExchange> {
    return this.request("POST", "/api/v1/tokens/exchange", { body: credentials });
  }

  createToken(name: string): Promise<CreatedApiToken> {
    return this.request("POST", "/api/v1/tokens", { body: { name } });
  }

  listTokens(): Promise<ApiToken[]> {
    return this.request("GET", "/api/v1/tokens");
  }

  deleteToken(id: string): Promise<void> {
    return this.request("DELETE", `/api/v1/tokens/${encodeURIComponent(id)}`);
  }

  /* ── Proxies ────────────────────────────────────────────────────────── */

  listProxies(query: ListProxiesQuery = {}): Promise<ProxyRecord[]> {
    return this.request("GET", "/api/v1/proxies", {
      query: { limit: DEFAULT_LIMIT, ...query },
    });
  }

  getProxy(id: string): Promise<ProxyRecord> {
    return this.request("GET", `/api/v1/proxies/${encodeURIComponent(id)}`);
  }

  createProxy(input: {
    host: string;
    port: number;
    protocol?: ProxyProtocol;
    country?: string;
  }): Promise<ProxyRecord> {
    return this.request("POST", "/api/v1/proxies", { body: input });
  }

  deleteProxy(id: string): Promise<void> {
    return this.request("DELETE", `/api/v1/proxies/${encodeURIComponent(id)}`);
  }

  /* ── Active proxy ───────────────────────────────────────────────────── */

  getActiveProxy(): Promise<Summary> {
    return this.request("GET", "/api/v1/active-proxy");
  }

  setActiveProxy(proxyId: string): Promise<ProxyRecord> {
    return this.request("PUT", "/api/v1/active-proxy", { body: { proxy_id: proxyId } });
  }

  clearActiveProxy(): Promise<void> {
    return this.request("DELETE", "/api/v1/active-proxy");
  }

  rotateActiveProxy(): Promise<ProxyRecord> {
    return this.request("POST", "/api/v1/active-proxy/rotate");
  }

  /* ── Scrape sources ─────────────────────────────────────────────────── */

  listSources(): Promise<Source[]> {
    return this.request("GET", "/api/v1/sources");
  }

  createSource(url: string): Promise<Source> {
    return this.request("POST", "/api/v1/sources", { body: { url } });
  }

  deleteSource(id: string): Promise<void> {
    return this.request("DELETE", `/api/v1/sources/${encodeURIComponent(id)}`);
  }

  /* ── Scrape runs ────────────────────────────────────────────────────── */

  createScrapeRun(): Promise<void> {
    return this.request("POST", "/api/v1/scrape-runs");
  }

  getCurrentScrapeRun(): Promise<ScrapeRun> {
    return this.request("GET", "/api/v1/scrape-runs/current");
  }

  /* ── Telemetry and audit ────────────────────────────────────────────── */

  getSummary(): Promise<Summary> {
    return this.request("GET", "/api/v1/summary");
  }

  getStats(): Promise<Stats> {
    return this.request("GET", "/api/v1/stats");
  }

  /** The distribution of the pool, and how it has moved. */
  getStatistics(): Promise<PoolStatistics> {
    return this.request("GET", "/api/v1/statistics");
  }

  /** The routing policy in force, and the state of the TUN bypass. */
  getRouting(): Promise<RoutingInfo> {
    return this.request("GET", "/api/v1/routing");
  }

  listAuditEvents(page: ListPage = {}): Promise<AuditEvent[]> {
    return this.request("GET", "/api/v1/audit-events", {
      query: { limit: 100, offset: 0, ...page },
    });
  }
}

async function readJson(response: Response): Promise<unknown> {
  const text = await response.text();
  if (text.length === 0) {
    return null;
  }
  try {
    return JSON.parse(text);
  } catch {
    return null;
  }
}

export function createProxySwitcherClient(options: ClientOptions): ProxySwitcherClient {
  return new ProxySwitcherClient(options);
}
