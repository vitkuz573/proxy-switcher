"use client";

import type { PoolStatistics } from "@proxy-switcher/sdk";
import { useFormatter, useTranslations } from "next-intl";
import {
  Area,
  AreaChart,
  Bar,
  BarChart,
  CartesianGrid,
  Cell,
  Pie,
  PieChart,
  XAxis,
  YAxis,
} from "recharts";
import {
  type ChartConfig,
  ChartContainer,
  ChartLegend,
  ChartLegendContent,
  ChartTooltip,
  ChartTooltipContent,
} from "@/components/ui/chart";

/**
 * The colours the charts use.
 *
 * A fixed palette rather than a generated one, so the same protocol has the
 * same colour in every chart and from one run to the next.
 */
const PALETTE = [
  "oklch(0.65 0.19 250)",
  "oklch(0.72 0.17 155)",
  "oklch(0.75 0.16 85)",
  "oklch(0.68 0.19 20)",
  "oklch(0.65 0.17 300)",
  "oklch(0.72 0.15 190)",
  "oklch(0.78 0.14 130)",
  "oklch(0.62 0.16 15)",
];

const HISTORY_CONFIG = {
  pool: { label: "pool", color: PALETTE[0] },
  working: { label: "working", color: PALETTE[1] },
} satisfies ChartConfig;

const PROTOCOL_CONFIG = {
  count: { label: "proxies" },
  working: { label: "working" },
} satisfies ChartConfig;

const ANONYMITY_CONFIG = {
  count: { label: "proxies" },
} satisfies ChartConfig;

const LATENCY_CONFIG = {
  count: { label: "proxies" },
} satisfies ChartConfig;

const COUNTRY_CONFIG = {
  count: { label: "proxies" },
  working: { label: "working" },
} satisfies ChartConfig;

/** A number with thousands separated, so the axis is readable. */
function compact(value: number): string {
  if (value >= 1_000_000) {
    return `${(value / 1_000_000).toFixed(1)}M`;
  }
  if (value >= 1_000) {
    return `${(value / 1_000).toFixed(value >= 10_000 ? 0 : 1)}k`;
  }
  return String(value);
}

