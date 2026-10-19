import { buildApp } from "./app.js";
import { loadConfig } from "./config.js";
import { openDatabase, pruneExpired } from "./db/index.js";
import { countUsers, createUser } from "./services/auth.js";

async function main(): Promise<void> {
  const config = loadConfig();
  const db = openDatabase(config.DATABASE_PATH);

  const pruned = pruneExpired(db);
  if (pruned.sessions + pruned.tokens > 0) {
    console.log(
      `pruned ${pruned.sessions} expired session(s) and ${pruned.tokens} expired token(s)`,
    );
  }

  // First-run administrator, so the service is usable without a SQL console.
  if (countUsers(db) === 0 && config.BOOTSTRAP_ADMIN_EMAIL && config.BOOTSTRAP_ADMIN_PASSWORD) {
    const admin = await createUser(
      { db, sessionTtlMs: 0, tokenTtlMs: 0 },
      config.BOOTSTRAP_ADMIN_EMAIL,
      config.BOOTSTRAP_ADMIN_PASSWORD,
      "admin",
    );
    console.log(`bootstrap administrator created: ${admin.email}`);
  } else if (countUsers(db) === 0) {
    console.warn(
      "no users exist. Set BOOTSTRAP_ADMIN_EMAIL and BOOTSTRAP_ADMIN_PASSWORD, " +
        "or POST /api/v1/auth/bootstrap while the service is running.",
    );
  }

  const app = await buildApp({ config, db });
  await app.listen({ host: config.API_HOST, port: config.API_PORT });

  app.log.info(`docs available at http://${config.API_HOST}:${config.API_PORT}/docs`);

  // Housekeeping: sessions and tokens do not clean themselves up.
  const pruneTimer = setInterval(() => {
    const result = pruneExpired(db);
    if (result.sessions + result.tokens > 0) {
      app.log.info({ ...result }, "pruned expired sessions and tokens");
    }
  }, 3_600_000);
  pruneTimer.unref();

  const shutdown = async (signal: string): Promise<void> => {
    app.log.info(`${signal} received, shutting down`);
    try {
      await app.close();
      db.close();
      process.exit(0);
    } catch (err) {
      app.log.error({ err }, "error during shutdown");
      process.exit(1);
    }
  };

  process.on("SIGINT", () => void shutdown("SIGINT"));
  process.on("SIGTERM", () => void shutdown("SIGTERM"));
}

main().catch((err) => {
  console.error(err instanceof Error ? err.message : err);
  process.exit(1);
});
