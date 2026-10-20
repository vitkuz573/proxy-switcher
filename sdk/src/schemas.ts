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
  Type.Literal("shadowsocks"),
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
    /**
     * The country the traffic leaves from, as an ISO 3166-1 alpha-2 code.
     *
     * A code and not a name: the platform names it, in the reader's own
     * language, and the flag is derived from it.
     */
    country: Type.Union([Type.String(), Type.Null()]),
    /** The country the list said the proxy was in. */
    country_reported: Type.Union([Type.String(), Type.Null()]),
    last_checked: Type.Union([Type.String(), Type.Null()]),
    last_success: Type.Union([Type.String(), Type.Null()]),
    consecutive_failures: Type.Integer(),
    score: Type.Number(),
    /** The address traffic left from, as the target reported it. */
    exit_ip: Type.Union([Type.String(), Type.Null()]),
    /** Whether the proxy can carry a tunnel, or only plain HTTP. */
    supports_tunnel: Type.Boolean(),
    /** The cipher a Shadowsocks proxy wants. */
    cipher: Type.Optional(Type.String()),
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
    /** The connections open at this moment. */
    tcp_connections: Type.Integer(),
    /** The tunnels opened since the engine started. */
    tcp_connections_total: Type.Integer(),
    /** The bytes carried towards the destination. */
    bytes_sent: Type.Integer(),
    /** The bytes carried back to the client. */
    bytes_received: Type.Integer(),
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
    /** The destinations the policy is routing into the device. */
    destinations: Type.Integer(),
    /** The routes that are in the routing table. */
    routes: Type.Integer(),
  },
  { $id: "RoutingInfo" },
);
export type RoutingInfo = Static<typeof RoutingInfoSchema>;

/**
 * How the proxy underneath is chosen.
 *
 * - `time` — the proxy in front is replaced on a schedule.
 * - `per_request` — every request may be carried by a different proxy.
 * - `sticky` — a client keeps one proxy for as long as its session lasts.
 */
export const RotationSchema = Type.Union([
  Type.Literal("time"),
  Type.Literal("per_request"),
  Type.Literal("sticky"),
]);
export type Rotation = Static<typeof RotationSchema>;

/** What a credential's traffic is allowed to do. */
export const ProxyPolicySettingsSchema = Type.Object(
  {
    rotation: RotationSchema,
    /** How long a client keeps its proxy, in seconds. */
    sticky_secs: Type.Integer(),
    /** How often the proxy in front is replaced, in seconds. */
    rotate_interval_secs: Type.Integer(),
    /** The countries the exit address may be in, as ISO 3166-1 alpha-2 codes. */
    countries: Type.Array(Type.String()),
    /** The protocols that may be used. Empty means any. */
    protocols: Type.Array(ProxyProtocolSchema),
    /** The score a proxy has to reach to be used. */
    min_score: Type.Number(),
  },
  { $id: "ProxyPolicySettings" },
);
export type ProxyPolicySettings = Static<typeof ProxyPolicySettingsSchema>;

/** A named policy. */
export const ProxyPolicySchema = Type.Object(
  {
    name: Type.String(),
    settings: ProxyPolicySettingsSchema,
    /** Whether the policy came with the engine. */
    builtin: Type.Boolean(),
  },
  { $id: "ProxyPolicy" },
);
export type ProxyPolicy = Static<typeof ProxyPolicySchema>;

/** A credential for the local proxy. */
export const ProxyCredentialSchema = Type.Object(
  {
    username: Type.String(),
    /** The policy the credential carries. */
    policy: Type.String(),
    enabled: Type.Boolean(),
    created_at: Type.String(),
    /**
     * The password, only in the answer that creates the credential.
     *
     * The engine keeps a digest of it and cannot show it again.
     */
    password: Type.Optional(Type.String()),
  },
  { $id: "ProxyCredential" },
);
export type ProxyCredential = Static<typeof ProxyCredentialSchema>;

/** A credential whose session was turned over. */
export const CredentialRotationSchema = Type.Object(
  {
    /** The credential the session belonged to. */
    username: Type.String(),
    /** The policy the credential carries. */
    policy: Type.String(),
    /** Whether anything was actually held. */
    rotated: Type.Boolean(),
    /** How many sessions were turned over. */
    sessions: Type.Integer(),
    /** The proxy in front now. */
    current_proxy: Type.Union([Type.String(), Type.Null()]),
  },
  { $id: "CredentialRotation" },
);
export type CredentialRotation = Static<typeof CredentialRotationSchema>;

/** The local proxy a program can be pointed at. */
export const LocalProxySchema = Type.Object(
  {
    enabled: Type.Boolean(),
    /** The address a program should be given. */
    address: Type.String(),
    /** The proxy in front right now. */
    current_proxy: Type.Union([Type.String(), Type.Null()]),
    /** The clients that hold a proxy of their own. */
    sessions: Type.Integer(),
    connections: Type.Integer(),
    tunnels: Type.Integer(),
    failures: Type.Integer(),
    rotations: Type.Integer(),
    refused: Type.Integer(),
    bytes_out: Type.Integer(),
    bytes_in: Type.Integer(),
  },
  { $id: "LocalProxy" },
);
export type LocalProxy = Static<typeof LocalProxySchema>;

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
        from_ms: Type.Integer(),
        to_ms: Type.Union([Type.Integer(), Type.Null()]),
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
  ProxyPolicySettings: ProxyPolicySettingsSchema,
  ProxyPolicy: ProxyPolicySchema,
  ProxyCredential: ProxyCredentialSchema,
  CredentialRotation: CredentialRotationSchema,
  LocalProxy: LocalProxySchema,
  Problem: ProblemSchema,
} as const;

export type SchemaName = keyof typeof SCHEMAS;

/** Reference to a schema registered via `SCHEMAS`. */
export const Ref = Type.Ref;
