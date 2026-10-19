"use client";

import { createProxySwitcherClient } from "@proxy-switcher/sdk";

/**
 * Browser-side API access.
 *
 * Requests are same-origin: they go to `/api/...` on the Next.js server, which
 * proxies them to the API service. The browser never holds a bearer token and
 * never talks cross-origin, so there is no CORS surface at all.
 *
 * An empty base URL makes every contract path resolve relative to the current
 * origin, so the same client code works unchanged on the server, where
 * `lib/api.ts` points it at the internal API address instead.
 */
export const api = createProxySwitcherClient({ baseUrl: "" });

export { ApiError } from "@proxy-switcher/sdk";
