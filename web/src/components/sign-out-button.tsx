"use client";

import { LogOut } from "lucide-react";
import { useTranslations } from "next-intl";
import { Button } from "@/components/ui/button";
import { useRouter } from "@/i18n/navigation";

/**
 * Sign-out is a mutation, so it runs in the browser against the same-origin
 * proxy. The session cookie is HttpOnly — the API clears it.
 */
export function SignOutButton() {
  const t = useTranslations("Common");
  const router = useRouter();

  async function signOut() {
    await fetch("/api/v1/sessions/current", { method: "DELETE", credentials: "include" });
    router.push("/login");
    router.refresh();
  }

  return (
    <Button variant="ghost" size="icon-sm" onClick={signOut} title={t("actions.signOut")}>
      <LogOut />
      <span className="sr-only">{t("actions.signOut")}</span>
    </Button>
  );
}
