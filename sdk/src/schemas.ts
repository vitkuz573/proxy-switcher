import { type Static, Type } from "@sinclair/typebox";

/**
 * The wire contract, defined once.
 *
 * The API validates against these schemas and the clients derive their types
 * from them, so the two can never drift. Generating the SDK from the OpenAPI
 * document instead would make the contract a build artefact that can lag the
 * server, and would tie the toolchain to TypeScript's JavaScript compiler API —
 * which TypeScript 7, the native port, no longer exposes.
 */

export const ProxyProtocolSchema = Type.Union([
  Type.Literal("http"),
  Type.Literal("https"),
  Type.Literal("socks4"),
  Type.Literal("socks5"),
  Type.Literal("trojan"),
]);
export type ProxyProtocol = Static<typeof ProxyProtocolSchema>;

export const AnonymitySchema = Type.Union([
  Type.Literal("transparent"),
  Type.Literal("anonymous"),
  Type.Literal("elite"),
  Type.Literal("unknown"),
]);
export type Anonymity = Static<typeof AnonymitySchema>;

export const RoleSchema = Type.Union([
  Type.Literal("admin"),
  Type.Literal("operator"),
  Type.Literal("viewer"),
]);
export type Role = Static<typeof RoleSchema>;

export const ProxyRecordSchema = Type.Object(
  {
    id: Type.String(),
    host: Type.String(),
    port: Type.Integer(),
    protocol: ProxyProtocolSchema,
    anonymity: AnonymitySchema,
    latency_ms: Type.Union([Type.Integer(), Type.Null()]),
    country: Type.Union([Type.String(), Type.Null()]),
    last_checked: Type.Union([Type.String(), Type.Null()]),
    last_success: Type.Union([Type.String(), Type.Null()]),
    consecutive_failures: Type.Integer(),
    score: Type.Number(),
    /** The address traffic left from, as the target reported it. */
    exit_ip: Type.Union([Type.String(), Type.Null()]),
    /** Whether the proxy can carry a tunnel, or only plain HTTP. */
    supports_tunnel: Type.Boolean(),
  },
  { $id: "Proxy" },
);
export type ProxyRecord = Static<typeof ProxyRecordSchema>;

export const SummarySchema = Type.Object(
  {
    pool_size: Type.Integer(),
    healthy_count: Type.Integer(),
    active_proxy: Type.Union([ProxyRecordSchema, Type.Null()]),
  },
  { $id: "Summary" },
);
export type Summary = Static<typeof SummarySchema>;

export const StatsSchema = Type.Object(
  {
    tcp_connections: Type.Integer(),
    udp_flows: Type.Integer(),
    dns_cache_size: Type.Integer(),
    healthy_count: Type.Integer(),
    pool_size: Type.Integer(),
  },
  { $id: "Stats" },
);
export type Stats = Static<typeof StatsSchema>;

export const HealthCheckProgressSchema = Type.Object(
  { checked: Type.Integer(), total: Type.Integer() },
  { $id: "HealthCheckProgress" },
);
export type HealthCheckProgress = Static<typeof HealthCheckProgressSchema>;

export const ScrapeRunSchema = Type.Object(
  {
    id: Type.String(),
    state: Type.Union([Type.Literal("idle"), Type.Literal("running"), Type.Literal("checking")]),
    started_at: Type.Union([Type.String(), Type.Null()]),
    finished_at: Type.Union([Type.String(), Type.Null()]),
    proxies_found: Type.Integer(),
    healthy_count: Type.Integer(),
    /** How many proxies are due for a probe right now. */
    pending: Type.Integer(),
    /** When the next proxy comes due. */
    next_due_at: Type.Union([Type.String(), Type.Null()]),
    health_check: Type.Union([HealthCheckProgressSchema, Type.Null()]),
    errors: Type.Array(Type.String()),
  },
  { $id: "ScrapeRun" },
);
export type ScrapeRun = Static<typeof ScrapeRunSchema>;

export const SourceSchema = Type.Object(
  { id: Type.String(), url: Type.String() },
  { $id: "Source" },
);
export type Source = Static<typeof SourceSchema>;

export const UserSchema = Type.Object(
  {
    id: Type.String({ format: "uuid" }),
    email: Type.String({ format: "email" }),
    role: RoleSchema,
    created_at: Type.String({ format: "date-time" }),
  },
  { $id: "User" },
);
export type User = Static<typeof UserSchema>;

export const ApiTokenSchema = Type.Object(
  {
    id: Type.String({ format: "uuid" }),
    name: Type.String(),
    prefix: Type.String(),
    created_at: Type.String({ format: "date-time" }),
    expires_at: Type.Union([Type.String({ format: "date-time" }), Type.Null()]),
    last_used_at: Type.Union([Type.String({ format: "date-time" }), Type.Null()]),
  },
  { $id: "ApiToken" },
);
export type ApiToken = Static<typeof ApiTokenSchema>;

