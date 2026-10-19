import { Globe } from "lucide-react";
import { ProxiesTable } from "@/components/proxies-table";
import { getProxies, getSummary } from "@/lib/api";
import { getCurrentUser } from "@/lib/auth";

export const metadata = { title: "Proxies" };

export default async function ProxiesPage() {
  const user = await getCurrentUser();
  const [proxies, summary] = await Promise.all([getProxies(), getSummary()]);

  return (
    <>
      <header>
        <h1 className="text-2xl font-semibold tracking-tight">Proxies</h1>
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
