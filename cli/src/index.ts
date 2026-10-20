#!/usr/bin/env node
import { createInterface } from "node:readline/promises";
import { Command, Help, type Option } from "commander";
import { anonymousClient, CliError, cliCall, client } from "./client.js";
import { configPath, loadConfig, saveConfig } from "./config.js";
import { countryFlag, countryName, resolveLocale, translator } from "./i18n.js";
import { runCli } from "./run.js";
import {
  accent,
  badge,
  bar,
  bold,
  dim,
  emit,
  err,
  field,
  fieldLines,
  formatBytes,
  live,
  muted,
  ok,
  panel,
  panelLines,
  section,
  setJson,
  spinner,
  table,
} from "./ui.js";

/**
 * The language for this run.
 *
 * It is resolved before the command tree is built, because the help text is
 * part of that tree: a Russian `--help` should be in Russian too, and the
 * global option is only known once the arguments have been parsed.
 */
const locale = resolveLocale(localeArgument(process.argv));
setJson(jsonArgument(process.argv));
const t = translator(locale);

/** Whether the machine-readable output was asked for. */
function jsonArgument(argv: string[]): boolean {
  return argv.includes("--json");
}

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
  .option("--locale <locale>", t("common.locale"))
  .option("--json", t("common.json"));

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

          const waiting = spinner(t("auth.login.waiting"));
          try {
            const api = anonymousClient(options.url);
            const granted = await cliCall(() => api.exchangeToken({ email, password }));

            saveConfig({
              baseUrl: options.url ?? loadConfig().baseUrl,
              token: granted.token,
            });

            waiting.stop();
            console.log(
              `  ${ok("✓")} ${t("auth.login.signedIn", {
                email: granted.user.email,
                role: granted.user.role,
              })}`,
            );
            console.log(
              `  ${dim(t("auth.login.tokenStored", { prefix: granted.api_token.prefix, path: configPath() }))}`,
            );
          } catch (error) {
            waiting.stop();
            throw error;
          }
        });
      }),
  )
  .addCommand(
    new Command("status").description(t("auth.status.description")).action(async () => {
      await runCli(async () => {
        const config = loadConfig();
        panel(t("auth.status.title"), [
          [t("auth.status.apiUrl"), config.baseUrl],
          [
            t("auth.status.token"),
            config.token ? ok(t("auth.status.stored")) : err(t("auth.status.missing")),
          ],
          [t("auth.status.config"), configPath()],
        ]);
      });
    }),
  )
  .addCommand(
    new Command("logout").description(t("auth.logout.description")).action(async () => {
      await runCli(async () => {
        saveConfig({ baseUrl: loadConfig().baseUrl });
        console.log(`  ${ok("✓")} ${t("auth.logout.done")}`);
      });
    }),
  );

// With no subcommand the tool shows the dashboard: the state of the engine is
// what a person opening it wants first.
program.action(async () => {
  await program.parseAsync(["node", "ps", "status"]);
});

