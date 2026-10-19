import type { EngineError } from "../engine/client.js";
import type { Problem } from "../schemas/index.js";

const TITLES: Record<number, string> = {
  400: "Bad request",
  401: "Unauthorized",
  403: "Forbidden",
  404: "Not found",
  405: "Method not allowed",
  409: "Conflict",
  415: "Unsupported media type",
  422: "Unprocessable entity",
  429: "Too many requests",
  500: "Internal server error",
  502: "Engine unavailable",
  503: "Service unavailable",
  504: "Engine timeout",
};

/**
 * RFC 9457 problem document: the single error shape for the whole API.
 * Clients get one shape to handle, and nothing internal leaks through `detail`.
 */
export const PROBLEM_CONTENT_TYPE = "application/problem+json";

export function problem(
  status: number,
  detail: string,
  options: { type?: string; instance?: string } = {},
): Problem {
  return {
    type: options.type ?? `https://proxy-switcher.dev/problems/${status}`,
    title: TITLES[status] ?? "Error",
    status,
    detail,
    ...(options.instance ? { instance: options.instance } : {}),
  };
}

/** Map an engine failure onto a public problem document. */
export function describeEngineError(
  err: EngineError,
  instance: string,
): { status: number; body: Problem } {
  if (err.status === null) {
    return { status: 502, body: problem(502, "The proxy engine is unreachable", { instance }) };
  }

  if (err.status >= 500) {
    return {
      status: 502,
      body: problem(502, "The proxy engine reported an internal error", { instance }),
    };
  }

  // The engine speaks the same RFC 9457 dialect, and its explanations are about
  // proxies and sources rather than infrastructure. Relaying them verbatim tells
  // the caller which resource was actually missing; a generic message does not.
  const detail = err.problem?.detail;

  if (err.status === 400 || err.status === 422) {
    return {
      status: 422,
      body: problem(422, detail ?? "The engine rejected the request", { instance }),
    };
  }

  if (err.status === 404) {
    return {
      status: 404,
      body: problem(404, detail ?? "The requested resource does not exist", { instance }),
    };
  }

  if (err.status === 409) {
    return {
      status: 409,
      body: problem(409, detail ?? "The engine is busy with that operation", { instance }),
    };
  }

  return {
    status: 502,
    body: problem(502, "The proxy engine returned an unexpected response", { instance }),
  };
}
