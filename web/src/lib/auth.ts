import "server-only";
import { cookies } from "next/headers";
import { getLocale } from "next-intl/server";
import { redirect } from "@/i18n/navigation";
import { API_INTERNAL_URL, SESSION_COOKIE } from "./env";

export interface SessionUser {
  id: string;
  email: string;
  role: "admin" | "operator" | "viewer";
}

/**
 * Resolves the signed-in user or redirects to the login form.
 *
 * Reading the session cookie is a request read, so this may only be called
 * from inside a `<Suspense>` boundary — that is what keeps the rest of the page
 * inside the static shell. `redirect()` throws to interrupt rendering, which is
 * why a null user is handled here rather than by the caller.
 */
export async function getCurrentUser(): Promise<SessionUser> {
  const session = (await cookies()).get(SESSION_COOKIE)?.value;
  const locale = await getLocale();

  if (!session) {
    redirect({ href: "/login", locale });
  }

  const response = await fetch(`${API_INTERNAL_URL}/api/v1/sessions/current`, {
    headers: { cookie: `${SESSION_COOKIE}=${session}` },
    cache: "no-store",
  });

  if (response.status === 401 || response.status === 403) {
    // The cookie exists but is no longer valid; drop it on the way out.
    redirect({ href: "/login", locale });
  }

  if (!response.ok) {
    // The API being down is not the same as being signed out.
    throw new Error(`Auth service unavailable (${response.status})`);
  }

  const body = (await response.json()) as { user: SessionUser };
  return body.user;
}

/** True when a session cookie is present. Cheap; does not validate it. */
export async function hasSessionCookie(): Promise<boolean> {
  return (await cookies()).get(SESSION_COOKIE) !== undefined;
}
