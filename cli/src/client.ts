import { ApiError, createProxySwitcherClient, type ProxySwitcherClient } from "@proxy-switcher/sdk";
import { loadConfig } from "./config.js";

export class CliError extends Error {
  constructor(
    message: string,
    readonly hint?: string,
  ) {
    super(message);
    this.name = "CliError";
  }
}

/** Unauthenticated client, used only for the login exchange. */
export function anonymousClient(baseUrl?: string): ProxySwitcherClient {
  return createProxySwitcherClient({ baseUrl: baseUrl ?? loadConfig().baseUrl });
}

export function client(): ProxySwitcherClient {
  const config = loadConfig();

  if (!config.token) {
    throw new CliError("Not signed in.", "Run 'ps auth login' first.");
  }

  return createProxySwitcherClient({ baseUrl: config.baseUrl, token: config.token });
}

/**
 * Turn an API failure into a readable CLI error.
 *
 * The SDK throws `ApiError` carrying the RFC 9457 problem document, so the
 * server's own wording survives all the way to the terminal. Status codes that
 * always mean the same thing to a user get an actionable hint instead.
 */
export function cliCall<T>(operation: () => Promise<T>): Promise<T> {
  return operation().catch((error: unknown) => {
    if (!(error instanceof ApiError)) {
      throw error;
    }

    const message = error.problem.detail || error.problem.title;

    if (error.status === 401) {
      throw new CliError(message, "Your token is invalid or expired. Run 'ps auth login'.");
    }
    if (error.status === 403) {
      throw new CliError(message, "Your role does not allow this action.");
    }
    if (error.status === 0) {
      throw new CliError(message, "Is the API service running?");
    }
    throw new CliError(message);
  });
}