program
  .command("status")
  .description(t("status.description"))
  .option("-w, --watch", t("status.watch"))
  .action(async (options: { watch?: boolean }) => {
    await runCli(async () => {
      if (options.watch) {
        // The dashboard redraws itself, so watching it does not fill the
        // terminal with a copy every two seconds.
        const view = live(async () => {
          const api = client();
          const [summary, stats] = await Promise.all([
            cliCall(() => api.getSummary()),
            cliCall(() => api.getStats()),
          ]);

          const active = summary.active_proxy;
          return [
            ...panelLines(t("status.title"), [
              [t("status.poolSize"), bold(summary.pool_size.toLocaleString())],
              [t("status.healthy"), bold(summary.healthy_count.toLocaleString())],
              [
                t("status.activeProxy"),
                active
                  ? `${ok(`${active.host}:${active.port}`)}  ${dim(`${active.protocol} · ${active.anonymity}`)}`
                  : muted(t("status.noneSelected")),
              ],
            ]),
            "",
            ...fieldLines(t("status.network"), [
              [t("status.tunnelsOpened"), String(stats.tcp_connections_total ?? 0)],
              [t("status.tcpConnections"), String(stats.tcp_connections)],
              [t("status.udpFlows"), String(stats.udp_flows)],
              [t("status.dnsCache"), String(stats.dns_cache_size)],
              [
                t("status.traffic"),
                `${formatBytes(stats.bytes_sent ?? 0)} / ${formatBytes(stats.bytes_received ?? 0)}`,
              ],
            ]),
          ];
        });

        // The view runs until the command is interrupted. The signal ends
        // the process outright: there is nothing left to do once the
        // dashboard is stopped.
        const stop = () => {
          view.stop();
          process.exit(0);
        };
        process.once("SIGINT", stop);
        process.once("SIGTERM", stop);
        return;
      }

      const api = client();
      const [summary, stats] = await Promise.all([
        cliCall(() => api.getSummary()),
        cliCall(() => api.getStats()),
      ]);

      if (emit({ ...summary, stats })) {
        return;
      }

      const active = summary.active_proxy;
      panel(t("status.title"), [
        [t("status.poolSize"), bold(summary.pool_size.toLocaleString())],
        [t("status.healthy"), bold(summary.healthy_count.toLocaleString())],
        [
          t("status.activeProxy"),
          active
            ? `${ok(`${active.host}:${active.port}`)}  ${dim(`${active.protocol} · ${active.anonymity}`)}`
            : muted(t("status.noneSelected")),
        ],
      ]);

      section(t("status.network"));
      field(t("status.tunnelsOpened"), stats.tcp_connections_total ?? 0);
      field(t("status.tcpConnections"), stats.tcp_connections);
      field(t("status.udpFlows"), stats.udp_flows);
      field(t("status.dnsCache"), stats.dns_cache_size);
      field(
        t("status.traffic"),
        `${formatBytes(stats.bytes_sent ?? 0)} / ${formatBytes(stats.bytes_received ?? 0)}`,
      );
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

      if (emit(rows)) {
        return;
      }

      console.log(
        `\n  ${bold(t("proxies.list.title", { shown: rows.length, total: summary.pool_size }))}`,
      );

      table(
        [
          { header: "", max: 1 },
          { header: t("proxies.list.endpoint"), max: 26 },
          { header: t("proxies.list.protocol"), max: 8 },
          { header: t("proxies.list.latency"), align: "right", max: 9 },
          { header: t("proxies.list.flag"), max: 3 },
          { header: t("proxies.list.score"), align: "right", max: 5 },
        ],
        rows.map((p) => [
          p.id === summary.active_proxy?.id ? accent("▸") : " ",
          `${p.host}:${p.port}`,
          p.protocol,
          p.latency_ms === null ? "—" : `${p.latency_ms} ms`,
          countryFlag(p.country),
          p.score.toFixed(1),
        ]),
      );
      console.log();
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

        const waiting = spinner(t("proxies.add.waiting"));
        try {
          const proxy = await cliCall(() =>
            client().createProxy({
              host,
              port: portNumber,
              protocol,
              ...(options.country ? { country: options.country } : {}),
            }),
          );
          waiting.stop();
          console.log(`  ${ok("✓")} ${t("proxies.add.done", { id: proxy.id })}`);
        } catch (error) {
          waiting.stop();
          throw error;
        }
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
      console.log(`  ${ok("✓")} ${t("proxies.remove.done", { id })}`);
    });
  });

proxies
  .command("switch")
  .description(t("proxies.switch.description"))
  .argument("<id>", t("proxies.switch.id"))
  .action(async (id: string) => {
    await runCli(async () => {
      const proxy = await cliCall(() => client().setActiveProxy(id));
      console.log(`  ${ok("✓")} ${t("proxies.switch.done", { id: proxy.id })}`);
    });
  });

program
  .command("rotate")
  .description(t("rotate.description"))
  .action(async () => {
    await runCli(async () => {
      const proxy = await cliCall(() => client().rotateActiveProxy());
      console.log(`  ${ok("✓")} ${t("rotate.done", { id: proxy.id })}`);
    });
  });

const sources = program.command("sources").description(t("sources.group"));

