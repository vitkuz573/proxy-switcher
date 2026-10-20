"use client";

import type { LocalProxy, ProxyCredential, ProxyPolicy } from "@proxy-switcher/sdk";
import { KeyRound, Plus, RefreshCw, Trash2 } from "lucide-react";
import { useTranslations } from "next-intl";
import { useState, useTransition } from "react";
import { toast } from "sonner";
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
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
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

/**
 * The local proxy, its policies and its credentials.
 *
 * The password of a credential is shown once, when it is created. The engine
 * keeps a digest of it, so it cannot be shown again — and the panel says so
 * rather than leaving an empty field behind.
 */
export function LocalProxyPanel({
  state,
  policies,
  credentials,
  canWrite,
}: {
  state: LocalProxy | null;
  policies: ProxyPolicy[];
  credentials: ProxyCredential[];
  canWrite: boolean;
}) {
  const t = useTranslations("Proxy");
  const tCommon = useTranslations("Common");
  const router = useRouter();
  const [, startTransition] = useTransition();

  const [open, setOpen] = useState(false);
  const [username, setUsername] = useState("");
  const [policy, setPolicy] = useState(policies[0]?.name ?? "default");
  const [pending, setPending] = useState(false);
  const [created, setCreated] = useState<ProxyCredential | null>(null);

  async function onCreate(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setPending(true);

    try {
      const credential = await api.createCredential({ username, policy });
      setCreated(credential);
      setUsername("");
      toast.success(t("created", { name: credential.username }));
      startTransition(() => router.refresh());
    } catch (error) {
      toast.error(error instanceof Error ? error.message : tCommon("error"));
    } finally {
      setPending(false);
    }
  }

  async function onRotate(name: string) {
    try {
      const rotation = await api.rotateCredential(name);
      if (rotation.rotated) {
        toast.success(t("rotated", { name }));
      } else {
        toast.message(t("rotateNothing", { name }));
      }
      startTransition(() => router.refresh());
    } catch (error) {
      toast.error(error instanceof Error ? error.message : tCommon("error"));
    }
  }

  async function onDelete(name: string) {
    try {
      await api.deleteCredential(name);
      toast.success(t("removed", { name }));
      startTransition(() => router.refresh());
    } catch (error) {
      toast.error(error instanceof Error ? error.message : tCommon("error"));
    }
  }

  return (
    <div className="grid gap-4">
      <Card>
        <CardContent className="grid gap-3">
          <div className="flex items-center justify-between text-sm">
            <span className="text-muted-foreground">{t("address")}</span>
            <Badge variant={state?.enabled ? "default" : "outline"}>
              {state?.enabled ? state.address : t("off")}
            </Badge>
          </div>

          {state?.enabled ? (
            <>
              <p className="text-xs text-muted-foreground">{t("hint")}</p>
              <code className="block overflow-x-auto rounded-md bg-muted px-3 py-2 text-xs">
                http://name:password@{state.address.replace("http://", "")}
              </code>
              <dl className="grid gap-3 sm:grid-cols-4">
                <Measure label={t("connections")} value={state.connections} />
                <Measure label={t("tunnels")} value={state.tunnels} />
                <Measure label={t("sessions")} value={state.sessions} />
                <Measure label={t("rotations")} value={state.rotations} />
              </dl>
            </>
          ) : (
            <p className="text-xs text-muted-foreground">{t("offHint")}</p>
          )}
        </CardContent>
      </Card>

      <Card>
        <CardContent className="grid gap-3">
          <div className="flex items-center justify-between">
            <span className="text-sm text-muted-foreground">{t("policies")}</span>
          </div>

          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>{t("policy")}</TableHead>
                <TableHead>{t("rotation")}</TableHead>
                <TableHead>{t("countries")}</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {policies.map((item) => (
                <TableRow key={item.name}>
                  <TableCell className="font-medium">
                    {item.name}
                    {item.builtin ? (
                      <Badge variant="outline" className="ml-2">
                        {t("builtin")}
                      </Badge>
                    ) : null}
                  </TableCell>
                  <TableCell>{item.settings.rotation}</TableCell>
                  <TableCell>{item.settings.countries.join(", ") || "—"}</TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </CardContent>
      </Card>

      <Card>
        <CardContent className="grid gap-3">
          <div className="flex items-center justify-between">
            <span className="text-sm text-muted-foreground">{t("credentials")}</span>
            {canWrite ? (
              <Button size="sm" onClick={() => setOpen(true)}>
                <Plus className="size-4" />
                {t("create")}
              </Button>
            ) : null}
          </div>

          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>{t("name")}</TableHead>
                <TableHead>{t("policy")}</TableHead>
                <TableHead>{t("created")}</TableHead>
                <TableHead className="w-10" />
              </TableRow>
            </TableHeader>
            <TableBody>
              {credentials.map((credential) => (
                <TableRow key={credential.username}>
                  <TableCell className="font-medium">
                    <span className="inline-flex items-center gap-1.5">
                      <KeyRound className="size-3.5" />
                      {credential.username}
                    </span>
                  </TableCell>
                  <TableCell>{credential.policy}</TableCell>
                  <TableCell>{new Date(credential.created_at).toLocaleString()}</TableCell>
                  <TableCell>
                    {canWrite ? (
                      <div className="flex items-center justify-end gap-1">
                        <Button
                          variant="ghost"
                          size="icon-sm"
                          aria-label={t("rotate")}
                          onClick={() => onRotate(credential.username)}
                        >
                          <RefreshCw className="size-4" />
                        </Button>
                        <AlertDialog>
                          <AlertDialogTrigger asChild>
                            <Button variant="ghost" size="icon-sm" aria-label={t("remove")}>
                              <Trash2 className="size-4" />
                            </Button>
                          </AlertDialogTrigger>
                          <AlertDialogContent>
                            <AlertDialogHeader>
                              <AlertDialogTitle>{t("removeTitle")}</AlertDialogTitle>
                              <AlertDialogDescription>
                                {t("removeDescription", { name: credential.username })}
                              </AlertDialogDescription>
                            </AlertDialogHeader>
                            <AlertDialogFooter>
                              <AlertDialogCancel>{tCommon("cancel")}</AlertDialogCancel>
                              <AlertDialogAction onClick={() => onDelete(credential.username)}>
                                {t("remove")}
                              </AlertDialogAction>
                            </AlertDialogFooter>
                          </AlertDialogContent>
                        </AlertDialog>
                      </div>
                    ) : null}
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </CardContent>
      </Card>

      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{t("create")}</DialogTitle>
            <DialogDescription>{t("createDescription")}</DialogDescription>
          </DialogHeader>

          <form onSubmit={onCreate} className="grid gap-3">
            <div className="grid gap-1.5">
              <Label htmlFor="credential-name">{t("name")}</Label>
              <Input
                id="credential-name"
                value={username}
                onChange={(event) => setUsername(event.target.value)}
                required
              />
            </div>

            <div className="grid gap-1.5">
              <Label htmlFor="credential-policy">{t("policy")}</Label>
              <Select value={policy} onValueChange={setPolicy}>
                <SelectTrigger id="credential-policy">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {policies.map((item) => (
                    <SelectItem key={item.name} value={item.name}>
                      {item.name}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>

            <DialogFooter>
              <Button type="submit" disabled={pending || username.trim().length === 0}>
                {t("create")}
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>

      <Dialog open={created !== null} onOpenChange={() => setCreated(null)}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{t("createdTitle")}</DialogTitle>
            <DialogDescription>{t("createdDescription")}</DialogDescription>
          </DialogHeader>

          <code className="block overflow-x-auto rounded-md bg-muted px-3 py-2 text-xs">
            http://{created?.username}:{created?.password}@127.0.0.1:9098
          </code>

          <DialogFooter>
            <Button onClick={() => setCreated(null)}>{tCommon("close")}</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}

/** A number with a label. */
function Measure({ label, value }: { label: string; value: number }) {
  return (
    <div className="grid gap-0.5">
      <dt className="text-xs text-muted-foreground">{label}</dt>
      <dd className="text-xl font-semibold tabular-nums">{value.toLocaleString()}</dd>
    </div>
  );
}
