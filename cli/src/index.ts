#!/usr/bin/env node
import { createInterface } from "node:readline/promises";
import { Command, Help, type Option } from "commander";
import { anonymousClient, CliError, cliCall, client } from "./client.js";
import { configPath, loadConfig, saveConfig } from "./config.js";
import { dim, green, heading, keyValue, red, table, truncate } from "./format.js";
import { countryFlag, countryName, resolveLocale, translator } from "./i18n.js";
import { runCli } from "./run.js";

/**
 * The language for this run.
 *
 * It is resolved before the command tree is built, because the help text is
 * part of that tree: a Russian `--help` should be in Russian too, and the
 * global option is only known once the arguments have been parsed.
 */
const locale = resolveLocale(localeArgument(process.argv));
const t = translator(locale);

/** The value of the global `--locale` option, read straight from the arguments. */
function localeArgument(argv: string[]): string | undefined {
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index] ?? "";

    if (argument === "--locale" || argument === "-l") {
      return argv[index + 1];
    }
    if (argument.startsWith("--locale=")) {
      return argument.slice("--locale=".length);
    }
  }
  return undefined;
}

const program = new Command();

// The help is part of the interface, so it is translated too.
//
// Commander writes the section headings itself — "Usage:", "Options:",
// "Commands:" — and the descriptions of the options it adds itself are English
// as well. The formatter is therefore given these overrides: they are own
// properties of the configuration, which is how commander copies them onto its
// help object.
const baseHelp = new Help();

program.configureHelp({
  formatItemList(this: Help, heading: string, items: string[], helper: Help): string[] {
    return baseHelp.formatItemList.call(this, translateHeading(heading), items, helper);
  },

  styleTitle(this: Help, title: string): string {
    return baseHelp.styleTitle.call(this, translateHeading(title));
  },

  optionDescription(this: Help, option: Option): string {
    // The descriptions commander writes for the options it adds itself.
    const key = BUILTIN_OPTION_DESCRIPTIONS[option.description ?? ""];
    return key ? t(key) : (option.description ?? "");
  },
});

/** The descriptions commander writes for the options it adds itself. */
const BUILTIN_OPTION_DESCRIPTIONS: Record<string, string> = {
  "output the version number": "help.version",
  "display help for command": "help.option",
  "display help": "help.option",
};

/**
 * Translate a heading commander writes.
 *
 * The translation has to be idempotent: a heading that has already been
 * translated comes back through the formatter a second time, and translating it
 * again would look for a key that does not exist.
 */
function translateHeading(heading: string): string {
  const key = heading
    .replace(/[:\s]+$/, "")
    .toLowerCase()
    .replace(/\s+/g, "-");
  const name = key === "global-options" ? "options" : key;

  return t.maybe(`help.${name}`) ?? heading;
}

program
  .name("ps")
  .description(t("common.description"))
  .version("2.0.0")
  .option("-u, --url <url>", t("common.apiUrl"))
  .option("--locale <locale>", t("common.locale"));

program
  .command("auth")
  .description(t("auth.group"))
  .addCommand(
    new Command("login")
      .description(t("auth.login.description"))
      .option("-e, --email <email>", t("auth.login.email"))
      .option("-p, --password <password>", t("auth.login.password"))
      .option("--url <url>", t("common.apiUrl"))
      .action(async (options: { email?: string; password?: string; url?: string }) => {
        await runCli(async () => {
          const email = options.email ?? (await prompt(t("auth.login.emailPrompt")));
          const password = options.password ?? (await prompt(t("auth.login.passwordPrompt"), true));

          const api = anonymousClient(options.url);
          const granted = await cliCall(() => api.exchangeToken({ email, password }));

          saveConfig({
            baseUrl: options.url ?? loadConfig().baseUrl,
            token: granted.token,
          });

          console.log(
            `\n  ${green("✓")} ${t("auth.login.signedIn", {
              email: granted.user.email,
              role: granted.user.role,
            })}`,
          );
          console.log(
            `  ${dim(t("auth.login.tokenStored", { prefix: granted.api_token.prefix, path: configPath() }))}`,
          );
        });
      }),
  )
  .addCommand(
    new Command("status").description(t("auth.status.description")).action(async () => {
      await runCli(async () => {
        const config = loadConfig();
        keyValue(t("auth.status.apiUrl"), config.baseUrl);
        keyValue(
          t("auth.status.token"),
          config.token ? green(t("auth.status.stored")) : red(t("auth.status.missing")),
        );
        keyValue(t("auth.status.config"), configPath());
      });
    }),
  )
  .addCommand(
    new Command("logout").description(t("auth.logout.description")).action(async () => {
      await runCli(async () => {
        saveConfig({ baseUrl: loadConfig().baseUrl });
        console.log(`  ${green("✓")} ${t("auth.logout.done")}`);
      });
    }),
  );

