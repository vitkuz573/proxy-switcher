import { type NextRequest, NextResponse } from "next/server";
import { API_INTERNAL_URL } from "@/lib/env";

/**
 * Same-origin proxy to the API service.
 *
 * The browser never talks to the API directly. That removes the cross-origin
 * problem entirely: no CORS preflight, the session cookie stays same-origin
 * (so SameSite=Lax actually applies), and the API can keep binding loopback
 * without being reachable from a browser context.
 *
 * This is a routing pass-through, not a second API surface. Authorization,
 * validation and RBAC all happen in the API; the session cookie is forwarded
 * unchanged and never inspected here.
 */

const HOP_BY_HOP = new Set([
  "connection",
  "keep-alive",
  "proxy-authenticate",
  "proxy-authorization",
  "te",
  "trailer",
  "transfer-encoding",
  "upgrade",
  "host",
  "content-length",
]);

function forwardHeaders(request: NextRequest): HeadersInit {
  const headers: Record<string, string> = {};
  request.headers.forEach((value, key) => {
    if (!HOP_BY_HOP.has(key.toLowerCase())) {
      headers[key] = value;
    }
  });
  return headers;
}

function responseHeaders(upstream: Headers): Headers {
  const headers = new Headers();
  // Set-Cookie must be preserved verbatim or the session cookie is lost.
  upstream.forEach((value, key) => {
    if (!HOP_BY_HOP.has(key.toLowerCase())) {
      headers.append(key, value);
    }
  });
  // The proxy is same-origin, so caching API responses would be wrong.
  headers.set("cache-control", "no-store");
  return headers;
}

async function proxy(
  request: NextRequest,
  context: { params: Promise<{ path?: string[] }> },
  method: string,
): Promise<Response> {
  const { path } = await context.params;
  const suffix = (path ?? []).map(encodeURIComponent).join("/");
  const target = `${API_INTERNAL_URL}/api/${suffix}${request.nextUrl.search}`;

  const hasBody = method !== "GET" && method !== "HEAD";
  const body = hasBody ? await request.arrayBuffer() : undefined;

  try {
    const upstream = await fetch(target, {
      method,
      headers: forwardHeaders(request),
      ...(body ? { body } : {}),
      cache: "no-store",
      redirect: "manual",
    });

    return new NextResponse(
      upstream.status === 204 || upstream.status === 304 ? null : await upstream.arrayBuffer(),
      {
        status: upstream.status,
        headers: responseHeaders(upstream.headers),
      },
    );
  } catch (err) {
    // Never surface the API's address or a stack trace to the browser.
    console.error(`[proxy] ${method} ${target} failed:`, err);
    return NextResponse.json(
      { error: "api_unreachable", message: "The proxy API is not reachable" },
      { status: 502 },
    );
  }
}

export async function GET(request: NextRequest, context: { params: Promise<{ path?: string[] }> }) {
  return proxy(request, context, "GET");
}

export async function POST(
  request: NextRequest,
  context: { params: Promise<{ path?: string[] }> },
) {
  return proxy(request, context, "POST");
}

export async function PATCH(
  request: NextRequest,
  context: { params: Promise<{ path?: string[] }> },
) {
  return proxy(request, context, "PATCH");
}

export async function PUT(request: NextRequest, context: { params: Promise<{ path?: string[] }> }) {
  return proxy(request, context, "PUT");
}

export async function DELETE(
  request: NextRequest,
  context: { params: Promise<{ path?: string[] }> },
) {
  return proxy(request, context, "DELETE");
}
