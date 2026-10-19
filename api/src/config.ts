import { z } from 'zod';

/**
 * Configuration is validated once at boot. A malformed environment must fail
 * the process immediately rather than surface as a confusing runtime error
 * later.
 */
const schema = z.object({
  NODE_ENV: z.enum(['development', 'test', 'production']).default('development'),

  /** Public API bind address. Loopback by default; changing it is deliberate. */
  API_HOST: z.string().default('127.0.0.1'),
  API_PORT: z.coerce.number().int().min(1).max(65535).default(8080),

  /** Internal Rust engine. Never exposed publicly. */
  ENGINE_URL: z.string().url().default('http://127.0.0.1:9099'),
  ENGINE_TIMEOUT_MS: z.coerce.number().int().min(100).default(15_000),

  /** Auth database. Separate from the engine's own proxy store. */
  DATABASE_PATH: z.string().default('/var/lib/proxy-switcher/auth.db'),

  /** Signing key for session cookies. Required in production. */
  SESSION_SECRET: z.string().min(32).optional(),

  /** First-run administrator. Only used when the users table is empty. */
  BOOTSTRAP_ADMIN_EMAIL: z.string().email().optional(),
  /** Minimum lowered to 8 by operator decision (2026-10-19). */
  BOOTSTRAP_ADMIN_PASSWORD: z.string().min(8).optional(),

  SESSION_TTL_HOURS: z.coerce.number().int().min(1).max(24 * 30).default(24),
  API_TOKEN_TTL_DAYS: z.coerce.number().int().min(1).max(365).default(90),

  /** Origins allowed to call the API with credentials (the Next.js app). */
  CORS_ORIGINS: z
    .string()
    .default('')
    .transform((v) =>
      v
        .split(',')
        .map((o) => o.trim())
        .filter(Boolean),
    ),

  /** Rate limit ceiling per principal, per window. */
  RATE_LIMIT_MAX: z.coerce.number().int().min(1).default(300),
  RATE_LIMIT_WINDOW_MS: z.coerce.number().int().min(1000).default(60_000),

  LOG_LEVEL: z.enum(['fatal', 'error', 'warn', 'info', 'debug', 'trace']).default('info'),
});

export type RawConfig = z.infer<typeof schema>;

/**
 * Resolved configuration. `SESSION_SECRET` is guaranteed to be a string after
 * `loadConfig` — it is either supplied or replaced with a development value.
 */
export type Config = Omit<RawConfig, 'SESSION_SECRET'> & { SESSION_SECRET: string };

export function loadConfig(env: NodeJS.ProcessEnv = process.env): Config {
  const parsed = schema.safeParse(env);

  if (!parsed.success) {
    const issues = parsed.error.issues
      .map((i) => `  - ${i.path.join('.')}: ${i.message}`)
      .join('\n');
    throw new Error(`Invalid configuration:\n${issues}`);
  }

  const config = parsed.data;

  if (config.NODE_ENV === 'production' && !config.SESSION_SECRET) {
    throw new Error('SESSION_SECRET is required in production (min 32 chars).');
  }

  if (config.BOOTSTRAP_ADMIN_EMAIL && !config.BOOTSTRAP_ADMIN_PASSWORD) {
    throw new Error('BOOTSTRAP_ADMIN_PASSWORD must be set when BOOTSTRAP_ADMIN_EMAIL is.');
  }

  return {
    ...config,
    SESSION_SECRET: config.SESSION_SECRET ?? 'dev-only-insecure-session-secret-value',
  };
}