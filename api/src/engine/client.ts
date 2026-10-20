import type { Config } from "../config.js";
import type {
  Anonymity,
  LocalProxy,
  PoolStatistics,
  Problem,
  ProxyCredential,
  ProxyPolicy,
  ProxyPolicySettings,
  ProxyProtocol,
  ProxyRecord,
  RoutingInfo,
  ScrapeRun,
  Source,
  Stats,
  Summary,
} from "../schemas/index.js";

/** Raised when the engine is unreachable, slow, or returns an error. */
export class EngineError extends Error {
  constructor(
    message: string,
    readonly status: number | null,
    /** The engine's own problem document, when it sent one. */
    readonly problem?: Problem,
    override readonly cause?: unknown,
  ) {
    super(message);
    this.name = "EngineError";
  }
}

interface EngineOptions {
  baseUrl: string;
  timeoutMs: number;
  /** Injection point for contract tests. Defaults to the global fetch. */
  fetch?: typeof globalThis.fetch;
}

/**
 * Typed client for the internal engine.
 *
 * The engine mirrors the public REST shape exactly, so this is a thin
 * pass-through rather than a translator between two dialects.
 */
export class EngineClient {
  constructor(private readonly opts: EngineOptions) {}

  private get fetchImpl(): typeof globalThis.fetch {
    return this.opts.fetch ?? globalThis.fetch;
  }

  static fromConfig(config: Config, fetchImpl?: typeof globalThis.fetch): EngineClient {
    return new EngineClient({
      baseUrl: config.ENGINE_URL.replace(/\/$/, ""),
      timeoutMs: config.ENGINE_TIMEOUT_MS,
      ...(fetchImpl ? { fetch: fetchImpl } : {}),
    });
  }

