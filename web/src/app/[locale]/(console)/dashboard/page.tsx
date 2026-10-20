import { Activity, Globe, Server, Waypoints } from "lucide-react";
import type { Metadata } from "next";
import { getTranslations, setRequestLocale } from "next-intl/server";
import { HealthProgress } from "@/components/health-progress";
import { Badge } from "@/components/ui/badge";
import { Card, CardContent } from "@/components/ui/card";
import { getRouting, getScrapeRun, getStats, getSummary } from "@/lib/api";

type Props = { params: Promise<{ locale: string }> };

export async function generateMetadata({ params }: Props): Promise<Metadata> {
  const { locale } = await params;
  const t = await getTranslations({ locale, namespace: "Dashboard" });

  return { title: t("title") };
}

export default async function DashboardPage({ params }: Props) {
  const { locale } = await params;
  setRequestLocale(locale);
  const t = await getTranslations("Dashboard");

  return (
    <>
      <header>
        <h1 className="text-2xl font-semibold tracking-tight">{t("title")}</h1>
        <p className="text-sm text-muted-foreground">{t("subtitle")}</p>
      </header>

      {/* Each section reads the session, so each streams in behind the layout's
          Suspense boundary instead of blocking the whole route. */}
      <StatsRow />
      <TrafficPanel />
      <RoutingPanel />
      <ScraperPanel />
    </>
  );
}

async function StatsRow() {
  const t = await getTranslations("Dashboard");
  const [summary, stats] = await Promise.all([getSummary(), getStats()]);

  return (
    <section className="grid gap-4 sm:grid-cols-2 xl:grid-cols-4">
      <Stat
        icon={<Server className="size-4" />}
        label={t("poolSize")}
        value={summary?.pool_size ?? 0}
        hint={t("poolSizeHint")}
      />
      <Stat
        icon={<Activity className="size-4" />}
        label={t("healthy")}
        value={stats?.healthy_count ?? 0}
        hint={t("healthyHint")}
      />
      <Stat
        icon={<Waypoints className="size-4" />}
        label={t("tunnelsOpened")}
        value={stats?.tcp_connections_total ?? 0}
        hint={t("tunnelsOpenedHint")}
      />
      <Stat
        icon={<Globe className="size-4" />}
        label={t("dnsCache")}
        value={stats?.dns_cache_size ?? 0}
        hint={t("dnsCacheHint")}
      />
    </section>
  );
}

/**
 * What the device has carried.
 *
 * The live count of connections is in here too, but on its own it says
 * almost nothing: a request goes through in a fraction of a second, so a
 * dashboard that shows only the live number looks broken even while the
 * device is working. The totals are the numbers that describe the work.
 */
async function TrafficPanel() {
  const t = await getTranslations("Dashboard");
  const stats = await getStats();

  const sent = stats?.bytes_sent ?? 0;
  const received = stats?.bytes_received ?? 0;

  return (
    <Card>
      <CardContent className="grid gap-3">
        <div className="flex items-center justify-between text-sm">
          <span className="text-muted-foreground">{t("traffic")}</span>
          <Badge variant="outline">
            {t("connectionsNow", { count: stats?.tcp_connections ?? 0 })}
          </Badge>
        </div>
        <dl className="grid gap-3 sm:grid-cols-3">
          <Measure label={t("sent")} value={sent} />
          <Measure label={t("received")} value={received} />
          <Measure label={t("total")} value={sent + received} />
        </dl>
      </CardContent>
    </Card>
  );
}

async function RoutingPanel() {
  const t = await getTranslations("Dashboard");
  const routing = await getRouting();

  const rules = (routing?.policy.cidrs.length ?? 0) + (routing?.policy.domains.length ?? 0);

  return (
    <Card>
      <CardContent className="grid gap-2">
        <div className="flex items-center justify-between text-sm">
          <span className="text-muted-foreground">{t("routing")}</span>
          <div className="flex items-center gap-2">
            {routing?.active_proxy ? (
              <Badge variant="outline">{routing.active_proxy}</Badge>
            ) : (
              <Badge variant="destructive">{t("noActiveProxy")}</Badge>
            )}
            <Badge variant={routing?.enabled ? "default" : "outline"}>
              {routing?.enabled ? routing.policy.mode : "none"}
            </Badge>
          </div>
        </div>
        {routing?.enabled ? (
          <p className="text-xs text-muted-foreground">
            {t("routingActive", {
              rules,
              destinations: routing.destinations,
              routes: routing.routes,
            })}
          </p>
        ) : (
          <p className="text-xs text-muted-foreground">{t("routingNone")}</p>
        )}
      </CardContent>
    </Card>
  );
}

async function ScraperPanel() {
  const scrape = await getScrapeRun();

  return <HealthProgress initial={scrape} />;
}

function Stat({
  icon,
  label,
  value,
  hint,
}: {
  icon: React.ReactNode;
  label: string;
  value: number;
  hint: string;
}) {
  return (
    <Card>
      <CardContent className="grid gap-1">
        <div className="flex items-center justify-between text-sm text-muted-foreground">
          <span className="flex items-center gap-1.5">
            {icon}
            {label}
          </span>
          <Badge variant="outline">{hint}</Badge>
        </div>
        <p className="text-3xl font-semibold tabular-nums">{value.toLocaleString()}</p>
      </CardContent>
    </Card>
  );
}

/** A number with a unit, kept readable. */
function Measure({ label, value }: { label: string; value: number }) {
  return (
    <div className="grid gap-0.5">
      <dt className="text-xs text-muted-foreground">{label}</dt>
      <dd className="text-xl font-semibold tabular-nums">{formatBytes(value)}</dd>
    </div>
  );
}

/** Bytes in the unit a reader can picture. */
function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} kB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
  return `${(bytes / 1024 / 1024 / 1024).toFixed(1)} GB`;
}
