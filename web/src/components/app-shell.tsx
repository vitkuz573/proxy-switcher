import { Suspense } from "react";
import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarHeader,
  SidebarRail,
} from "@/components/ui/sidebar";
import { Skeleton } from "@/components/ui/skeleton";
import { AppNav } from "@/components/app-nav";
import { SignOutButton } from "@/components/sign-out-button";
import { getCurrentUser } from "@/lib/auth";

/**
 * App chrome.
 *
 * Navigation is static and ships in the shell. Only the account footer reads
 * the session, so it streams behind a Suspense boundary — that is what lets
 * the rest of the page prerender instead of blocking on every navigation.
 */
export function AppShell() {
  return (
    <Sidebar collapsible="icon">
      <SidebarHeader>
        <div className="flex items-center gap-2 px-2 py-1.5">
          <span className="size-5 rounded-md bg-primary" />
          <span className="truncate font-semibold group-data-[collapsible=icon]:hidden">
            Proxy Switcher
          </span>
        </div>
      </SidebarHeader>

      <SidebarContent>
        <AppNav />
      </SidebarContent>

      <SidebarFooter>
        <Suspense fallback={<FooterSkeleton />}>
          <AccountFooter />
        </Suspense>
      </SidebarFooter>
      <SidebarRail />
    </Sidebar>
  );
}

function FooterSkeleton() {
  return (
    <div className="flex items-center gap-2 px-2">
      <Skeleton className="size-6 rounded-md" />
      <Skeleton className="h-4 flex-1" />
    </div>
  );
}

async function AccountFooter() {
  const user = await getCurrentUser();

  return (
    <div className="flex items-center justify-between gap-2 px-2 text-xs text-muted-foreground group-data-[collapsible=icon]:hidden">
      <div className="min-w-0">
        <p className="truncate font-medium text-foreground">{user.email}</p>
        <p className="truncate">{user.role}</p>
      </div>
      <SignOutButton />
    </div>
  );
}