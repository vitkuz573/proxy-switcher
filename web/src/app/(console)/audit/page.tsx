import { redirect } from "next/navigation";
import { Badge } from "@/components/ui/badge";
import { Card, CardContent } from "@/components/ui/card";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { getAuditEvents } from "@/lib/api";
import { getCurrentUser } from "@/lib/auth";

export const metadata = { title: "Audit" };

export default async function AuditPage() {
  const user = await getCurrentUser();

  // The API restricts /audit to admins; reflect that instead of rendering an
  // empty table that will 403 on the client.
  if (user.role !== "admin") {
    redirect("/dashboard");
  }

  const entries = await getAuditEvents(200);

  return (
    <>
      <header>
        <h1 className="text-2xl font-semibold tracking-tight">Audit</h1>
        <p className="text-sm text-muted-foreground">
          Every mutating action and denied request, newest first.
        </p>
      </header>

      <Card>
        <CardContent className="px-0">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>When</TableHead>
                <TableHead>Actor</TableHead>
                <TableHead>Action</TableHead>
                <TableHead>Target</TableHead>
                <TableHead>Outcome</TableHead>
                <TableHead>Detail</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {entries.map((entry) => (
                <TableRow key={entry.id}>
                  <TableCell className="whitespace-nowrap text-xs text-muted-foreground">
                    {new Date(entry.ts).toLocaleString()}
                  </TableCell>
                  <TableCell>
                    <Badge variant="outline">{entry.actor_type}</Badge>
                  </TableCell>
                  <TableCell className="font-mono text-xs">{entry.action}</TableCell>
                  <TableCell className="max-w-48 truncate font-mono text-xs text-muted-foreground">
                    {entry.target ?? "—"}
                  </TableCell>
                  <TableCell>
                    <Badge variant={entry.outcome === "allowed" ? "default" : "destructive"}>
                      {entry.outcome}
                    </Badge>
                  </TableCell>
                  <TableCell className="max-w-72 truncate text-xs text-muted-foreground">
                    {entry.detail ?? "—"}
                  </TableCell>
                </TableRow>
              ))}

              {entries.length === 0 ? (
                <TableRow>
                  <TableCell colSpan={6} className="h-24 text-center text-muted-foreground">
                    Nothing recorded yet.
                  </TableCell>
                </TableRow>
              ) : null}
            </TableBody>
          </Table>
        </CardContent>
      </Card>
    </>
  );
}
