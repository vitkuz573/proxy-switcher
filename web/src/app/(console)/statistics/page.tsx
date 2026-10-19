import { PoolStatisticsCharts } from "@/components/pool-statistics-charts";
import { Badge } from "@/components/ui/badge";
import { Card, CardContent } from "@/components/ui/card";
import { getStatistics } from "@/lib/api";

export const metadata = { title: "Статистика" };

/**
 * The distribution of the pool.
 *
 * The numbers come from one pass over the pool, so the charts and the totals
 * always agree with each other.
 */
export default async function StatisticsPage() {
  const statistics = await getStatistics();

  if (!statistics) {
    return (
      <Card>
        <CardContent className="py-16 text-center text-sm text-muted-foreground">
          Движок недоступен.
        </CardContent>
      </Card>
    );
  }

  return (
    <>
      <header>
        <h1 className="text-2xl font-semibold tracking-tight">Статистика пула</h1>
        <p className="text-sm text-muted-foreground">
          Распределение прокси по протоколам, анонимности, задержке и странам.
        </p>
      </header>

      <section className="grid gap-4 sm:grid-cols-2 xl:grid-cols-4">
        <Metric label="В пуле" value={statistics.pool_size} />
        <Metric label="Рабочих" value={statistics.working} />
        <Metric label="С туннелем" value={statistics.tunnel_capable} />
        <Metric label="Ждут проверки" value={statistics.due} />
      </section>

      <PoolStatisticsCharts statistics={statistics} />

      <p className="text-xs text-muted-foreground">
        Разных адресов выхода: {statistics.distinct_exit_addresses.toLocaleString("ru-RU")} —
        столько на самом деле прокси среди {statistics.pool_size.toLocaleString("ru-RU")} записей.
      </p>
    </>
  );
}

function Metric({ label, value }: { label: string; value: number }) {
  return (
    <Card>
      <CardContent className="grid gap-1">
        <div className="flex items-center justify-between text-sm text-muted-foreground">
          <span>{label}</span>
          <Badge variant="outline">пул</Badge>
        </div>
        <p className="text-3xl font-semibold tabular-nums">{value.toLocaleString("ru-RU")}</p>
      </CardContent>
    </Card>
  );
}
