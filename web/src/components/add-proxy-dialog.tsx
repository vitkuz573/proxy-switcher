"use client";

import type { Anonymity, ProxyProtocol } from "@proxy-switcher/sdk";
import { type FormEvent, useState } from "react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
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
import { api } from "@/lib/client";

const PROTOCOLS: ProxyProtocol[] = ["http", "https", "socks4", "socks5"];
const ANONYMITY: Anonymity[] = ["transparent", "anonymous", "elite", "unknown"];

export function AddProxyDialog({
  open,
  onOpenChange,
  onCreated,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onCreated: () => void;
}) {
  const [host, setHost] = useState("");
  const [port, setPort] = useState("");
  const [protocol, setProtocol] = useState<ProxyProtocol>("http");
  const [anonymity, setAnonymity] = useState<Anonymity>("unknown");
  const [country, setCountry] = useState("");
  const [pending, setPending] = useState(false);

  async function onSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const parsedPort = Number(port);

    if (!Number.isInteger(parsedPort) || parsedPort < 1 || parsedPort > 65535) {
      toast.error("Port must be between 1 and 65535");
      return;
    }

    setPending(true);
    try {
      const proxy = await api.createProxy({
        host: host.trim(),
        port: parsedPort,
        protocol,
        ...(country.trim() ? { country: country.trim() } : {}),
      });
      toast.success(`Added ${proxy.id}`);
      setHost("");
      setPort("");
      setCountry("");
      onCreated();
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "Could not add the proxy");
    } finally {
      setPending(false);
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Add a proxy</DialogTitle>
          <DialogDescription>
            The proxy joins the pool immediately and is health checked on the next cycle.
          </DialogDescription>
        </DialogHeader>

        <form onSubmit={onSubmit} className="grid gap-4">
          <div className="grid gap-2">
            <Label htmlFor="host">Host or IP</Label>
            <Input
              id="host"
              required
              placeholder="198.51.100.7"
              value={host}
              onChange={(e) => setHost(e.target.value)}
            />
          </div>

          <div className="grid grid-cols-2 gap-3">
            <div className="grid gap-2">
              <Label htmlFor="port">Port</Label>
              <Input
                id="port"
                required
                inputMode="numeric"
                placeholder="8080"
                value={port}
                onChange={(e) => setPort(e.target.value)}
              />
            </div>
            <div className="grid gap-2">
              <Label>Protocol</Label>
              <Select value={protocol} onValueChange={(v) => setProtocol(v as ProxyProtocol)}>
                <SelectTrigger>
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {PROTOCOLS.map((value) => (
                    <SelectItem key={value} value={value}>
                      {value}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
          </div>

          <div className="grid grid-cols-2 gap-3">
            <div className="grid gap-2">
              <Label>Anonymity</Label>
              <Select value={anonymity} onValueChange={(v) => setAnonymity(v as Anonymity)}>
                <SelectTrigger>
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {ANONYMITY.map((value) => (
                    <SelectItem key={value} value={value}>
                      {value}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
            <div className="grid gap-2">
              <Label htmlFor="country">Country</Label>
              <Input
                id="country"
                placeholder="optional"
                value={country}
                onChange={(e) => setCountry(e.target.value)}
              />
            </div>
          </div>

          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={pending}>
              {pending ? "Adding…" : "Add proxy"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
