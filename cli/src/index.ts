#!/usr/bin/env node
import { createInterface } from "node:readline/promises";
import { Command } from "commander";
import { anonymousClient, CliError, cliCall, client } from "./client.js";
import { configPath, loadConfig, saveConfig } from "./config.js";
import { dim, green, heading, keyValue, red, table, truncate } from "./format.js";
import { runCli } from "./run.js";

const program = new Command();

program
  .name("ps")
  .description("Control the proxy-switcher engine")
  .version("2.0.0")
  .option("-u, --url <url>", "API base URL");

program
  .command("auth")
  .description("Credential management")
  .addCommand(
    new Command("login")
      .description("Exchange email and password for an API token")
      .option("-e, --email <email>", "account email")
      .option("-p, --password <password>", "account password")
      .option("--url <url>", "API base URL")
      .action(async (options: { email?: string; password?: string; url?: string }) => {
        await runCli(async () => {
          const email = options.email ?? (await prompt("Email: "));
          const password = options.password ?? (await prompt("Password: ", true));

          const granted = await cliCall(() =>
            anonymousClient(options.url).exchangeToken({ email, password }),
          );

          saveConfig({
            baseUrl: options.url ?? loadConfig().baseUrl,
            token: granted.token,
          });

          console.log(
            `\n  ${green("✓")} Signed in as ${granted.user.email} (${granted.user.role})`,
          );
          console.log(`  ${dim(`token ${granted.api_token.prefix} stored in ${configPath()}`)}`);
        });
      }),
  )
  .addCommand(
    new Command("status").description("Show which token is stored").action(async () => {
      await runCli(async () => {
        const config = loadConfig();
        keyValue("API URL", config.baseUrl);
        keyValue("Token", config.token ? green("stored") : red("missing"));
        keyValue("Config", configPath());
      });
    }),
  )
  .addCommand(
    new Command("logout").description("Forget the stored token").action(async () => {
      await runCli(async () => {
        saveConfig({ baseUrl: loadConfig().baseUrl });
        console.log(`  ${green("✓")} Token removed`);
      });
    }),
  );

program
  .command("status")
  .description("Active proxy and pool size")
  .action(async () => {
    await runCli(async () => {
      const api = client();
      const [summary, stats] = await Promise.all([
        cliCall(() => api.getSummary()),
        cliCall(() => api.getStats()),
      ]);

      heading("Engine");
      keyValue("Pool size", summary.pool_size);
      keyValue("Healthy", summary.healthy_count);
      keyValue("TCP connections", stats.tcp_connections);
      keyValue("UDP flows", stats.udp_flows);
      keyValue("DNS cache entries", stats.dns_cache_size);

      heading("Active proxy");
      if (!summary.active_proxy) {
        console.log(`  ${dim("none selected")}`);
        return;
      }
      const p = summary.active_proxy;
      keyValue("Endpoint", `${p.host}:${p.port}`);
      keyValue("Protocol", p.protocol);
      keyValue("Anonymity", p.anonymity);
      keyValue("Latency", p.latency_ms === null ? null : `${p.latency_ms} ms`);
      keyValue("Score", p.score.toFixed(1));
    });
  });

const proxies = program.command("proxies").description("Manage the proxy pool");

proxies
  .command("list", { isDefault: true })
  .description("List proxies")
  .option("-s, --search <text>", "filter by host or country")
  .option("-n, --limit <count>", "maximum rows", "25")
  .option("--sort <key>", "score | latency | host", "score")
  .action(async (options: { search?: string; limit: string; sort: string }) => {
    await runCli(async () => {
      const api = client();
      const limit = Number(options.limit);
      const rows = await cliCall(() =>
        api.listProxies({
          ...(options.search ? { search: options.search } : {}),
          sort: options.sort as "score" | "latency" | "host",
          limit: Number.isFinite(limit) ? limit : 25,
        }),
      );
      const summary = await cliCall(() => api.getSummary());

      heading(`Proxies (${rows.length})`);
      table(
        ["", "ENDPOINT", "PROTO", "LATENCY", "SCORE"],
        rows.map((p) => [
          p.id === summary.active_proxy?.id ? "*" : " ",
          truncate(p.id, 26),
          truncate(p.protocol, 7),
          p.latency_ms === null ? "—" : `${p.latency_ms} ms`,
          p.score.toFixed(1),
        ]),
      );
    });
  });

proxies
  .command("add")
  .description("Add a proxy manually")
  .argument("<host>", "hostname or IP")
  .argument("<port>", "port number")
  .argument("[protocol]", "http | https | socks4 | socks5", "http")
  .option("--country <code>", "country label")
  .action(
    async (
      host: string,
      port: string,
      protocol: "http" | "https" | "socks4" | "socks5",
      options: { country?: string },
    ) => {
      await runCli(async () => {
        const portNumber = Number(port);
        if (!Number.isInteger(portNumber) || portNumber < 1 || portNumber > 65535) {
          throw new CliError("Port must be an integer between 1 and 65535.");
        }

        const proxy = await cliCall(() =>
          client().createProxy({
            host,
            port: portNumber,
            protocol,
            ...(options.country ? { country: options.country } : {}),
          }),
        );
        console.log(`  ${green("✓")} Added ${proxy.id} to the pool`);
      });
    },
  );