sources
  .command("list", { isDefault: true })
  .description(t("sources.list.description"))
  .action(async () => {
    await runCli(async () => {
      const list = await cliCall(() => client().listSources());

      if (emit(list)) {
        return;
      }

      console.log(`\n  ${bold(t("sources.list.title", { count: list.length }))}`);
      table(
        [
          { header: t("sources.list.id"), max: 20 },
          { header: t("sources.list.url"), max: 70 },
        ],
        list.map((s) => [s.id, s.url]),
      );
      console.log();
    });
  });

sources
  .command("add")
  .description(t("sources.add.description"))
  .argument("<url>", t("sources.add.url"))
  .action(async (url: string) => {
    await runCli(async () => {
      const source = await cliCall(() => client().createSource(url));
      console.log(`  ${ok("✓")} ${t("sources.add.done", { id: source.id })}`);
    });
  });

sources
  .command("remove")
  .description(t("sources.remove.description"))
  .argument("<id>", t("sources.remove.id"))
  .action(async (id: string) => {
    await runCli(async () => {
      await cliCall(() => client().deleteSource(id));
      console.log(`  ${ok("✓")} ${t("sources.remove.done", { id })}`);
    });
  });

/* ── The local proxy, its policies and its credentials ─────────────────── */

const proxy = program.command("proxy").description(t("proxy.group"));