  private async request<T>(method: string, path: string, body?: unknown): Promise<T> {
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), this.opts.timeoutMs);

    let response: Response;
    try {
      // Built conditionally: `exactOptionalPropertyTypes` forbids passing an
      // explicit `undefined` for an optional RequestInit field.
      const init: RequestInit = {
        method,
        signal: controller.signal,
        ...(body === undefined
          ? {}
          : {
              headers: { "content-type": "application/json" },
              body: JSON.stringify(body),
            }),
      };
      response = await this.fetchImpl(`${this.opts.baseUrl}${path}`, init);
    } catch (err) {
      // Never surface a raw fetch error to clients: it leaks the internal URL.
      throw new EngineError("engine unreachable", null, undefined, err);
    } finally {
      clearTimeout(timer);
    }

    if (!response.ok) {
      const text = await response.text().catch(() => "");
      // The engine speaks the same RFC 9457 dialect, so when it sent a problem
      // document its explanation is already written for a human.
      let parsed: Problem | undefined;
      try {
        const candidate = JSON.parse(text) as Partial<Problem>;
        if (typeof candidate.detail === "string" && typeof candidate.status === "number") {
          parsed = {
            type: candidate.type ?? "about:blank",
            title: candidate.title ?? "Engine error",
            status: candidate.status,
            detail: candidate.detail,
            ...(candidate.instance ? { instance: candidate.instance } : {}),
          };
        }
      } catch {
        parsed = undefined;
      }

      throw new EngineError(
        `engine returned ${response.status}`,
        response.status,
        parsed,
        text.slice(0, 500),
      );
    }

    // 201 without a body, 202 and 204 are acknowledgements, not payloads.
    if (response.status === 204 || response.status === 202) {
      return undefined as T;
    }

    const text = await response.text();
    if (text.length === 0) {
      return undefined as T;
    }

    try {
      return JSON.parse(text) as T;
    } catch (err) {
      throw new EngineError("engine returned malformed JSON", response.status, undefined, err);
    }
  }

  /* ── Proxies ────────────────────────────────────────────────────────── */

  listProxies(
    query: { limit?: number; offset?: number; sort?: string; search?: string } = {},
  ): Promise<ProxyRecord[]> {
    const params = new URLSearchParams();
    for (const [key, value] of Object.entries(query)) {
      if (value !== undefined) {
        params.set(key, String(value));
      }
    }
    const suffix = params.size > 0 ? `?${params}` : "";
    return this.request<ProxyRecord[]>("GET", `/api/v1/proxies${suffix}`);
  }

  getProxy(id: string): Promise<ProxyRecord> {
    return this.request<ProxyRecord>("GET", `/api/v1/proxies/${encodeURIComponent(id)}`);
  }

  createProxy(input: {
    host: string;
    port: number;
    protocol: ProxyProtocol;
    country?: string;
  }): Promise<ProxyRecord> {
    return this.request<ProxyRecord>("POST", "/api/v1/proxies", input);
  }

  deleteProxy(id: string): Promise<void> {
    return this.request<void>("DELETE", `/api/v1/proxies/${encodeURIComponent(id)}`);
  }

  /* ── Active proxy (singleton) ───────────────────────────────────────── */

  getActiveProxy(): Promise<Summary> {
    return this.request<Summary>("GET", "/api/v1/active-proxy");
  }

  setActiveProxy(proxyId: string): Promise<ProxyRecord> {
    return this.request<ProxyRecord>(
      "PUT",
      `/api/v1/active-proxy?proxy_id=${encodeURIComponent(proxyId)}`,
    );
  }

  clearActiveProxy(): Promise<void> {
    return this.request<void>("DELETE", "/api/v1/active-proxy");
  }

  rotateActiveProxy(): Promise<ProxyRecord> {
    return this.request<ProxyRecord>("POST", "/api/v1/active-proxy/rotate");
  }

  /* ── Sources ────────────────────────────────────────────────────────── */

  listSources(): Promise<Source[]> {
    return this.request<Source[]>("GET", "/api/v1/sources");
  }

  createSource(url: string): Promise<Source> {
    return this.request<Source>("POST", "/api/v1/sources", { url });
  }

  deleteSource(id: string): Promise<void> {
    return this.request<void>("DELETE", `/api/v1/sources/${encodeURIComponent(id)}`);
  }

  /* ── Scrape runs ────────────────────────────────────────────────────── */

  createScrapeRun(): Promise<void> {
    return this.request<void>("POST", "/api/v1/scrape-runs");
  }

  getCurrentScrapeRun(): Promise<ScrapeRun> {
    return this.request<ScrapeRun>("GET", "/api/v1/scrape-runs/current");
  }

  /* ── Telemetry ──────────────────────────────────────────────────────── */

  getSummary(): Promise<Summary> {
    return this.request<Summary>("GET", "/api/v1/summary");
  }

  getStatistics(): Promise<PoolStatistics> {
    return this.request<PoolStatistics>("GET", "/api/v1/statistics");
  }

  getRouting(): Promise<RoutingInfo> {
    return this.request<RoutingInfo>("GET", "/api/v1/routing");
  }

  getStats(): Promise<Stats> {
    return this.request<Stats>("GET", "/api/v1/stats");
  }

  getDns(): Promise<Array<{ ip: string; hostname: string }>> {
    return this.request<Array<{ ip: string; hostname: string }>>("GET", "/api/v1/dns");
  }

  /* ── The local proxy, its policies and its credentials ──────────────── */

  getLocalProxy(): Promise<LocalProxy> {
    return this.request<LocalProxy>("GET", "/api/v1/local-proxy");
  }

  listPolicies(): Promise<ProxyPolicy[]> {
    return this.request<ProxyPolicy[]>("GET", "/api/v1/policies");
  }

  createPolicy(policy: { name: string; settings: ProxyPolicySettings }): Promise<ProxyPolicy> {
    return this.request<ProxyPolicy>("POST", "/api/v1/policies", policy);
  }

  deletePolicy(name: string): Promise<void> {
    return this.request<void>("DELETE", `/api/v1/policies/${encodeURIComponent(name)}`);
  }

  listCredentials(): Promise<ProxyCredential[]> {
    return this.request<ProxyCredential[]>("GET", "/api/v1/credentials");
  }

  createCredential(credential: { username: string; policy: string }): Promise<ProxyCredential> {
    return this.request<ProxyCredential>("POST", "/api/v1/credentials", credential);
  }

  deleteCredential(username: string): Promise<void> {
    return this.request<void>("DELETE", `/api/v1/credentials/${encodeURIComponent(username)}`);
  }
}

export async function isEngineHealthy(client: EngineClient): Promise<boolean> {
  try {
    await client.getSummary();
    return true;
  } catch {
    return false;
  }
}

export type { Anonymity };
