import { NextResponse, type NextRequest } from "next/server";

const SESSION_COOKIE = "ps_session";

/** Pages that require a session. */
const PROTECTED = ["/dashboard", "/proxies", "/scraper", "/audit"];

/**
 * Cheap first gate: bounce visitors without a session cookie before any
 * rendering happens, so an unauthenticated request gets a real redirect instead
 * of a static shell that redirects on the client.
 *
 * This is a routing shortcut, not a security boundary — the cookie could be
 * forged or stale. `lib/auth.ts` verifies the session against the API and is
 * the actual check.
 */
export function proxy(request: NextRequest): NextResponse {
  const { pathname } = request.nextUrl;
  const hasSession = request.cookies.has(SESSION_COOKIE);

  const isProtected = PROTECTED.some(
    (prefix) => pathname === prefix || pathname.startsWith(`${prefix}/`),
  );

  if (isProtected && !hasSession) {
    const url = request.nextUrl.clone();
    url.pathname = "/login";
    url.search = `?next=${encodeURIComponent(pathname)}`;
    return NextResponse.redirect(url);
  }

  // Signed-in visitors have no business on the login form.
  if (pathname === "/login" && hasSession) {
    const url = request.nextUrl.clone();
    url.pathname = "/dashboard";
    url.search = "";
    return NextResponse.redirect(url);
  }

  return NextResponse.next();
}

export const config = {
  matcher: [
    "/((?!_next/static|_next/image|favicon.ico|.*\\.(?:svg|png|jpg|jpeg|gif|webp)$).*)",
  ],
};