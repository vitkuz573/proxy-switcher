"use client";

import type { ProxyRecord } from "@proxy-switcher/sdk";
import { MoreHorizontal, Plus, RefreshCw, Search, Trash2 } from "lucide-react";
import { useTranslations } from "next-intl";
import { useEffect, useMemo, useState, useTransition } from "react";
import { toast } from "sonner";
import { AddProxyDialog } from "@/components/add-proxy-dialog";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from "@/components/ui/alert-dialog";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { useRouter } from "@/i18n/navigation";
import { api } from "@/lib/client";

type SortKey = "score" | "latency" | "host";

export function ProxiesTable({
  initial,
  activeId,
  canWrite,
}: {
  initial: ProxyRecord[];
  activeId: string | null;
  canWrite: boolean;
}) {
  const t = useTranslations("Proxies");
  const router = useRouter();
  const [proxies, setProxies] = useState(initial);
  const [query, setQuery] = useState("");
  const [sort, setSort] = useState<SortKey>("score");
  const [addOpen, setAddOpen] = useState(false);
  const [pending, startTransition] = useTransition();

  // Adopt the fresh pool after router.refresh(). Keyed on the server payload so
  // an optimistic delete stays visible until the next real refresh.
  useEffect(() => {
    setProxies(initial);
  }, [initial]);

  const visible = useMemo(() => {
    const needle = query.trim().toLowerCase();
    const filtered = needle
      ? proxies.filter(
          (p) =>
            p.host.toLowerCase().includes(needle) ||
            String(p.port).includes(needle) ||
            p.protocol.includes(needle) ||
            (p.country ?? "").toLowerCase().includes(needle),
        )
      : proxies;

    return [...filtered].sort((a, b) => {
      if (sort === "host") {
        return a.host.localeCompare(b.host);
      }
      if (sort === "latency") {
        return (
          (a.latency_ms ?? Number.MAX_SAFE_INTEGER) - (b.latency_ms ?? Number.MAX_SAFE_INTEGER)
        );
      }
      return b.score - a.score;
    });
  }, [proxies, query, sort]);

  function refresh() {
    startTransition(() => router.refresh());
  }

  async function onSwitch(id: string, host: string) {
    try {
      await api.setActiveProxy(id);
      toast.success(t("switchedTo", { host }));
      refresh();
    } catch (err) {
      toast.error(err instanceof Error ? err.message : t("switchFailed"));
    }
  }

  async function onDelete(id: string) {
    try {
      await api.deleteProxy(id);
      setProxies((prev) => prev.filter((p) => p.id !== id));
      toast.success(t("removed"));
      refresh();
    } catch (err) {
      toast.error(err instanceof Error ? err.message : t("removeFailed"));
    }
  }

  async function onRotate() {
    try {
      await api.rotateActiveProxy();
      toast.success(t("rotated"));
      refresh();
    } catch (err) {
      toast.error(err instanceof Error ? err.message : t("rotateFailed"));
    }
  }

  return (
    <div className="grid gap-4">
      <div className="flex flex-wrap items-center gap-2">
        <div className="relative min-w-56 flex-1">
          <Search className="pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted-foreground" />
          <Input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder={t("filter")}
            className="pl-8"
            aria-label={t("filterLabel")}
          />
        </div>

        <div className="flex gap-1 rounded-lg border p-0.5">
          {(["score", "latency", "host"] as const).map((key) => (
            <Button
              key={key}
              size="sm"
              variant={sort === key ? "secondary" : "ghost"}
              onClick={() => setSort(key)}
            >
              {t(`sort.${key}`)}
            </Button>
          ))}
        </div>

        {canWrite ? (
          <>
            <Button variant="outline" onClick={onRotate} disabled={pending}>
              <RefreshCw />
              Rotate
            </Button>
            <Button onClick={() => setAddOpen(true)}>
              <Plus />
              Add proxy
            </Button>
          </>
        ) : null}
      </div>

      <Card>
        <CardContent className="px-0">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>{t("columns.endpoint")}</TableHead>
                <TableHead>{t("columns.protocol")}</TableHead>
                <TableHead>{t("columns.anonymity")}</TableHead>
                <TableHead className="text-right">{t("columns.latency")}</TableHead>
                <TableHead className="text-right">{t("columns.score")}</TableHead>
                <TableHead>{t("columns.egress")}</TableHead>
                <TableHead>{t("columns.checked")}</TableHead>
                <TableHead className="w-12" />
              </TableRow>
            </TableHeader>
            <TableBody>
              {visible.map((proxy) => (
                <TableRow
                  key={proxy.id}
                  className={proxy.id === activeId ? "bg-primary/5" : undefined}
                >
                  <TableCell className="font-mono text-xs">
                    {proxy.host}:{proxy.port}
                    {proxy.country ? (
                      <span className="ml-2 font-sans text-muted-foreground">{proxy.country}</span>
                    ) : null}
                    {proxy.id === activeId ? <Badge className="ml-2">active</Badge> : null}
                  </TableCell>
                  <TableCell>
                    <Badge variant="outline">{proxy.protocol}</Badge>
                  </TableCell>
                  <TableCell className="text-muted-foreground">{proxy.anonymity}</TableCell>
                  <TableCell className="text-right tabular-nums">
                    {proxy.latency_ms === null ? "—" : `${proxy.latency_ms} ms`}
                  </TableCell>
                  <TableCell className="text-right tabular-nums">
                    {proxy.score.toFixed(1)}
                  </TableCell>
                  <TableCell className="text-xs text-muted-foreground">
                    {proxy.last_checked ? new Date(proxy.last_checked).toLocaleTimeString() : "—"}
                  </TableCell>
                  <TableCell>
                    {canWrite ? (
                      <DropdownMenu>
                        <DropdownMenuTrigger asChild>
                          <Button variant="ghost" size="icon-sm" aria-label={t("columns.actions")}>
                            <MoreHorizontal />
                          </Button>
                        </DropdownMenuTrigger>
                        <DropdownMenuContent align="end">
                          <DropdownMenuItem
                            onClick={() => onSwitch(proxy.id, `${proxy.host}:${proxy.port}`)}
                            disabled={proxy.id === activeId}
                          >
                            {t("switchTo")}
                          </DropdownMenuItem>
                          <DeleteProxyDialog
                            endpoint={`${proxy.host}:${proxy.port}`}
                            onConfirm={() => onDelete(proxy.id)}
                          />
                        </DropdownMenuContent>
                      </DropdownMenu>
                    ) : null}
                  </TableCell>
                </TableRow>
              ))}

              {visible.length === 0 ? (
                <TableRow>
                  <TableCell colSpan={8} className="h-24 text-center text-muted-foreground">
                    {proxies.length === 0 ? t("empty") : t("noMatch")}
                  </TableCell>
                </TableRow>
              ) : null}
            </TableBody>
          </Table>
        </CardContent>
      </Card>

      <AddProxyDialog
        open={addOpen}
        onOpenChange={setAddOpen}
        onCreated={() => {
          setAddOpen(false);
          refresh();
        }}
      />
    </div>
  );
}

function DeleteProxyDialog({ endpoint, onConfirm }: { endpoint: string; onConfirm: () => void }) {
  const t = useTranslations("Proxies");
  const tCommon = useTranslations("Common");

  return (
    <AlertDialog>
      <AlertDialogTrigger asChild>
        <DropdownMenuItem onSelect={(event) => event.preventDefault()} className="text-destructive">
          <Trash2 />
          {t("removeAction")}
        </DropdownMenuItem>
      </AlertDialogTrigger>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>{t("removeTitle", { endpoint })}</AlertDialogTitle>
          <AlertDialogDescription>{t("removeDescription")}</AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>{tCommon("actions.cancel")}</AlertDialogCancel>
          <AlertDialogAction onClick={onConfirm}>{t("removeAction")}</AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
