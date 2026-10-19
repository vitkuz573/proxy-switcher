import { Activity, Globe, RefreshCw, Server } from "lucide-react";
import { HealthProgress, IDLE_SCRAPE_RUN } from "@/components/health-progress";
import { Badge } from "@/components/ui/badge";
import { Card, CardContent } from "@/components/ui/card";
import { getScrapeRun, getStats, getSummary } from "@/lib/api";

export const metadata = { title: "Dashboard" };

export default function DashboardPage() {
  return (
    <>
      <header>
        <h1 className="text-2xl font-semibold tracking-tight">Dashboard</h1>
        <p className="text-sm text-muted-foreground">
          System-wide IP switching through public proxies.
        </p>
      </header>

      {/* Each section reads the session, so each streams in behind the layout's
          Suspense boundary instead of blocking the whole route. */}
      <StatsRow />
      <ScraperPanel />
    </>
  );
}

async function StatsRow() {
  const [summary, stats] = await Promise.all([getSummary(), getStats()]);

  return (
    <section className="grid gap-4 sm:grid-cols-2 xl:grid-cols-4">
      <Stat
        icon={<Server className="size-4" />}
        label="Pool size"
        value={summary?.pool_size ?? 0}
        hint="proxies in the engine pool"
      />
      <Stat
        icon={<Activity className="size-4" />}
        label="Healthy"
        value={stats?.healthy_count ?? 0}
        hint="confirmed reachable"
      />
      <Stat
        icon={<RefreshCw className="size-4" />}
        label="TCP connections"
        value={stats?.tcp_connections ?? 0}
        hint="active forwards"
      />
      <Stat
        icon={<Globe className="size-4" />}
        label="DNS cache"
        value={stats?.dns_cache_size ?? 0}
        hint="resolved names"
      />
    </section>
  );
}

async function ScraperPanel() {
  return <HealthProgress initial={(await getScrapeRun()) ?? IDLE_SCRAPE_RUN} />;
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
