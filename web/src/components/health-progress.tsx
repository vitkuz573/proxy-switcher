"use client";

import type { Problem, ScrapeRun } from "@proxy-switcher/sdk";
import { RefreshCw } from "lucide-react";
import { useEffect, useState } from "react";
import { toast } from "sonner";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Progress } from "@/components/ui/progress";
import { Separator } from "@/components/ui/separator";

const IDLE: ScrapeRun = {
  id: "run_current",
  state: "idle",
  started_at: null,
  finished_at: null,
  proxies_found: 0,
  healthy_count: 0,
  health_check: null,
  errors: [],
};

/**
 * Polls the current run while a scrape or health check is in flight and stops as
 * soon as it settles, so an idle console does not hold a request open every few
 * seconds.
 */
export function HealthProgress({ initial }: { initial: ScrapeRun }) {
  const [run, setRun] = useState(initial);
  const [busy, setBusy] = useState(false);

  const active = run.state !== "idle" || run.health_check !== null;

  useEffect(() => {
    if (!active) {
      return;
    }

    let cancelled = false;

    const tick = async () => {
      try {
        const response = await fetch(`/api/v1/scrape-runs/current`, {
          credentials: "include",
          cache: "no-store",
        });
        if (response.ok && !cancelled) {
          setRun((await response.json()) as ScrapeRun);
        }
      } catch {
        // A dropped poll is not worth surfacing; the next tick retries.
      }
    };

    const timer = setInterval(tick, 2000);
    return () => {
      cancelled = true;
      clearInterval(timer);
    };
  }, [active]);

  async function trigger() {
    setBusy(true);
    try {
      const response = await fetch(`/api/v1/scrape-runs`, {
        method: "POST",
        credentials: "include",
      });
      if (!response.ok) {
        // Every failure is an RFC 9457 problem document.
        const problem = (await response.json().catch(() => null)) as Problem | null;
        throw new Error(problem?.detail ?? `Failed with ${response.status}`);
      }
      toast.success("Scrape started");
      setRun((prev) => ({ ...prev, state: "running" }));
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "Could not trigger a scrape");
    } finally {
      setBusy(false);
    }
  }

  const progress = run.health_check;
  const percent =
    progress && progress.total > 0 ? Math.round((progress.checked / progress.total) * 100) : 0;

  return (
    <Card>
      <CardHeader className="flex flex-row items-center justify-between gap-3 space-y-0">
        <div>
          <CardTitle className="text-base">Scraper</CardTitle>
          <p className="text-sm text-muted-foreground">
            {run.started_at ? `Last run ${new Date(run.started_at).toLocaleString()}` : "Never run"}
          </p>
        </div>
        <div className="flex items-center gap-2">
          {active ? (
            <Badge variant="secondary">{run.state}</Badge>
          ) : (
            <Badge variant="outline">idle</Badge>
          )}
          <Button size="sm" onClick={trigger} disabled={busy || active}>
            <RefreshCw className={busy ? "animate-spin" : undefined} />
            Scrape now
          </Button>
        </div>
      </CardHeader>

      <CardContent className="grid gap-4">
        <div className="grid grid-cols-2 gap-4 sm:grid-cols-3">
          <Metric label="Proxies found" value={run.proxies_found} />
          <Metric label="Healthy" value={run.healthy_count} />
          <Metric
            label="Errors"
            value={run.errors.length}
            className={run.errors.length > 0 ? "text-destructive" : undefined}
          />
        </div>

        {progress ? (
          <div className="grid gap-2">
            <div className="flex items-center justify-between text-sm">
              <span className="text-muted-foreground">Health check</span>
              <span className="tabular-nums">
                {progress.checked.toLocaleString()} / {progress.total.toLocaleString()} ({percent}%)
              </span>
            </div>
            <Progress value={percent} />
          </div>
        ) : null}

        {run.errors.length > 0 ? (
          <>
            <Separator />
            <ul className="grid gap-1 text-sm text-destructive">
              {run.errors.slice(0, 5).map((message) => (
                <li key={message} className="truncate">
                  {message}
                </li>
              ))}
            </ul>
          </>
        ) : null}
      </CardContent>
    </Card>
  );
}

function Metric({ label, value, className }: { label: string; value: number; className?: string }) {
  return (
    <div className="rounded-lg border p-3">
      <p className="text-xs uppercase tracking-wide text-muted-foreground">{label}</p>
      <p className={`text-2xl font-semibold tabular-nums ${className ?? ""}`}>
        {value.toLocaleString()}
      </p>
    </div>
  );
}

export { IDLE as IDLE_SCRAPE_RUN };