export function PoolStatisticsCharts({ statistics }: { statistics: PoolStatistics }) {
  const t = useTranslations("Statistics.charts");
  const format = useFormatter();

  const history = statistics.history.map((sample) => ({
    ...sample,
    at: format.dateTime(new Date(sample.at), {
      hour: "2-digit",
      minute: "2-digit",
    }),
  }));

  const protocols = statistics.by_protocol.map((entry) => ({
    protocol: entry.protocol,
    count: entry.count,
    working: entry.working,
  }));

  const anonymity = statistics.by_anonymity.map((entry) => ({
    anonymity: entry.anonymity,
    count: entry.count,
  }));

  // The range is named here, in the language of the reader: the engine reports
  // the boundaries and knows nothing about languages.
  const latency = statistics.latency.map((entry) => ({
    range:
      entry.to_ms === null
        ? t("latencyRangeOpen", { from: format.number(entry.from_ms) })
        : t("latencyRange", {
            from: format.number(entry.from_ms),
            to: format.number(entry.to_ms),
          }),
    count: entry.count,
  }));

  const countries = statistics.by_country.slice(0, 12).map((entry) => ({
    country: entry.country,
    count: entry.count,
    working: entry.working,
  }));

  return (
    <div className="grid gap-4 lg:grid-cols-2">
      <section className="lg:col-span-2 rounded-xl border p-4">
        <h2 className="text-sm font-medium">{t("historyTitle")}</h2>
        <p className="text-xs text-muted-foreground">{t("historyDescription")}</p>
        {history.length > 1 ? (
          <ChartContainer config={HISTORY_CONFIG} className="h-64 w-full">
            <AreaChart data={history}>
              <CartesianGrid vertical={false} />
              <XAxis dataKey="at" tickLine={false} axisLine={false} minTickGap={24} />
              <YAxis
                tickLine={false}
                axisLine={false}
                width={48}
                tickFormatter={(value: number) => compact(value)}
              />
              <ChartTooltip content={<ChartTooltipContent indicator="line" />} />
              <ChartLegend content={<ChartLegendContent />} />
              <Area
                dataKey="pool"
                type="monotone"
                stroke="var(--color-pool)"
                fill="var(--color-pool)"
                fillOpacity={0.18}
                strokeWidth={2}
              />
              <Area
                dataKey="working"
                type="monotone"
                stroke="var(--color-working)"
                fill="var(--color-working)"
                fillOpacity={0.25}
                strokeWidth={2}
              />
            </AreaChart>
          </ChartContainer>
        ) : (
          <p className="py-16 text-center text-sm text-muted-foreground">{t("historyEmpty")}</p>
        )}
      </section>

      <section className="rounded-xl border p-4">
        <h2 className="text-sm font-medium">{t("protocolTitle")}</h2>
        <p className="text-xs text-muted-foreground">{t("protocolDescription")}</p>
        <ChartContainer config={PROTOCOL_CONFIG} className="h-64 w-full">
          <BarChart data={protocols} layout="vertical" margin={{ left: 8 }}>
            <CartesianGrid horizontal={false} />
            <XAxis type="number" tickLine={false} axisLine={false} tickFormatter={compact} />
            <YAxis
              type="category"
              dataKey="protocol"
              tickLine={false}
              axisLine={false}
              width={64}
            />
            <ChartTooltip content={<ChartTooltipContent />} />
            <ChartLegend content={<ChartLegendContent />} />
            <Bar dataKey="count" fill="var(--color-count)" radius={4} />
            <Bar dataKey="working" fill="var(--color-working)" radius={4} />
          </BarChart>
        </ChartContainer>
      </section>

      <section className="rounded-xl border p-4">
        <h2 className="text-sm font-medium">{t("anonymityTitle")}</h2>
        <p className="text-xs text-muted-foreground">{t("anonymityDescription")}</p>
        <ChartContainer config={ANONYMITY_CONFIG} className="h-64 w-full">
          <PieChart>
            <ChartTooltip content={<ChartTooltipContent hideLabel />} />
            <Pie
              data={anonymity}
              dataKey="count"
              nameKey="anonymity"
              innerRadius={50}
              outerRadius={90}
              paddingAngle={2}
            >
              {anonymity.map((entry, index) => (
                <Cell key={entry.anonymity} fill={PALETTE[index % PALETTE.length]} />
              ))}
            </Pie>
            <ChartLegend content={<ChartLegendContent nameKey="anonymity" />} />
          </PieChart>
        </ChartContainer>
      </section>

      <section className="rounded-xl border p-4">
        <h2 className="text-sm font-medium">{t("latencyTitle")}</h2>
        <p className="text-xs text-muted-foreground">{t("latencyDescription")}</p>
        <ChartContainer config={LATENCY_CONFIG} className="h-64 w-full">
          <BarChart data={latency} margin={{ left: 8 }}>
            <CartesianGrid vertical={false} />
            <XAxis
              dataKey="range"
              tickLine={false}
              axisLine={false}
              interval={0}
              angle={-20}
              height={50}
              textAnchor="end"
            />
            <YAxis
              tickLine={false}
              axisLine={false}
              width={48}
              tickFormatter={(value: number) => compact(value)}
            />
            <ChartTooltip content={<ChartTooltipContent />} />
            <Bar dataKey="count" fill="var(--color-count)" radius={4} />
          </BarChart>
        </ChartContainer>
      </section>

      <section className="rounded-xl border p-4">
        <h2 className="text-sm font-medium">{t("countryTitle")}</h2>
        <p className="text-xs text-muted-foreground">{t("countryDescription")}</p>
        <ChartContainer config={COUNTRY_CONFIG} className="h-64 w-full">
          <BarChart data={countries} layout="vertical" margin={{ left: 8 }}>
            <CartesianGrid horizontal={false} />
            <XAxis type="number" tickLine={false} axisLine={false} tickFormatter={compact} />
            <YAxis
              type="category"
              dataKey="country"
              tickLine={false}
              axisLine={false}
              width={110}
            />
            <ChartTooltip content={<ChartTooltipContent />} />
            <ChartLegend content={<ChartLegendContent />} />
            <Bar dataKey="count" fill="var(--color-count)" radius={4} />
            <Bar dataKey="working" fill="var(--color-working)" radius={4} />
          </BarChart>
        </ChartContainer>
      </section>
    </div>
  );
}
