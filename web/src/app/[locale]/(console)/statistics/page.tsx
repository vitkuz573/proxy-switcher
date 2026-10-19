import type { Metadata } from "next";
import { getTranslations, setRequestLocale } from "next-intl/server";
import { PoolStatisticsCharts } from "@/components/pool-statistics-charts";
import { Badge } from "@/components/ui/badge";
import { Card, CardContent } from "@/components/ui/card";
import { getStatistics } from "@/lib/api";

type Props = { params: Promise<{ locale: string }> };

export async function generateMetadata({ params }: Props): Promise<Metadata> {
  const { locale } = await params;
  const t = await getTranslations({ locale, namespace: "Statistics" });

  return { title: t("title") };
}

/**
 * The distribution of the pool.
 *
 * The numbers come from one pass over the pool, so the charts and the totals
 * always agree with each other.
 */
export default async function StatisticsPage({ params }: Props) {
  const { locale } = await params;
  setRequestLocale(locale);
  const t = await getTranslations("Statistics");

  const statistics = await getStatistics();

  if (!statistics) {
    return (
      <Card>
        <CardContent className="py-16 text-center text-sm text-muted-foreground">
          {t("unavailable")}
        </CardContent>
      </Card>
    );
  }

  return (
    <>
      <header>
        <h1 className="text-2xl font-semibold tracking-tight">{t("title")}</h1>
        <p className="text-sm text-muted-foreground">{t("subtitle")}</p>
      </header>

      <section className="grid gap-4 sm:grid-cols-2 xl:grid-cols-4">
        <Metric
          label={t("metrics.pool")}
          value={statistics.pool_size}
          hint={t("metrics.poolHint")}
        />
        <Metric
          label={t("metrics.working")}
          value={statistics.working}
          hint={t("metrics.poolHint")}
        />
        <Metric
          label={t("metrics.tunnel")}
          value={statistics.tunnel_capable}
          hint={t("metrics.poolHint")}
        />
        <Metric label={t("metrics.due")} value={statistics.due} hint={t("metrics.poolHint")} />
      </section>

      <PoolStatisticsCharts statistics={statistics} />

      <p className="text-xs text-muted-foreground">
        {t("exitAddresses", {
          count: statistics.distinct_exit_addresses.toLocaleString(),
          total: statistics.pool_size.toLocaleString(),
        })}
      </p>
    </>
  );
}

function Metric({ label, value, hint }: { label: string; value: number; hint: string }) {
  return (
    <Card>
      <CardContent className="grid gap-1">
        <div className="flex items-center justify-between text-sm text-muted-foreground">
          <span>{label}</span>
          <Badge variant="outline">{hint}</Badge>
        </div>
        <p className="text-3xl font-semibold tabular-nums">{value.toLocaleString()}</p>
      </CardContent>
    </Card>
  );
}
