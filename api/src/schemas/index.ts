/**
 * The contract lives in `@proxy-switcher/sdk` so the server and its clients
 * are compiled against one definition. This module re-exports it under the
 * paths the API uses internally.
 */

export type {
  Anonymity,
  ApiToken,
  AuditEvent,
  CreatedApiToken,
  PoolStatistics,
  Problem,
  ProxyProtocol,
  ProxyRecord,
  Role,
  RoutingInfo,
  RoutingPolicy,
  SchemaName,
  ScrapeRun,
  Source,
  Stats,
  Summary,
  TokenExchange,
  User,
} from "@proxy-switcher/sdk/schemas";
export {
  AnonymitySchema,
  ApiTokenSchema,
  AuditEventSchema,
  CreatedApiTokenSchema,
  HealthCheckProgressSchema,
  PoolStatisticsSchema,
  ProblemSchema,
  ProxyProtocolSchema,
  ProxyRecordSchema,
  Ref,
  RoleSchema,
  RoutingInfoSchema,
  RoutingPolicySchema,
  SCHEMAS,
  ScrapeRunSchema,
  SourceSchema,
  StatsSchema,
  SummarySchema,
  TokenExchangeSchema,
  UserSchema,
} from "@proxy-switcher/sdk/schemas";
