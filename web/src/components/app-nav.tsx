"use client";

import { BarChart3, Database, Globe, LayoutDashboard, Network, ScrollText } from "lucide-react";
import { useTranslations } from "next-intl";
import {
  SidebarGroup,
  SidebarGroupContent,
  SidebarGroupLabel,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
} from "@/components/ui/sidebar";
import { Link, usePathname } from "@/i18n/navigation";

const NAV = [
  { title: "dashboard", href: "/dashboard", icon: LayoutDashboard },
  { title: "proxies", href: "/proxies", icon: Globe },
  { title: "proxy", href: "/proxy", icon: Network },
  { title: "scraper", href: "/scraper", icon: Database },
  { title: "statistics", href: "/statistics", icon: BarChart3 },
  { title: "audit", href: "/audit", icon: ScrollText },
] as const;

export function AppNav() {
  const t = useTranslations("Nav");
  const pathname = usePathname();

  return (
    <SidebarGroup>
      <SidebarGroupLabel>{t("group")}</SidebarGroupLabel>
      <SidebarGroupContent>
        <SidebarMenu>
          {NAV.map((item) => (
            <SidebarMenuItem key={item.href}>
              <SidebarMenuButton
                asChild
                isActive={pathname.startsWith(item.href)}
                tooltip={t(item.title)}
              >
                <Link href={item.href}>
                  <item.icon />
                  <span>{t(item.title)}</span>
                </Link>
              </SidebarMenuButton>
            </SidebarMenuItem>
          ))}
        </SidebarMenu>
      </SidebarGroupContent>
    </SidebarGroup>
  );
}
