import { chmodSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join } from "node:path";

export interface CliConfig {
  baseUrl: string;
  /** Bearer token issued by the API. */
  token?: string;
}

const CONFIG_DIR =
  process.env.PROXY_SWITCHER_CONFIG_DIR ?? join(homedir(), ".config", "proxy-switcher");
const CONFIG_PATH = join(CONFIG_DIR, "config.json");

const DEFAULT_BASE_URL = process.env.PROXY_SWITCHER_API_URL ?? "http://127.0.0.1:8088";

export function loadConfig(): CliConfig {
  try {
    const raw = readFileSync(CONFIG_PATH, "utf8");
    const parsed = JSON.parse(raw) as Partial<CliConfig>;
    return {
      baseUrl: parsed.baseUrl ?? DEFAULT_BASE_URL,
      ...(parsed.token ? { token: parsed.token } : {}),
    };
  } catch {
    return { baseUrl: DEFAULT_BASE_URL };
  }
}

/** Persisted with owner-only permissions: it holds a live credential. */
export function saveConfig(config: CliConfig): void {
  mkdirSync(dirname(CONFIG_PATH), { recursive: true });
  writeFileSync(CONFIG_PATH, `${JSON.stringify(config, null, 2)}\n`, { mode: 0o600 });
  chmodSync(CONFIG_PATH, 0o600);
}

export function configPath(): string {
  return CONFIG_PATH;
}
