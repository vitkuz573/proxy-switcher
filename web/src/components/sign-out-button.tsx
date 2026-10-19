"use client";

import { LogOut } from "lucide-react";
import { useRouter } from "next/navigation";
import { Button } from "@/components/ui/button";

/**
 * Sign-out is a mutation, so it runs in the browser against the same-origin
 * proxy. The session cookie is HttpOnly — the API clears it.
 */
export function SignOutButton() {
  const router = useRouter();

  async function signOut() {
    await fetch("/api/v1/sessions/current", { method: "DELETE", credentials: "include" });
    router.push("/login");
    router.refresh();
  }

  return (
    <Button variant="ghost" size="icon-sm" onClick={signOut} title="Sign out">
      <LogOut />
      <span className="sr-only">Sign out</span>
    </Button>
  );
}
