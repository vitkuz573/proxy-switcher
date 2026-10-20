import { Network } from "lucide-react";
import type { Metadata } from "next";
import { getTranslations, setRequestLocale } from "next-intl/server";
import { LocalProxyPanel } from "@/components/local-proxy-panel";
import { getCredentials, getLocalProxy, getPolicies } from "@/lib/api";
import { getCurrentUser } from "@/lib/auth";

type Props = { params: Promise<{ locale: string }> };

export async function generateMetadata({ params }: Props): Promise<Metadata> {
  const { locale } = await params;
  const t = await getTranslations({ locale, namespace: "Proxy" });

  return { title: t("title") };
}

export default async function ProxyPage({ params }: Props) {
  const { locale } = await params;
  setRequestLocale(locale);
  const t = await getTranslations("Proxy");

  const user = await getCurrentUser();
  const [state, policies, credentials] = await Promise.all([
    getLocalProxy(),
    getPolicies(),
    getCredentials(),
  ]);

  return (
    <>
      <header>
        <h1 className="text-2xl font-semibold tracking-tight">{t("title")}</h1>
        <p className="text-sm text-muted-foreground">
          <span className="inline-flex items-center gap-1">
            <Network className="size-3.5" />
            {t("subtitle")}
          </span>
        </p>
      </header>

      <LocalProxyPanel
        state={state}
        policies={policies}
        credentials={credentials}
        canWrite={user.role !== "viewer"}
      />
    </>
  );
}
