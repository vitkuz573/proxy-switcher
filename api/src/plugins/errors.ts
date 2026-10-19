import { EngineError } from '../engine/client.js';

export interface ErrorBody {
  error: string;
  message: string;
  details?: unknown;
}

const STATUS_TO_CODE: Record<number, string> = {
  400: 'bad_request',
  401: 'unauthorized',
  403: 'forbidden',
  404: 'not_found',
  409: 'conflict',
  422: 'unprocessable',
  429: 'rate_limited',
  500: 'internal_error',
  502: 'engine_unavailable',
  504: 'engine_timeout',
};

/**
 * Single translation point for every failure leaving the API. Clients must never
 * receive a stack trace, a filesystem path, or the engine's internal URL.
 */
export function errorBody(status: number, message: string, details?: unknown): ErrorBody {
  const body: ErrorBody = { error: STATUS_TO_CODE[status] ?? 'error', message };
  if (details !== undefined) {
    body.details = details;
  }
  return body;
}

export function describeEngineError(err: EngineError): { status: number; body: ErrorBody } {
  if (err.status === 409) {
    return { status: 409, body: errorBody(409, 'The engine is already busy with that operation') };
  }
  if (err.status === 404) {
    return { status: 404, body: errorBody(404, 'Not found') };
  }
  if (err.status && err.status >= 500) {
    return { status: 502, body: errorBody(502, 'The proxy engine reported an internal error') };
  }
  if (err.status === null) {
    return { status: 502, body: errorBody(502, 'The proxy engine is unreachable') };
  }
  return { status: 400, body: errorBody(400, 'The engine rejected the request') };
}