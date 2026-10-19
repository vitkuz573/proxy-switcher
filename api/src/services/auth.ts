import { createHash, randomBytes, randomUUID, timingSafeEqual } from "node:crypto";
import type { DatabaseSync } from "node:sqlite";
import { hash, verify } from "@node-rs/argon2";
import {
  type ApiTokenRow,
  namedParams,
  type Role,
  type SessionRow,
  type UserRow,
} from "../db/index.js";

const ARGON2_OPTIONS = {
  memoryCost: 19_456,
  timeCost: 2,
  parallelism: 1,
} as const;

export function hashPassword(plain: string): Promise<string> {
  return hash(plain, ARGON2_OPTIONS);
}

export async function verifyPassword(digest: string, plain: string): Promise<boolean> {
  try {
    return await verify(digest, plain, ARGON2_OPTIONS);
  } catch {
    // A malformed stored hash must read as "wrong password", not a 500.
    return false;
  }
}

function sha256(value: string): string {
  return createHash("sha256").update(value).digest("hex");
}

export interface AuthContext {
  db: DatabaseSync;
  sessionTtlMs: number;
  tokenTtlMs: number;
}

/* ── Users ─────────────────────────────────────────────────────────────── */

export async function createUser(
  ctx: AuthContext,
  email: string,
  password: string,
  role: Role,
): Promise<UserRow> {
  const normalized = email.trim().toLowerCase();
  const existing = ctx.db.prepare("SELECT id FROM users WHERE email = ?").get(normalized);
  if (existing) {
    throw new Error("user already exists");
  }

  const user: UserRow = {
    id: randomUUID(),
    email: normalized,
    password_hash: await hashPassword(password),
    role,
    created_at: Date.now(),
    disabled_at: null,
  };

  ctx.db
    .prepare(
      `INSERT INTO users (id, email, password_hash, role, created_at, disabled_at)
       VALUES ($id, $email, $password_hash, $role, $created_at, $disabled_at)`,
    )
    .run(namedParams(user));

  return user;
}

export function countUsers(db: DatabaseSync): number {
  const row = db.prepare("SELECT COUNT(*) AS n FROM users").get() as { n: number };
  return row.n;
}

/**
 * Verify credentials. A missing user still runs a dummy hash verification so
 * response timing does not reveal whether the address exists.
 */
export async function authenticate(
  ctx: AuthContext,
  email: string,
  password: string,
): Promise<UserRow | null> {
  const row = ctx.db
    .prepare("SELECT * FROM users WHERE email = ?")
    .get(email.trim().toLowerCase()) as UserRow | undefined;

  if (!row) {
    await verifyPassword(
      "$argon2id$v=19$m=19456,t=2,p=1$AAAAAAAAAAAAAAAA$AAAAAAAABBBBCCCCDDDDEEEEFFFF",
      password,
    );
    return null;
  }

  if (row.disabled_at !== null) {
    return null;
  }

  const ok = await verifyPassword(row.password_hash, password);
  return ok ? row : null;
}

/* ── Sessions (Web / browser) ───────────────────────────────────────────── */

export function createSession(
  ctx: AuthContext,
  userId: string,
  meta: { ip?: string; userAgent?: string } = {},
): SessionRow {
  const now = Date.now();
  const session: SessionRow = {
    id: randomBytes(32).toString("base64url"),
    user_id: userId,
    created_at: now,
    expires_at: now + ctx.sessionTtlMs,
    ip: meta.ip ?? null,
    user_agent: meta.userAgent ?? null,
  };

  ctx.db
    .prepare(
      `INSERT INTO sessions (id, user_id, created_at, expires_at, ip, user_agent)
       VALUES ($id, $user_id, $created_at, $expires_at, $ip, $user_agent)`,
    )
    .run(namedParams(session));

  return session;
}

export function resolveSession(
  ctx: AuthContext,
  sessionId: string,
): { session: SessionRow; user: UserRow } | null {
  const session = ctx.db.prepare("SELECT * FROM sessions WHERE id = ?").get(sessionId) as
    | SessionRow
    | undefined;

  if (!session || session.expires_at < Date.now()) {
    return null;
  }

  const user = ctx.db.prepare("SELECT * FROM users WHERE id = ?").get(session.user_id) as
    | UserRow
    | undefined;

  if (!user || user.disabled_at !== null) {
    return null;
  }

  return { session, user };
}

export function destroySession(ctx: AuthContext, sessionId: string): void {
  ctx.db.prepare("DELETE FROM sessions WHERE id = ?").run(sessionId);
}

export function destroyAllSessions(ctx: AuthContext, userId: string): void {
  ctx.db.prepare("DELETE FROM sessions WHERE user_id = ?").run(userId);
}

