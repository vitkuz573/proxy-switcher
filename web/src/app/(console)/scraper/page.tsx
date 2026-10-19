import { AlertTriangle } from "lucide-react";
import { HealthProgress, IDLE_SCRAPE_RUN } from "@/components/health-progress";
import { SourcesPanel } from "@/components/sources-panel";
import { getScrapeRun, getSources } from "@/lib/api";
import { getCurrentUser } from "@/lib/auth";

export const metadata = { title: "Scraper" };

export default async function ScraperPage() {
  const user = await getCurrentUser();
  const [scrape, sources] = await Promise.all([getScrapeRun(), getSources()]);

  const run = scrape ?? IDLE_SCRAPE_RUN;

  return (
    <>
      <header>
        <h1 className="text-2xl font-semibold tracking-tight">Scraper</h1>
        <p className="text-sm text-muted-foreground">
          Discovery and health checking of the proxy pool.
        </p>
      </header>

      {run.errors.length > 0 ? (
        <div className="flex items-start gap-3 rounded-lg border border-destructive/40 bg-destructive/5 p-4">
          <AlertTriangle className="mt-0.5 size-4 shrink-0 text-destructive" />
          <div className="grid gap-1 text-sm">
            <p className="font-medium text-destructive">Last cycle reported errors</p>
            <ul className="text-muted-foreground">
              {run.errors.map((message) => (
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