/** Returned exactly once, at creation time. Never retrievable again. */
export const CreatedApiTokenSchema = Type.Object(
  { token: Type.String(), api_token: ApiTokenSchema },
  { $id: "CreatedApiToken" },
);
export type CreatedApiToken = Static<typeof CreatedApiTokenSchema>;

export const TokenExchangeSchema = Type.Object(
  { token: Type.String(), api_token: ApiTokenSchema, user: UserSchema },
  { $id: "TokenExchange" },
);
export type TokenExchange = Static<typeof TokenExchangeSchema>;

export const AuditEventSchema = Type.Object(
  {
    id: Type.Integer(),
    ts: Type.String({ format: "date-time" }),
    actor_type: Type.String(),
    actor_id: Type.Union([Type.String(), Type.Null()]),
    action: Type.String(),
    target: Type.Union([Type.String(), Type.Null()]),
    outcome: Type.String(),
    ip: Type.Union([Type.String(), Type.Null()]),
    detail: Type.Union([Type.String(), Type.Null()]),
  },
  { $id: "AuditEvent" },
);
export type AuditEvent = Static<typeof AuditEventSchema>;

/**
 * Which traffic a proxy applies to.
 *
 * The default routes nothing: a proxy is a tool for specific destinations,
 * not a default route for the whole host.
 */
export const RoutingPolicySchema = Type.Object(
  {
    mode: Type.Union([Type.Literal("none"), Type.Literal("allowlist"), Type.Literal("denylist")]),
    cidrs: Type.Array(Type.String()),
    domains: Type.Array(Type.String()),
    ports: Type.Array(Type.Integer()),
    always_direct: Type.Array(Type.String()),
  },
  { $id: "RoutingPolicy" },
);
export type RoutingPolicy = Static<typeof RoutingPolicySchema>;

export const RoutingInfoSchema = Type.Object(
  {
    policy: RoutingPolicySchema,
    enabled: Type.Boolean(),
    bypass_mark: Type.Integer(),
    default_route: Type.Boolean(),
    active_proxy: Type.Union([Type.String(), Type.Null()]),
  },
  { $id: "RoutingInfo" },
);
export type RoutingInfo = Static<typeof RoutingInfoSchema>;

/** How many proxies there are, and how many of them work. */
export const PoolStatisticsSchema = Type.Object(
  {
    pool_size: Type.Integer(),
    working: Type.Integer(),
    tunnel_capable: Type.Integer(),
    unverified: Type.Integer(),
    below_threshold: Type.Integer(),
    due: Type.Integer(),
    by_protocol: Type.Array(
      Type.Object({
        protocol: ProxyProtocolSchema,
        count: Type.Integer(),
        working: Type.Integer(),
      }),
    ),
    by_anonymity: Type.Array(
      Type.Object({
        anonymity: AnonymitySchema,
        count: Type.Integer(),
        working: Type.Integer(),
      }),
    ),
    by_country: Type.Array(
      Type.Object({
        country: Type.String(),
        count: Type.Integer(),
        working: Type.Integer(),
      }),
    ),
    latency: Type.Array(
      Type.Object({
        range: Type.String(),
        count: Type.Integer(),
      }),
    ),
    distinct_exit_addresses: Type.Integer(),
    history: Type.Array(
      Type.Object({
        at: Type.String({ format: "date-time" }),
        pool_size: Type.Integer(),
        working: Type.Integer(),
        due: Type.Integer(),
      }),
    ),
  },
  { $id: "PoolStatistics" },
);
export type PoolStatistics = Static<typeof PoolStatisticsSchema>;

/** RFC 9457 problem document. The single error shape for the whole API. */
export const ProblemSchema = Type.Object(
  {
    type: Type.String({ format: "uri" }),
    title: Type.String(),
    status: Type.Integer(),
    detail: Type.String(),
    instance: Type.Optional(Type.String()),
  },
  { $id: "Problem" },
);
export type Problem = Static<typeof ProblemSchema>;

/**
 * Every reusable schema, keyed by its `$id`.
 *
 * `HealthCheckProgress` is deliberately absent: it carries its own `$id` and is
 * compiled as part of `ScrapeRun`, so registering it again is a duplicate.
 */
export const SCHEMAS = {
  Proxy: ProxyRecordSchema,
  Summary: SummarySchema,
  Stats: StatsSchema,
  ScrapeRun: ScrapeRunSchema,
  Source: SourceSchema,
  User: UserSchema,
  ApiToken: ApiTokenSchema,
  CreatedApiToken: CreatedApiTokenSchema,
  TokenExchange: TokenExchangeSchema,
  AuditEvent: AuditEventSchema,
  PoolStatistics: PoolStatisticsSchema,
  RoutingPolicy: RoutingPolicySchema,
  RoutingInfo: RoutingInfoSchema,
  Problem: ProblemSchema,
} as const;

export type SchemaName = keyof typeof SCHEMAS;

/** Reference to a schema registered via `SCHEMAS`. */
export const Ref = Type.Ref;
