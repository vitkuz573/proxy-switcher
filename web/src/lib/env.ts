import "server-only";

/**
 * Server-only network configuration.
 *
 * The browser never contacts the API directly: requests go through the
 * same-origin proxy at `/api/[...path]`. That removes cross-origin requests
 * entirely, keeps the session cookie same-origin so `SameSite=Lax` actually
 * applies, and lets the API keep binding loopback.
 *
 * A missing value falls back to loopback rather than throwing: this module is
 * evaluated during `next build`, and loopback is the safe default — nothing is
 * reachable off-host until someone sets the variable deliberately.
 */
function resolveUrl(name: string, fallback: string): string {
  const value = process.env[name];
  if (value) {
    return value;
  }
  if (process.env.NODE_ENV === "production") {
    console.warn(
      `[env] ${name} is unset, falling back to ${fallback}. ` +
        "Set it before serving this app outside the host.",
    );
  }
  return fallback;
}

export const API_INTERNAL_URL = resolveUrl("API_INTERNAL_URL", "http://127.0.0.1:8088");

/** Name of the API's session cookie. Kept in sync with `api/src/plugins/auth.ts`. */
export const SESSION_COOKIE = "ps_session";
