import { AlertTriangle } from "lucide-react";
import type { Metadata } from "next";
import { getTranslations, setRequestLocale } from "next-intl/server";
import { HealthProgress } from "@/components/health-progress";
import { SourcesPanel } from "@/components/sources-panel";
import { getScrapeRun, getSources } from "@/lib/api";
import { getCurrentUser } from "@/lib/auth";

type Props = { params: Promise<{ locale: string }> };

export async function generateMetadata({ params }: Props): Promise<Metadata> {
  const { locale } = await params;
  const t = await getTranslations({ locale, namespace: "Scraper" });

  return { title: t("title") };
}

export default async function ScraperPage({ params }: Props) {
  const { locale } = await params;
  setRequestLocale(locale);
  const t = await getTranslations("Scraper");

  const user = await getCurrentUser();
  const [scrape, sources] = await Promise.all([getScrapeRun(), getSources()]);

  const run = scrape;

  return (
    <>
      <header>
        <h1 className="text-2xl font-semibold tracking-tight">{t("title")}</h1>
        <p className="text-sm text-muted-foreground">{t("subtitle")}</p>
      </header>

      {run && run.errors.length > 0 ? (
        <div className="flex items-start gap-3 rounded-lg border border-destructive/40 bg-destructive/5 p-4">
          <AlertTriangle className="mt-0.5 size-4 shrink-0 text-destructive" />
          <div className="grid gap-1 text-sm">
            <p className="font-medium text-destructive">{t("errorsTitle")}</p>
            <ul className="text-muted-foreground">
              {[...new Set(run.errors)].map((message) => (
                <li key={message} className="truncate">
                  {message}
                </li>
              ))}
            </ul>
          </div>
        </div>
      ) : null}

      <HealthProgress initial={run} />
      <SourcesPanel initial={sources} canManage={user.role === "admin"} />
    </>
  );
}
