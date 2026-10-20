/**
 * The local proxy, reached through the API's own port.
 *
 * A program is given one address — the API's — and uses it as a proxy. The
 * traffic is carried by the engine's local proxy, which knows the pool, the
 * health of every proxy in it, and the policy the client's credential
 * carries. This module is the passage: it moves the bytes and adds nothing
 * of its own.
 *
 * Two shapes of request arrive:
 *
 * - `CONNECT host:port` — a tunnel. Node hands over the socket and stops
 *   reading, so the bytes are moved as they are.
 * - an absolute URI in the request line — an ordinary HTTP request. The
 *   request is replayed to the engine with the URI intact, because that is
 *   what a forward proxy expects to read.
 *
 * The credential is not touched: the headers are passed on unchanged, so the
 * engine authenticates the client exactly as it would have done directly.
 */

import type { IncomingMessage } from "node:http";
import http from "node:http";
import { connect as connectSocket, type Socket } from "node:net";
import type { FastifyInstance, FastifyReply, FastifyRequest } from "fastify";

/** Where the engine's local proxy listens. */
export interface LocalProxyTarget {
  host: string;
  port: number;
}

/**
 * Whether the request is one a forward proxy would receive.
 *
 * A forward proxy is asked for an absolute URI: the target is in the request
 * line, not in the `Host` header.
 */
export function isProxyRequest(url: string): boolean {
  return url.startsWith("http://") || url.startsWith("https://");
}

/**
 * Serve the local proxy on the API's own port.
 *
 * The tunnels are handled on the socket, because a tunnel has no request
 * body and no response shape: the bytes go in both directions until one side
 * stops.
 */
export function serveLocalProxy(app: FastifyInstance, target: LocalProxyTarget): void {
  app.server.on("connect", (req, clientSocket, head) => {
    const upstream = connectSocket({ host: target.host, port: target.port });

    upstream.once("connect", () => {
      // Node has already read the request line, and the engine needs it: the
      // target is named there. The head is written again before anything
      // else, so the engine sees the request as the client sent it.
      upstream.write(requestLine(req));
      if (head.length > 0) upstream.write(head);

      clientSocket.pipe(upstream);
      upstream.pipe(clientSocket);
    });

    const fail = (reason: string) => {
      app.log.warn({ reason }, "the local proxy could not be reached");
      clientSocket.end(
        "HTTP/1.1 502 Bad Gateway\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nThe local proxy is not reachable.\n",
      );
    };

    upstream.once("error", fail);
    clientSocket.once("error", () => upstream.destroy());
    upstream.once("close", () => clientSocket.end());
  });
}

/**
 * The request as the client wrote it.
 *
 * Node hands over the socket after it has read the request line, so the line
 * is written again from what is left of the request: the method, the target
 * and the headers.
 */
function requestLine(req: IncomingMessage): string {
  const lines = [`${req.method ?? "CONNECT"} ${req.url ?? ""} HTTP/1.1`];

  for (const [name, value] of Object.entries(req.headers)) {
    if (value === undefined) continue;
    lines.push(`${name}: ${Array.isArray(value) ? value.join(", ") : value}`);
  }

  return `${lines.join("\r\n")}\r\n\r\n`;
}

/**
 * Carry an ordinary HTTP request through to the engine's local proxy.
 *
 * The request is replayed with its absolute URI, which is what a forward
 * proxy reads. The answer is streamed back as it arrives.
 */
export async function forwardProxyRequest(
  req: FastifyRequest,
  reply: FastifyReply,
  target: LocalProxyTarget,
): Promise<void> {
  // The promise settles when the answer has been sent. Returning earlier
  // would let the framework finish the request with an empty body while the
  // answer was still on its way.
  return new Promise<void>((resolve) => {
    const upstream = http.request({
      host: target.host,
      port: target.port,
      method: req.method,
      // The absolute URI is the point: it is what tells the proxy where the
      // request is going.
      path: req.url,
      headers: req.headers,
    });

    upstream.on("response", (response) => {
      reply.code(response.statusCode ?? 502);
      for (const [name, value] of Object.entries(response.headers)) {
        if (value !== undefined) reply.header(name, value);
      }
      reply.send(response);
      reply.raw.on("finish", resolve);
      reply.raw.on("close", resolve);
    });

    upstream.on("error", () => {
      if (!reply.sent) {
        reply
          .code(502)
          .type("text/plain; charset=utf-8")
          .send("The local proxy is not reachable.\n");
      }
      resolve();
    });

    req.raw.pipe(upstream);
  });
}

/** Close a socket without letting the error escape. */
export function closeQuietly(socket: Socket): void {
  socket.destroy();
}
