import { mkdirSync } from "node:fs";
import { dirname } from "node:path";
import { DatabaseSync, type SQLInputValue } from "node:sqlite";

/**
 * Uses Node's built-in SQLite rather than a native addon. A compiled binding
 * pins the deployment to one Node ABI — a version bump turns into a rebuild
 * that has to succeed before the service can start at all. The built-in module
 * has no such coupling.
 */

const SCHEMA = `
CREATE TABLE IF NOT EXISTS users (
  id           TEXT PRIMARY KEY,
  email        TEXT NOT NULL UNIQUE,
  password_hash TEXT NOT NULL,
  role         TEXT NOT NULL CHECK (role IN ('admin', 'operator', 'viewer')),
  created_at   INTEGER NOT NULL,
  disabled_at  INTEGER
);

CREATE TABLE IF NOT EXISTS sessions (
  id           TEXT PRIMARY KEY,
  user_id      TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  created_at   INTEGER NOT NULL,
  expires_at   INTEGER NOT NULL,
  ip           TEXT,
  user_agent   TEXT
);
CREATE INDEX IF NOT EXISTS idx_sessions_user ON sessions(user_id);
CREATE INDEX IF NOT EXISTS idx_sessions_expires ON sessions(expires_at);

CREATE TABLE IF NOT EXISTS api_tokens (
  id           TEXT PRIMARY KEY,
  user_id      TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  name         TEXT NOT NULL,
  prefix       TEXT NOT NULL UNIQUE,
  token_hash   TEXT NOT NULL,
  created_at   INTEGER NOT NULL,
  expires_at   INTEGER,
  last_used_at INTEGER,
  revoked_at   INTEGER
);
CREATE INDEX IF NOT EXISTS idx_tokens_prefix ON api_tokens(prefix);
CREATE INDEX IF NOT EXISTS idx_tokens_user ON api_tokens(user_id);

CREATE TABLE IF NOT EXISTS audit_log (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  ts          INTEGER NOT NULL,
  actor_type  TEXT NOT NULL CHECK (actor_type IN ('user', 'token', 'anonymous')),
  actor_id    TEXT,
  action      TEXT NOT NULL,
  target      TEXT,
  outcome     TEXT NOT NULL CHECK (outcome IN ('allowed', 'denied')),
  ip          TEXT,
  detail      TEXT
);
CREATE INDEX IF NOT EXISTS idx_audit_ts ON audit_log(ts DESC);
CREATE INDEX IF NOT EXISTS idx_audit_actor ON audit_log(actor_id, ts DESC);
`;

export type Database = DatabaseSync;

/**
 * `node:sqlite` types named parameters as a bare string-keyed record, which
 * domain rows do not satisfy because they have no index signature. Spreading
 * into a fresh object keeps every call site free of casts.
 */
export function namedParams<T extends object>(row: T): Record<string, SQLInputValue> {
  return { ...row } as unknown as Record<string, SQLInputValue>;
}

export type Role = "admin" | "operator" | "viewer";

export interface UserRow {
  id: string;
  email: string;
  password_hash: string;
  role: Role;
  created_at: number;
  disabled_at: number | null;
}

export interface SessionRow {
  id: string;
  user_id: string;
  created_at: number;
  expires_at: number;
  ip: string | null;
  user_agent: string | null;
}

export interface ApiTokenRow {
  id: string;
  user_id: string;
  name: string;
  prefix: string;
  token_hash: string;
  created_at: number;
  expires_at: number | null;
  last_used_at: number | null;
  revoked_at: number | null;
}

export function openDatabase(path: string): DatabaseSync {
  mkdirSync(dirname(path), { recursive: true });
  const db = new DatabaseSync(path);

  // WAL keeps the read-heavy API responsive while the audit log is written.
  db.exec("PRAGMA journal_mode = WAL");
  db.exec("PRAGMA foreign_keys = ON");
  db.exec("PRAGMA busy_timeout = 5000");

  db.exec(SCHEMA);
  return db;
}

/** Drop expired sessions and past-dated tokens. Called on boot and hourly. */
export function pruneExpired(
  db: DatabaseSync,
  now = Date.now(),
): { sessions: number; tokens: number } {
  const sessions = Number(db.prepare("DELETE FROM sessions WHERE expires_at < ?").run(now).changes);
  const tokens = Number(
    db.prepare("DELETE FROM api_tokens WHERE expires_at IS NOT NULL AND expires_at < ?").run(now)
      .changes,
  );
  return { sessions, tokens };
}
