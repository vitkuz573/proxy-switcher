import { Activity, Globe, RefreshCw, Server } from "lucide-react";
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
        icon={<RefreshCw className="size-4" />}
        label={t("tcpConnections")}
        value={stats?.tcp_connections ?? 0}
        hint={t("tcpConnectionsHint")}
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

async function RoutingPanel() {
  const t = await getTranslations("Dashboard");
  const routing = await getRouting();

  return (
    <Card>
      <CardContent className="grid gap-2">
        <div className="flex items-center justify-between text-sm">
          <span className="text-muted-foreground">{t("routing")}</span>
          <Badge variant={routing?.enabled ? "default" : "outline"}>
            {routing?.enabled ? routing.policy.mode : "none"}
          </Badge>
        </div>
        <p className="text-xs text-muted-foreground">
          {routing?.enabled
            ? `${routing.policy.cidrs.length + routing.policy.domains.length} rules apply`
            : t("routingNone")}
        </p>
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