proxy
  .command("status", { isDefault: true })
  .description(t("proxy.status.description"))
  .action(async () => {
    await runCli(async () => {
      const state = await cliCall(() => client().getLocalProxy());

      if (emit(state)) {
        return;
      }

      panel(t("proxy.status.title"), [
        [
          t("proxy.status.address"),
          state.enabled ? ok(state.address) : muted(t("proxy.status.off")),
        ],
        [t("proxy.status.current"), state.current_proxy ? state.current_proxy : muted("—")],
        [t("proxy.status.connections"), String(state.connections)],
        [t("proxy.status.tunnels"), String(state.tunnels)],
        [t("proxy.status.sessions"), String(state.sessions)],
        [t("proxy.status.rotations"), String(state.rotations)],
        [t("proxy.status.failures"), String(state.failures)],
        [t("proxy.status.refused"), String(state.refused)],
        [
          t("proxy.status.traffic"),
          `${formatBytes(state.bytes_out)} / ${formatBytes(state.bytes_in)}`,
        ],
      ]);

      console.log();
      console.log(
        `  ${muted(t("proxy.status.hint"))}  ${bold(
          `http://name:password@${state.address.replace("http://", "")}`,
        )}`,
      );
      console.log();
    });
  });

const policies = program.command("policies").description(t("policies.group"));

policies
  .command("list", { isDefault: true })
  .description(t("policies.list.description"))
  .action(async () => {
    await runCli(async () => {
      const list = await cliCall(() => client().listPolicies());

      if (emit(list)) {
        return;
      }

      console.log(`\n  ${bold(t("policies.list.title", { count: list.length }))}`);
      table(
        [
          { header: t("policies.list.name"), max: 24 },
          { header: t("policies.list.rotation"), max: 14 },
          { header: t("policies.list.countries"), max: 30 },
        ],
        list.map((p) => [
          p.builtin ? `${p.name} ${dim("*")}` : p.name,
          p.settings.rotation,
          p.settings.countries.join(", ") || "—",
        ]),
      );
      console.log();
    });
  });

policies
  .command("create")
  .description(t("policies.create.description"))
  .argument("<name>", t("policies.create.name"))
  .option("-r, --rotation <mode>", t("policies.create.rotation"), "sticky")
  .option("-c, --countries <codes>", t("policies.create.countries"))
  .option("-s, --sticky-secs <seconds>", t("policies.create.sticky"))
  .option("-i, --interval-secs <seconds>", t("policies.create.interval"))
  .action(
    async (
      name: string,
      options: {
        rotation: "sticky" | "per_request" | "time";
        countries?: string;
        stickySecs?: string;
        intervalSecs?: string;
      },
    ) => {
      await runCli(async () => {
        const policy = await cliCall(() =>
          client().createPolicy({
            name,
            settings: {
              rotation: options.rotation,
              sticky_secs: Number(options.stickySecs ?? 600),
              rotate_interval_secs: Number(options.intervalSecs ?? 300),
              countries: (options.countries ?? "")
                .split(",")
                .map((code) => code.trim().toUpperCase())
                .filter(Boolean),
              protocols: [],
              min_score: 0,
            },
          }),
        );

        console.log(`  ${ok("✓")} ${t("policies.create.done", { name: policy.name })}`);
      });
    },
  );

policies
  .command("remove")
  .description(t("policies.remove.description"))
  .argument("<name>", t("policies.remove.name"))
  .action(async (name: string) => {
    await runCli(async () => {
      await cliCall(() => client().deletePolicy(name));
      console.log(`  ${ok("✓")} ${t("policies.remove.done", { name })}`);
    });
  });

const credentials = program.command("credentials").description(t("credentials.group"));

credentials
  .command("list", { isDefault: true })
  .description(t("credentials.list.description"))
  .action(async () => {
    await runCli(async () => {
      const list = await cliCall(() => client().listCredentials());

      if (emit(list)) {
        return;
      }

      console.log(`\n  ${bold(t("credentials.list.title", { count: list.length }))}`);
      table(
        [
          { header: t("credentials.list.name"), max: 24 },
          { header: t("credentials.list.policy"), max: 24 },
          { header: t("credentials.list.created"), max: 24 },
        ],
        list.map((c) => [c.username, c.policy, c.created_at.slice(0, 19).replace("T", " ")]),
      );
      console.log();
    });
  });

credentials
  .command("create")
  .description(t("credentials.create.description"))
  .argument("<name>", t("credentials.create.name"))
  .option("-p, --policy <name>", t("credentials.create.policy"), "default")
  .action(async (name: string, options: { policy: string }) => {
    await runCli(async () => {
      const credential = await cliCall(() =>
        client().createCredential({ username: name, policy: options.policy }),
      );

      if (emit(credential)) {
        return;
      }

      console.log();
      console.log(`  ${ok("✓")} ${t("credentials.create.done", { name: credential.username })}`);
      console.log(`  ${t("credentials.create.password")}  ${bold(credential.password ?? "")}`);
      const state = await cliCall(() => client().getLocalProxy());
      console.log(
        `  ${muted(t("credentials.create.hint"))}  ${bold(
          `http://${credential.username}:${credential.password}@${state.address.replace("http://", "")}`,
        )}`,
      );
      console.log();
    });
  });

credentials
  .command("remove")
  .description(t("credentials.remove.description"))
  .argument("<name>", t("credentials.remove.name"))
  .action(async (name: string) => {
    await runCli(async () => {
      await cliCall(() => client().deleteCredential(name));
      console.log(`  ${ok("✓")} ${t("credentials.remove.done", { name })}`);
    });
  });

const scrape = program.command("scrape").description(t("scrape.group"));

scrape
  .command("status", { isDefault: true })
  .description(t("scrape.status.description"))
  .option("-w, --watch", t("status.watch"))
  .action(async (options: { watch?: boolean }) => {
    await runCli(async () => {
      if (options.watch) {
        const view = live(async () => {
          const run = await cliCall(() => client().getCurrentScrapeRun());
          const progress = run.health_check;
          const percent =
            progress && progress.total > 0
              ? Math.round((progress.checked / progress.total) * 100)
              : 0;

          return [
            ...panelLines(t("scrape.status.title"), [
              [
                t("scrape.status.state"),
                `${run.state === "idle" ? muted("○") : ok("●")} ${run.state}`,
              ],
              [t("scrape.status.proxiesFound"), bold(run.proxies_found.toLocaleString())],
              [t("scrape.status.healthy"), bold(run.healthy_count.toLocaleString())],
              [t("scrape.status.dueForProbe"), bold(run.pending.toLocaleString())],
            ]),
            "",
            `  ${bar(progress?.checked ?? 0, progress?.total ?? 0)}  ${bold(
              `${(progress?.checked ?? 0).toLocaleString()}/${(progress?.total ?? 0).toLocaleString()}`,
            )} ${dim(`(${percent}%)`)}`,
            "",
          ];
        });

        const stop = () => {
          view.stop();
          process.exit(0);
        };
        process.once("SIGINT", stop);
        process.once("SIGTERM", stop);
        return;
      }

      const run = await cliCall(() => client().getCurrentScrapeRun());

      if (emit(run)) {
        return;
      }

      const state = run.state === "idle" ? "idle" : run.state === "running" ? "warn" : "ok";

      panel(t("scrape.status.title"), [
        [t("scrape.status.state"), `${badge(state)} ${run.state}`],
        [
          t("scrape.status.lastRun"),
          run.started_at
            ? new Date(run.started_at).toLocaleString(locale)
            : muted(t("common.never")),
        ],
        [t("scrape.status.proxiesFound"), bold(run.proxies_found.toLocaleString())],
        [t("scrape.status.healthy"), bold(run.healthy_count.toLocaleString())],
        [t("scrape.status.dueForProbe"), bold(run.pending.toLocaleString())],
        [
          t("scrape.status.nextDue"),
          run.next_due_at ? new Date(run.next_due_at).toLocaleString(locale) : muted("—"),
        ],
      ]);

      if (run.health_check) {
        const { checked, total } = run.health_check;
        const percent = total > 0 ? Math.round((checked / total) * 100) : 0;
        section(t("scrape.status.healthCheck"));
        console.log(
          `  ${bar(checked, total)}  ${bold(`${checked.toLocaleString()}/${total.toLocaleString()}`)} ${dim(`(${percent}%)`)}`,
        );
        console.log();
      }

      if (run.errors.length > 0) {
        section(t("scrape.status.errorsTitle"));
        for (const message of run.errors.slice(0, 8)) {
          console.log(`  ${err("!")} ${message}`);
        }
        console.log();
      }
    });
  });

scrape
  .command("run")
  .description(t("scrape.run.description"))
  .action(async () => {
    await runCli(async () => {
      await cliCall(() => client().createScrapeRun());
      console.log(`  ${ok("✓")} ${t("scrape.run.done")}`);
    });
  });

program
  .command("geo")
  .description(t("geo.description"))
  .option("-n, --limit <count>", t("geo.limit"), "20")
  .action(async (options: { limit: string }) => {
    await runCli(async () => {
      const limit = Number(options.limit);
      const statistics = await cliCall(() => client().getStatistics());

      if (emit(statistics)) {
        return;
      }

      panel(t("geo.title"), [
        [t("geo.poolSize"), bold(statistics.pool_size.toLocaleString())],
        [t("geo.working"), bold(statistics.working.toLocaleString())],
        [t("geo.exitAddresses"), bold(statistics.distinct_exit_addresses.toLocaleString())],
        [t("geo.countries"), bold(statistics.by_country.length.toLocaleString())],
      ]);

      console.log(`\n  ${bold(t("geo.byCountry", { count: statistics.by_country.length }))}`);
      table(
        [
          { header: t("geo.flag"), max: 3 },
          { header: t("geo.country"), max: 28 },
          { header: t("geo.proxies"), align: "right", max: 8 },
          { header: t("geo.workingColumn"), align: "right", max: 8 },
        ],
        statistics.by_country
          .slice(0, Number.isFinite(limit) ? limit : 20)
          .map((entry) => [
            countryFlag(entry.country),
            countryName(entry.country, locale),
            entry.count.toLocaleString(),
            entry.working.toLocaleString(),
          ]),
      );
      console.log();
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

      if (emit(entries)) {
        return;
      }

      console.log(`\n  ${bold(t("audit.title", { count: entries.length }))}`);
      table(
        [
          { header: t("audit.when"), max: 20 },
          { header: t("audit.actor"), max: 10 },
          { header: t("audit.action"), max: 24 },
          { header: t("audit.target"), max: 26 },
          { header: t("audit.outcome"), max: 8 },
        ],
        entries.map((e) => [
          new Date(e.ts).toLocaleString(locale),
          e.actor_type,
          e.action,
          e.target ?? "—",
          e.outcome === "allowed" ? ok(t("audit.allowed")) : err(t("audit.denied")),
        ]),
      );
      console.log();
    });
  });

async function prompt(question: string, secret = false): Promise<string> {
  if (secret && process.stdin.isTTY) {
    process.stdout.write(question);
    const rl = createInterface({ input: process.stdin, output: process.stdout, terminal: true });
    // Suppress echo: the value is a live credential.
    const mutedAnswer = new Promise<string>((resolve) => {
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
    const value = await Promise.race([rl.question(""), mutedAnswer]);
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