/* ── API tokens (CLI) ───────────────────────────────────────────────────── */

const TOKEN_PREFIX_LEN = 8;

/**
 * A token is `ps_<lookupPrefix>_<secret>`. Only a SHA-256 of the full token is
 * stored, so a database leak does not yield usable credentials. The lookup
 * prefix is stored in clear to keep token verification a single indexed read.
 */
export function createApiToken(
  ctx: AuthContext,
  userId: string,
  name: string,
): { token: string; row: ApiTokenRow } {
  const lookupPrefix = randomBytes(6).toString("hex");
  const secret = randomBytes(32).toString("base64url");
  const token = `ps_${lookupPrefix}_${secret}`;

  const row: ApiTokenRow = {
    id: randomUUID(),
    user_id: userId,
    name,
    prefix: `ps_${lookupPrefix}`,
    token_hash: sha256(token),
    created_at: Date.now(),
    expires_at: Date.now() + ctx.tokenTtlMs,
    last_used_at: null,
    revoked_at: null,
  };

  ctx.db
    .prepare(
      `INSERT INTO api_tokens (id, user_id, name, prefix, token_hash, created_at, expires_at, last_used_at, revoked_at)
       VALUES ($id, $user_id, $name, $prefix, $token_hash, $created_at, $expires_at, $last_used_at, $revoked_at)`,
    )
    .run(namedParams(row));

  return { token, row };
}

export function verifyApiToken(
  ctx: AuthContext,
  token: string,
): { token: ApiTokenRow; user: UserRow } | null {
  if (!token.startsWith("ps_") || token.length <= 3 + TOKEN_PREFIX_LEN + 1) {
    return null;
  }

  // The secret is base64url, whose alphabet includes `_`. Splitting the whole
  // token on `_` would therefore mis-split most real tokens; only the prefix
  // segment is delimited, everything after it is the secret.
  const rest = token.slice(3);
  const separator = rest.indexOf("_");
  if (separator <= 0) {
    return null;
  }

  const prefix = `ps_${rest.slice(0, separator)}`;
  const row = ctx.db.prepare("SELECT * FROM api_tokens WHERE prefix = ?").get(prefix) as
    | ApiTokenRow
    | undefined;

  if (!row || row.revoked_at !== null) {
    return null;
  }

  const expected = Buffer.from(row.token_hash, "hex");
  const actual = Buffer.from(sha256(token), "hex");
  if (expected.length !== actual.length || !timingSafeEqual(expected, actual)) {
    return null;
  }

  if (row.expires_at !== null && row.expires_at < Date.now()) {
    return null;
  }

  const user = ctx.db.prepare("SELECT * FROM users WHERE id = ?").get(row.user_id) as
    | UserRow
    | undefined;

  if (!user || user.disabled_at !== null) {
    return null;
  }

  ctx.db.prepare("UPDATE api_tokens SET last_used_at = ? WHERE id = ?").run(Date.now(), row.id);

  return { token: row, user };
}

export function listApiTokens(db: DatabaseSync, userId: string): ApiTokenRow[] {
  // node:sqlite types rows as `Record<string, SQLOutputValue>`; the row shapes
  // are declared alongside the schema in db/index.ts.
  return db
    .prepare("SELECT * FROM api_tokens WHERE user_id = ? ORDER BY created_at DESC")
    .all(userId) as unknown as ApiTokenRow[];
}

export function revokeApiToken(db: DatabaseSync, userId: string, tokenId: string): boolean {
  const result = db
    .prepare(
      "UPDATE api_tokens SET revoked_at = ? WHERE id = ? AND user_id = ? AND revoked_at IS NULL",
    )
    .run(Date.now(), tokenId, userId);
  return result.changes > 0;
}

/* ── Audit ─────────────────────────────────────────────────────────────── */

export interface AuditInput {
  actorType: "user" | "token" | "anonymous";
  actorId: string | null;
  action: string;
  target?: string | null;
  outcome: "allowed" | "denied";
  ip?: string | null;
  detail?: string | null;
}

export function recordAudit(db: DatabaseSync, entry: AuditInput): void {
  db.prepare(
    `INSERT INTO audit_log (ts, actor_type, actor_id, action, target, outcome, ip, detail)
     VALUES ($ts, $actor_type, $actor_id, $action, $target, $outcome, $ip, $detail)`,
  ).run(
    namedParams({
      ts: Date.now(),
      actor_type: entry.actorType,
      actor_id: entry.actorId,
      action: entry.action,
      target: entry.target ?? null,
      outcome: entry.outcome,
      ip: entry.ip ?? null,
      detail: entry.detail ?? null,
    }),
  );
}
