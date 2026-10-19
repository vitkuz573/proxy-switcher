"use client";

import type { Source } from "@proxy-switcher/sdk";
import { Plus, Trash2 } from "lucide-react";
import { useRouter } from "next/navigation";
import { type FormEvent, useState } from "react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { api } from "@/lib/client";

/**
 * Scrape sources are an admin-only surface: they decide what enters the pool.
 * The API enforces that too; hiding the control is only a convenience.
 */
export function SourcesPanel({ initial, canManage }: { initial: Source[]; canManage: boolean }) {
  const router = useRouter();
  const [sources, setSources] = useState(initial);
  const [url, setUrl] = useState("");
  const [pending, setPending] = useState(false);

  function refresh() {
    router.refresh();
  }

  async function onAdd(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const trimmed = url.trim();
    if (!trimmed) {
      return;
    }

    setPending(true);
    try {
      await api.createSource(trimmed);
      toast.success("Source added");
      setUrl("");
      refresh();
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "Could not add the source");
    } finally {
      setPending(false);
    }
  }

  async function onRemove(id: string) {
    const previous = sources;
    setSources((prev) => prev.filter((s) => s.id !== id));
    try {
      // Sources are addressed by their stable id; the URL is data, not identity.
      await api.deleteSource(id);
      toast.success("Source removed");
      refresh();
    } catch (err) {
      setSources(previous);
      toast.error(err instanceof Error ? err.message : "Could not remove the source");
    }
  }

  return (
    <Card>
      <CardHeader>
        <CardTitle className="text-base">Scrape sources</CardTitle>
      </CardHeader>
      <CardContent className="grid gap-4">
        {canManage ? (
          <form onSubmit={onAdd} className="flex gap-2">
            <Input
              value={url}
              onChange={(e) => setUrl(e.target.value)}
              placeholder="https://example.com/proxy-list.txt"
              aria-label="Source URL"
            />
            <Button type="submit" disabled={pending}>
              <Plus />
              Add
            </Button>
          </form>
        ) : null}

        <ul className="divide-y rounded-lg border">
          {sources.map((source) => (
            <li key={source.id} className="flex items-center justify-between gap-3 px-3 py-2">
              <span className="truncate font-mono text-xs">{source.url}</span>
              {canManage ? (
                <Button
                  variant="ghost"
                  size="icon-sm"
                  onClick={() => onRemove(source.id)}
                  aria-label={`Remove ${source.url}`}
                >
                  <Trash2 />
                </Button>
              ) : null}
            </li>
          ))}

          {sources.length === 0 ? (
            <li className="px-3 py-6 text-center text-sm text-muted-foreground">
              No sources configured. The built-in defaults are used when the list is empty.
            </li>
          ) : null}
        </ul>
      </CardContent>
    </Card>
  );
}
