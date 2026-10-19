import { Globe } from "lucide-react";
import type { Metadata } from "next";
import { getTranslations, setRequestLocale } from "next-intl/server";
import { ProxiesTable } from "@/components/proxies-table";
import { getProxies, getSummary } from "@/lib/api";
import { getCurrentUser } from "@/lib/auth";

type Props = { params: Promise<{ locale: string }> };

export async function generateMetadata({ params }: Props): Promise<Metadata> {
  const { locale } = await params;
  const t = await getTranslations({ locale, namespace: "Proxies" });

  return { title: t("title") };
}

export default async function ProxiesPage({ params }: Props) {
  const { locale } = await params;
  setRequestLocale(locale);
  const t = await getTranslations("Proxies");

  const user = await getCurrentUser();
  const [proxies, summary] = await Promise.all([getProxies(), getSummary()]);

  return (
    <>
      <header>
        <h1 className="text-2xl font-semibold tracking-tight">{t("title")}</h1>
        <p className="text-sm text-muted-foreground">
          {proxies.length.toLocaleString()} in the pool,{" "}
          <span className="inline-flex items-center gap-1">
            <Globe className="size-3.5" />
            ranked by measured latency
          </span>
          .
        </p>
      </header>

      <ProxiesTable
        initial={proxies}
        activeId={summary?.active_proxy?.id ?? null}
        canWrite={user.role !== "viewer"}
      />
    </>
  );
}