program
  .command("status")
  .description(t("status.description"))
  .action(async () => {
    await runCli(async () => {
      const api = client();
      const [summary, stats] = await Promise.all([
        cliCall(() => api.getSummary()),
        cliCall(() => api.getStats()),
      ]);

      heading(t("status.engine"));
      keyValue(t("status.poolSize"), summary.pool_size);
      keyValue(t("status.healthy"), stats.healthy_count);
      keyValue(t("status.tcpConnections"), stats.tcp_connections);
      keyValue(t("status.udpFlows"), stats.udp_flows);
      keyValue(t("status.dnsCache"), stats.dns_cache_size);

      heading(t("status.activeProxy"));
      if (!summary.active_proxy) {
        console.log(`  ${dim(t("status.noneSelected"))}`);
        return;
      }
      const p = summary.active_proxy;
      keyValue(t("status.endpoint"), `${p.host}:${p.port}`);
      keyValue(t("status.protocol"), p.protocol);
      keyValue(t("status.anonymity"), p.anonymity);
      keyValue(t("status.latency"), p.latency_ms === null ? null : `${p.latency_ms} ms`);
      keyValue(t("status.score"), p.score.toFixed(1));
    });
  });

const proxies = program.command("proxies").description(t("proxies.group"));

proxies
  .command("list", { isDefault: true })
  .description(t("proxies.list.description"))
  .option("-s, --search <text>", t("proxies.list.search"))
  .option("-n, --limit <count>", t("proxies.list.limit"), "25")
  .option("--sort <key>", t("proxies.list.sort"), "score")
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

      heading(t("proxies.list.title", { shown: rows.length, total: summary.pool_size }));
      table(
        t.list("proxies.list.columns"),
        rows.map((p) => [
          p.id === summary.active_proxy?.id ? "*" : " ",
          truncate(`${p.host}:${p.port}`, 26),
          truncate(p.protocol, 7),
          truncate(p.latency_ms === null ? "—" : `${p.latency_ms} ms`, 9),
          countryFlag(p.country),
          p.score.toFixed(1),
        ]),
      );
    });
  });

proxies
  .command("add")
  .description(t("proxies.add.description"))
  .argument("<host>", t("proxies.add.host"))
  .argument("<port>", t("proxies.add.port"))
  .argument("[protocol]", t("proxies.add.protocol"), "http")
  .option("--country <code>", t("proxies.add.country"))
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
          throw new CliError(t("proxies.add.badPort"));
        }

        const proxy = await cliCall(() =>
          client().createProxy({
            host,
            port: portNumber,
            protocol,
            ...(options.country ? { country: options.country } : {}),
          }),
        );
        console.log(`  ${green("✓")} ${t("proxies.add.done", { id: proxy.id })}`);
      });
    },
  );

proxies
  .command("remove")
  .description(t("proxies.remove.description"))
  .argument("<id>", t("proxies.remove.id"))
  .action(async (id: string) => {
    await runCli(async () => {
      await cliCall(() => client().deleteProxy(id));
      console.log(`  ${green("✓")} ${t("proxies.remove.done", { id })}`);
    });
  });

proxies
  .command("switch")
  .description(t("proxies.switch.description"))
  .argument("<id>", t("proxies.switch.id"))
  .action(async (id: string) => {
    await runCli(async () => {
      const proxy = await cliCall(() => client().setActiveProxy(id));
      console.log(`  ${green("✓")} ${t("proxies.switch.done", { id: proxy.id })}`);
    });
  });

program
  .command("rotate")
  .description(t("rotate.description"))
  .action(async () => {
    await runCli(async () => {
      const proxy = await cliCall(() => client().rotateActiveProxy());
      console.log(`  ${green("✓")} ${t("rotate.done", { id: proxy.id })}`);
    });
  });

const sources = program.command("sources").description(t("sources.group"));

