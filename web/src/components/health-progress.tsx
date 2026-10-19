"use client";

import { useEffect, useState } from "react";
import { toast } from "sonner";
import { RefreshCw } from "lucide-react";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Progress } from "@/components/ui/progress";
import { Separator } from "@/components/ui/separator";

export interface ScrapeState {
  running: boolean;
  checking_progress: [number, number] | null;
  proxies_found: number;
  healthy_count: number;
  last_run: string | null;
  errors: string[];
}

/**
 * Polls scrape status while a health check is running and stops as soon as it
 * finishes, so an idle console does not hold a request open every few seconds.
 */
export function HealthProgress({ initial }: { initial: ScrapeState }) {
  const [state, setState] = useState(initial);
  const [busy, setBusy] = useState(false);

  const active = state.running || state.checking_progress !== null;

  useEffect(() => {
    if (!active) {
      return;
    }

    let cancelled = false;

    const tick = async () => {
      try {
        const response = await fetch(`/api/v1/scrape/status`, {
          credentials: "include",
          cache: "no-store",
        });
        if (response.ok && !cancelled) {
          setState((await response.json()) as ScrapeState);
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
      const response = await fetch(`/api/v1/scrape`, {
        method: "POST",
        credentials: "include",
      });
      if (!response.ok) {
        const body = (await response.json().catch(() => null)) as { message?: string } | null;
        throw new Error(body?.message ?? `Failed with ${response.status}`);
      }
      toast.success("Scrape queued");
      setState((prev) => ({ ...prev, running: true }));
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "Could not trigger a scrape");
    } finally {
      setBusy(false);
    }
  }

  const [checked, total] = state.checking_progress ?? [0, 0];
  const percent = total > 0 ? Math.round((checked / total) * 100) : 0;

  return (
    <Card>
      <CardHeader className="flex flex-row items-center justify-between gap-3 space-y-0">
        <div>
          <CardTitle className="text-base">Scraper</CardTitle>
          <p className="text-sm text-muted-foreground">
            {state.last_run
              ? `Last run ${new Date(state.last_run).toLocaleString()}`
              : "Never run"}
          </p>
        </div>
        <div className="flex items-center gap-2">
          {active ? (
            <Badge variant="secondary">running</Badge>
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
          <Metric label="Proxies found" value={state.proxies_found} />
          <Metric label="Healthy" value={state.healthy_count} />
          <Metric
            label="Errors"
            value={state.errors.length}
            className={state.errors.length > 0 ? "text-destructive" : undefined}
          />
        </div>

        {state.checking_progress ? (
          <div className="grid gap-2">
            <div className="flex items-center justify-between text-sm">
              <span className="text-muted-foreground">Health check</span>
              <span className="tabular-nums">
                {checked.toLocaleString()} / {total.toLocaleString()} ({percent}%)
              </span>
            </div>
            <Progress value={percent} />
          </div>
        ) : null}

        {state.errors.length > 0 ? (
          <>
            <Separator />
            <ul className="grid gap-1 text-sm text-destructive">
              {state.errors.slice(0, 5).map((message, index) => (
                <li key={index} className="truncate">
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

function Metric({
  label,
  value,
  className,
}: {
  label: string;
  value: number;
  className?: string;
}) {
  return (
    <div className="rounded-lg border p-3">
      <p className="text-xs uppercase tracking-wide text-muted-foreground">{label}</p>
      <p className={`text-2xl font-semibold tabular-nums ${className ?? ""}`}>
        {value.toLocaleString()}
      </p>
    </div>
  );
}