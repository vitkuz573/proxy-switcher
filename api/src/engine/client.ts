import type { Config } from '../config.js';
import type {
  Proxy,
  ProxyProtocol,
  ScrapeStatus,
  Source,
  Stats,
  Status,
} from '../schemas/index.js';

/** Raised when the engine is unreachable, slow, or returns an error. */
export class EngineError extends Error {
  constructor(
    message: string,
    readonly status: number | null,
    override readonly cause?: unknown,
  ) {
    super(message);
    this.name = 'EngineError';
  }
}

interface EngineOptions {
  baseUrl: string;
  timeoutMs: number;
}

export class EngineClient {
  constructor(private readonly opts: EngineOptions) {}

  static fromConfig(config: Config): EngineClient {
    return new EngineClient({
      baseUrl: config.ENGINE_URL.replace(/\/$/, ''),
      timeoutMs: config.ENGINE_TIMEOUT_MS,
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
              headers: { 'content-type': 'application/json' },
              body: JSON.stringify(body),
            }),
      };
      response = await fetch(`${this.opts.baseUrl}${path}`, init);
    } catch (err) {
      // Never surface a raw fetch error to clients: it leaks the internal URL.
      throw new EngineError('engine unreachable', null, err);
    } finally {
      clearTimeout(timer);
    }

    if (!response.ok) {
      const text = await response.text().catch(() => '');
      throw new EngineError(
        `engine returned ${response.status}`,
        response.status,
        text.slice(0, 500),
      );
    }

    if (response.status === 204) {
      return undefined as T;
    }

    try {
      return (await response.json()) as T;
    } catch (err) {
      throw new EngineError('engine returned malformed JSON', response.status, err);
    }
  }

  getStatus(): Promise<Status> {
    return this.request<Status>('GET', '/api/v1/status');
  }

  getStats(): Promise<Stats> {
    return this.request<Stats>('GET', '/api/v1/stats');
  }

  getScrapeStatus(): Promise<ScrapeStatus> {
    return this.request<ScrapeStatus>('GET', '/api/v1/scrape/status');
  }

  triggerScrape(): Promise<void> {
    return this.request<void>('POST', '/api/v1/scrape');
  }

  listProxies(): Promise<Proxy[]> {
    return this.request<Proxy[]>('GET', '/api/v1/proxies');
  }

  addProxy(input: {
    host: string;
    port: number;
    protocol: ProxyProtocol;
    anonymity?: Proxy['anonymity'];
    country?: string;
  }): Promise<Proxy> {
    return this.request<Proxy>('POST', '/api/v1/proxies', input);
  }

  deleteProxy(id: string): Promise<void> {
    return this.request<void>('DELETE', `/api/v1/proxies/${encodeURIComponent(id)}`);
  }

  switchProxy(id: string): Promise<Status> {
    return this.request<Status>('POST', '/api/v1/switch', { id });
  }

  rotate(): Promise<Status> {
    return this.request<Status>('POST', '/api/v1/rotate');
  }

  listSources(): Promise<Source[]> {
    return this.request<Source[]>('GET', '/api/v1/sources');
  }

  addSource(url: string): Promise<Source> {
    return this.request<Source>('POST', '/api/v1/sources', { url });
  }

  deleteSource(url: string): Promise<void> {
    return this.request<void>('DELETE', `/api/v1/sources/${encodeURIComponent(url)}`);
  }
}

export async function isEngineHealthy(client: EngineClient): Promise<boolean> {
  try {
    await client.getStatus();
    return true;
  } catch {
    return false;
  }
}