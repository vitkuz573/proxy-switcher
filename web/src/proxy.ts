import { type NextRequest, NextResponse } from "next/server";
import createIntlProxy from "next-intl/middleware";

import { routing } from "@/i18n/routing";

const SESSION_COOKIE = "ps_session";

/** Pages that require a session. */
const PROTECTED = ["/dashboard", "/proxies", "/scraper", "/statistics", "/audit"];

const intlProxy = createIntlProxy(routing);

/**
 * The locale, then the session.
 *
 * The locale comes first because it rewrites the path: without the rewrite
 * the session check would see `/ru/dashboard` and not recognise it as a
 * protected page.
 *
 * The session check is a routing shortcut, not a security boundary — the
 * cookie could be forged or stale. `lib/auth.ts` verifies the session against
 * the API and is the actual check.
 */
export function proxy(request: NextRequest): NextResponse {
  const response = intlProxy(request);
  const { pathname } = request.nextUrl;
  const hasSession = request.cookies.has(SESSION_COOKIE);

  // The locale prefix is part of the address, so it is stripped before the
  // path is matched against the protected pages.
  const locale = routing.locales.find(
    (candidate) => pathname === `/${candidate}` || pathname.startsWith(`/${candidate}/`),
  );
  const withoutLocale = locale ? pathname.slice(locale.length + 1) || "/" : pathname;

  const isProtected = PROTECTED.some(
    (prefix) => withoutLocale === prefix || withoutLocale.startsWith(`${prefix}/`),
  );

  if (isProtected && !hasSession) {
    const url = request.nextUrl.clone();
    url.pathname = `/${locale ?? routing.defaultLocale}/login`;
    url.search = `?next=${encodeURIComponent(pathname)}`;
    return NextResponse.redirect(url);
  }

  // Signed-in visitors have no business on the login form.
  if (withoutLocale === "/login" && hasSession) {
    const url = request.nextUrl.clone();
    url.pathname = `/${locale ?? routing.defaultLocale}/dashboard`;
    url.search = "";
    return NextResponse.redirect(url);
  }

  return response;
}

export const config = {
  // Everything except the API, the internals, and files with an extension.
  matcher: "/((?!api|_next|_vercel|.*\\..*).*)",
};
