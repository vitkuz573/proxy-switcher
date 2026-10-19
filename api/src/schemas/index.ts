import { Type } from '@sinclair/typebox';
import type { Static } from '@sinclair/typebox';

export const ProxyProtocolSchema = Type.Union([
  Type.Literal('http'),
  Type.Literal('https'),
  Type.Literal('socks4'),
  Type.Literal('socks5'),
]);
export type ProxyProtocol = Static<typeof ProxyProtocolSchema>;

export const AnonymitySchema = Type.Union([
  Type.Literal('transparent'),
  Type.Literal('anonymous'),
  Type.Literal('elite'),
  Type.Literal('unknown'),
]);
export type Anonymity = Static<typeof AnonymitySchema>;

export const ProxySchema = Type.Object(
  {
    id: Type.String(),
    host: Type.String(),
    port: Type.Integer(),
    protocol: ProxyProtocolSchema,
    anonymity: AnonymitySchema,
    latency_ms: Type.Union([Type.Integer(), Type.Null()]),
    country: Type.Union([Type.String(), Type.Null()]),
    last_checked: Type.Union([Type.String(), Type.Null()]),
    score: Type.Number(),
  },
  { $id: 'Proxy' },
);
export type Proxy = Static<typeof ProxySchema>;

export const StatusSchema = Type.Object(
  {
    active_proxy: Type.Union([ProxySchema, Type.Null()]),
    pool_size: Type.Integer(),
  },
  { $id: 'Status' },
);
export type Status = Static<typeof StatusSchema>;

export const StatsSchema = Type.Object(
  {
    tcp_connections: Type.Integer(),
    udp_flows: Type.Integer(),
    dns_cache_size: Type.Integer(),
    healthy_count: Type.Integer(),
  },
  { $id: 'Stats' },
);
export type Stats = Static<typeof StatsSchema>;

export const ScrapeStatusSchema = Type.Object(
  {
    running: Type.Boolean(),
    last_run: Type.Union([Type.String(), Type.Null()]),
    proxies_found: Type.Integer(),
    healthy_count: Type.Integer(),
    /** The engine reports progress as a positional `[checked, total]` pair. */
    checking_progress: Type.Union([
      Type.Tuple([Type.Integer(), Type.Integer()]),
      Type.Null(),
    ]),
    errors: Type.Array(Type.String()),
  },
  { $id: 'ScrapeStatus' },
);
export type ScrapeStatus = Static<typeof ScrapeStatusSchema>;

export const SourceSchema = Type.Object(
  {
    url: Type.String(),
  },
  { $id: 'Source' },
);
export type Source = Static<typeof SourceSchema>;

export const UserSchema = Type.Object(
  {
    id: Type.String({ format: 'uuid' }),
    email: Type.String({ format: 'email' }),
    role: Type.Union([Type.Literal('admin'), Type.Literal('operator'), Type.Literal('viewer')]),
    created_at: Type.String(),
  },
  { $id: 'User' },
);
export type PublicUser = Static<typeof UserSchema>;

/** Alias used by routes; the wire shape is identical. */
export const PublicUserSchema = UserSchema;

export const ApiTokenSchema = Type.Object(
  {
    id: Type.String({ format: 'uuid' }),
    name: Type.String(),
    prefix: Type.String(),
    created_at: Type.String(),
    expires_at: Type.Union([Type.String(), Type.Null()]),
    last_used_at: Type.Union([Type.String(), Type.Null()]),
  },
  { $id: 'ApiToken' },
);
export type PublicApiToken = Static<typeof ApiTokenSchema>;

/** Returned exactly once, at creation time. Never retrievable again. */
export const CreatedApiTokenSchema = Type.Object(
  {
    token: Type.String(),
    api_token: ApiTokenSchema,
  },
  { $id: 'CreatedApiToken' },
);
export type CreatedApiToken = Static<typeof CreatedApiTokenSchema>;

export const AuditEntrySchema = Type.Object(
  {
    id: Type.Integer(),
    ts: Type.String(),
    actor_type: Type.String(),
    actor_id: Type.Union([Type.String(), Type.Null()]),
    action: Type.String(),
    target: Type.Union([Type.String(), Type.Null()]),
    outcome: Type.String(),
    ip: Type.Union([Type.String(), Type.Null()]),
    detail: Type.Union([Type.String(), Type.Null()]),
  },
  { $id: 'AuditEntry' },
);
export type AuditEntry = Static<typeof AuditEntrySchema>;

export const ErrorSchema = Type.Object(
  {
    error: Type.String(),
    message: Type.String(),
    details: Type.Optional(Type.Unknown()),
  },
  { $id: 'Error' },
);
export type ApiError = Static<typeof ErrorSchema>;

/**
 * Every reusable schema, keyed by its `$id`.
 *
 * Registered with `app.addSchema()` so Fastify can resolve `Type.Ref` for both
 * request validation and response serialisation, and so the OpenAPI document
 * emits them as named components instead of inlining a copy at every use.
 */
export const SCHEMAS = {
  Proxy: ProxySchema,
  Status: StatusSchema,
  Stats: StatsSchema,
  ScrapeStatus: ScrapeStatusSchema,
  Source: SourceSchema,
  User: UserSchema,
  ApiToken: ApiTokenSchema,
  CreatedApiToken: CreatedApiTokenSchema,
  AuditEntry: AuditEntrySchema,
  Error: ErrorSchema,
} as const;

export type SchemaName = keyof typeof SCHEMAS;

/** Convenience alias for referencing a registered schema in a route. */
export const Ref = Type.Ref;