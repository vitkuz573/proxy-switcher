import type { Metadata } from "next";
import { getTranslations, setRequestLocale } from "next-intl/server";
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
import { redirect } from "@/i18n/navigation";
import { getAuditEvents } from "@/lib/api";
import { getCurrentUser } from "@/lib/auth";

type Props = { params: Promise<{ locale: string }> };

export async function generateMetadata({ params }: Props): Promise<Metadata> {
  const { locale } = await params;
  const t = await getTranslations({ locale, namespace: "Audit" });

  return { title: t("title") };
}

export default async function AuditPage({ params }: Props) {
  const { locale } = await params;
  setRequestLocale(locale);
  const t = await getTranslations("Audit");

  const user = await getCurrentUser();

  // The API restricts /audit to admins; reflect that instead of rendering an
  // empty table that will 403 on the client.
  if (user.role !== "admin") {
    redirect({ href: "/dashboard", locale });
  }

  const entries = await getAuditEvents(200);

  return (
    <>
      <header>
        <h1 className="text-2xl font-semibold tracking-tight">{t("title")}</h1>
        <p className="text-sm text-muted-foreground">{t("subtitle")}</p>
      </header>

      <Card>
        <CardContent className="px-0">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>{t("columns.when")}</TableHead>
                <TableHead>{t("columns.actor")}</TableHead>
                <TableHead>{t("columns.action")}</TableHead>
                <TableHead>{t("columns.target")}</TableHead>
                <TableHead>{t("columns.outcome")}</TableHead>
                <TableHead>{t("columns.detail")}</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {entries.map((entry) => (
                <TableRow key={entry.id}>
                  <TableCell className="whitespace-nowrap text-xs text-muted-foreground">
                    {new Date(entry.ts).toLocaleString(locale)}
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
                      {entry.outcome === "allowed" ? t("allowed") : t("denied")}
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
                    {t("empty")}
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