sources
  .command("list", { isDefault: true })
  .description(t("sources.list.description"))
  .action(async () => {
    await runCli(async () => {
      const list = await cliCall(() => client().listSources());
      heading(t("sources.list.title", { count: list.length }));
      table(
        t.list("sources.list.columns"),
        list.map((s) => [s.id, truncate(s.url, 74)]),
      );
    });
  });

sources
  .command("add")
  .description(t("sources.add.description"))
  .argument("<url>", t("sources.add.url"))
  .action(async (url: string) => {
    await runCli(async () => {
      const source = await cliCall(() => client().createSource(url));
      console.log(`  ${green("✓")} ${t("sources.add.done", { id: source.id })}`);
    });
  });

sources
  .command("remove")
  .description(t("sources.remove.description"))
  .argument("<id>", t("sources.remove.id"))
  .action(async (id: string) => {
    await runCli(async () => {
      await cliCall(() => client().deleteSource(id));
      console.log(`  ${green("✓")} ${t("sources.remove.done", { id })}`);
    });
  });

const scrape = program.command("scrape").description(t("scrape.group"));

scrape
  .command("status", { isDefault: true })
  .description(t("scrape.status.description"))
  .action(async () => {
    await runCli(async () => {
      const run = await cliCall(() => client().getCurrentScrapeRun());

      heading(t("scrape.status.title"));
      keyValue(t("scrape.status.state"), run.state);
      keyValue(
        t("scrape.status.lastRun"),
        run.started_at ? new Date(run.started_at).toLocaleString() : null,
      );
      keyValue(t("scrape.status.proxiesFound"), run.proxies_found);
      keyValue(t("scrape.status.healthy"), run.healthy_count);
      keyValue(t("scrape.status.dueForProbe"), run.pending);
      keyValue(
        t("scrape.status.nextDue"),
        run.next_due_at ? new Date(run.next_due_at).toLocaleString() : null,
      );
      keyValue(t("scrape.status.errors"), run.errors.length);

      if (run.health_check) {
        const { checked, total } = run.health_check;
        const percent = total > 0 ? Math.round((checked / total) * 100) : 0;
        heading(t("scrape.status.healthCheck"));
        keyValue(t("scrape.status.progress", { checked, total, percent }), "");
      }

      for (const error of run.errors) {
        console.log(`  ${red("!")} ${error}`);
      }
    });
  });

scrape
  .command("run")
  .description(t("scrape.run.description"))
  .action(async () => {
    await runCli(async () => {
      await cliCall(() => client().createScrapeRun());
      console.log(`  ${green("✓")} ${t("scrape.run.done")}`);
    });
  });

program
  .command("geo")
  .description("Show where the pool is")
  .option("-n, --limit <count>", "how many countries to show", "20")
  .action(async (options: { limit: string }) => {
    await runCli(async () => {
      const limit = Number(options.limit);
      const statistics = await cliCall(() => client().getStatistics());

      heading(t("geo.title"));
      keyValue(t("geo.poolSize"), statistics.pool_size);
      keyValue(t("geo.working"), statistics.working);
      keyValue(t("geo.exitAddresses"), statistics.distinct_exit_addresses);
      keyValue(t("geo.countries"), statistics.by_country.length);

      const countries = statistics.by_country.slice(0, Number.isFinite(limit) ? limit : 20);

      heading(t("geo.byCountry", { count: countries.length }));
      table(
        t.list("geo.columns"),
        countries.map((entry) => [
          countryFlag(entry.country),
          truncate(countryName(entry.country, locale), 24),
          String(entry.count),
          String(entry.working),
        ]),
      );
    });
  });

program
  .command("audit")
  .description(t("audit.description"))
  .option("-n, --limit <count>", t("audit.limit"), "25")
  .action(async (options: { limit: string }) => {
    await runCli(async () => {
      const limit = Number(options.limit);
      const entries = await cliCall(() =>
        client().listAuditEvents({ limit: Number.isFinite(limit) ? limit : 25 }),
      );

      heading(t("audit.title", { count: entries.length }));
      table(
        t.list("audit.columns"),
        entries.map((e) => [
          new Date(e.ts).toLocaleString(),
          e.actor_type,
          truncate(e.action, 22),
          truncate(e.target ?? "—", 28),
          e.outcome === "allowed" ? green(t("audit.allowed")) : red(t("audit.denied")),
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