proxies
  .command("remove")
  .description("Remove a proxy from the pool")
  .argument("<id>", "proxy id, i.e. host:port")
  .action(async (id: string) => {
    await runCli(async () => {
      await cliCall(() => client().deleteProxy(id));
      console.log(`  ${green("✓")} Removed ${id}`);
    });
  });

proxies
  .command("switch")
  .description("Route traffic through a proxy")
  .argument("<id>", "proxy id, i.e. host:port")
  .action(async (id: string) => {
    await runCli(async () => {
      const proxy = await cliCall(() => client().setActiveProxy(id));
      console.log(`  ${green("✓")} Active proxy: ${proxy.id}`);
    });
  });

program
  .command("rotate")
  .description("Move to the next proxy in the pool")
  .action(async () => {
    await runCli(async () => {
      const proxy = await cliCall(() => client().rotateActiveProxy());
      console.log(`  ${green("✓")} Active proxy: ${proxy.id}`);
    });
  });

const sources = program.command("sources").description("Manage scrape sources");

sources
  .command("list", { isDefault: true })
  .description("List scrape sources")
  .action(async () => {
    await runCli(async () => {
      const list = await cliCall(() => client().listSources());
      heading(`Sources (${list.length})`);
      table(
        ["ID", "URL"],
        list.map((s) => [s.id, truncate(s.url, 74)]),
      );
    });
  });

sources
  .command("add")
  .description("Add a scrape source")
  .argument("<url>")
  .action(async (url: string) => {
    await runCli(async () => {
      const source = await cliCall(() => client().createSource(url));
      console.log(`  ${green("✓")} Source added as ${source.id}`);
    });
  });

sources
  .command("remove")
  .description("Remove a scrape source")
  .argument("<id>", "source id, as shown by `ps sources list`")
  .action(async (id: string) => {
    await runCli(async () => {
      await cliCall(() => client().deleteSource(id));
      console.log(`  ${green("✓")} Source ${id} removed`);
    });
  });

const scrape = program.command("scrape").description("Trigger and inspect scrapes");

scrape
  .command("status", { isDefault: true })
  .description("Show scrape state and health-check progress")
  .action(async () => {
    await runCli(async () => {
      const run = await cliCall(() => client().getCurrentScrapeRun());

      heading("Scrape");
      keyValue("State", run.state);
      keyValue("Last run", run.started_at ? new Date(run.started_at).toLocaleString() : null);
      keyValue("Proxies found", run.proxies_found);
      keyValue("Healthy", run.healthy_count);
      keyValue("Due for probe", run.pending);
      keyValue(
        "Next probe due",
        run.next_due_at ? new Date(run.next_due_at).toLocaleString() : null,
      );
      keyValue("Errors", run.errors.length);

      if (run.health_check) {
        const { checked, total } = run.health_check;
        const percent = total > 0 ? Math.round((checked / total) * 100) : 0;
        heading("Health check");
        keyValue("Progress", `${checked} / ${total} (${percent}%)`);
      }

      for (const error of run.errors) {
        console.log(`  ${red("!")} ${error}`);
      }
    });
  });

scrape
  .command("run")
  .description("Trigger a scrape cycle now")
  .action(async () => {
    await runCli(async () => {
      await cliCall(() => client().createScrapeRun());
      console.log(`  ${green("✓")} Scrape started`);
    });
  });

program
  .command("audit")
  .description("Show the audit log (admin only)")
  .option("-n, --limit <count>", "maximum rows", "25")
  .action(async (options: { limit: string }) => {
    await runCli(async () => {
      const limit = Number(options.limit);
      const entries = await cliCall(() =>
        client().listAuditEvents({ limit: Number.isFinite(limit) ? limit : 25 }),
      );

      heading(`Audit (${entries.length})`);
      table(
        ["WHEN", "ACTOR", "ACTION", "TARGET", "OUTCOME"],
        entries.map((e) => [
          new Date(e.ts).toLocaleString(),
          e.actor_type,
          truncate(e.action, 22),
          truncate(e.target ?? "—", 28),
          e.outcome === "allowed" ? green("ok") : red("denied"),
        ]),
      );
    });
  });

async function prompt(question: string, secret = false): Promise<string> {
  if (secret && process.stdin.isTTY) {
    process.stdout.write(question);
    const rl = createInterface({ input: process.stdin, output: process.stdout, terminal: true });
    // Suppress echo: the value is a live credential.
    const muted = new Promise<string>((resolve) => {
      const onData = (char: string) => {
        if (char === "\n" || char === "\r") {
          process.stdin.removeListener("data", onData);
        } else {
          resolve("");
        }
      };
      process.stdin.on("data", onData);
    });
    process.stdout.write(dim(" (hidden)"));
    const value = await Promise.race([rl.question(""), muted]);
    rl.close();
    process.stdout.write("\n");
    return value;
  }

  const rl = createInterface({ input: process.stdin, output: process.stdout });
  const answer = await rl.question(question);
  rl.close();
  return answer;
}

await program.parseAsync(process.argv);
