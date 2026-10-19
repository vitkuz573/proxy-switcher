import { setRequestLocale } from "next-intl/server";
import { Suspense } from "react";
import { AppShell } from "@/components/app-shell";
import { Separator } from "@/components/ui/separator";
import { SidebarInset, SidebarProvider } from "@/components/ui/sidebar";
import { Skeleton } from "@/components/ui/skeleton";

/**
 * The shell itself never reads the session, so it prerenders. Session checks
 * happen inside Suspense boundaries (see `lib/auth.ts`), which is what keeps
 * this layout static instead of blocking on every navigation.
 */
type Props = {
  children: React.ReactNode;
  params: Promise<{ locale: string }>;
};

export default async function ConsoleLayout({ children, params }: Props) {
  const { locale } = await params;
  setRequestLocale(locale);

  return (
    <SidebarProvider>
      <AppShell />
      <SidebarInset>
        <Separator className="mb-0" />
        <div className="flex flex-1 flex-col gap-6 p-6">
          <Suspense fallback={<PageSkeleton />}>{children}</Suspense>
        </div>
      </SidebarInset>
    </SidebarProvider>
  );
}

function PageSkeleton() {
  return (
    <div className="grid gap-4">
      <Skeleton className="h-8 w-48" />
      <div className="grid gap-4 sm:grid-cols-2 xl:grid-cols-4">
        <Skeleton className="h-24 rounded-xl" />
        <Skeleton className="h-24 rounded-xl" />
        <Skeleton className="h-24 rounded-xl" />
        <Skeleton className="h-24 rounded-xl" />
      </div>
      <Skeleton className="h-64 rounded-xl" />
    </div>
  );
}
