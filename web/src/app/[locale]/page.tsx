import { setRequestLocale } from "next-intl/server";
import { redirect } from "@/i18n/navigation";

type Props = { params: Promise<{ locale: string }> };

/**
 * Sends visitors to the sign-in form or the console. `proxy.ts` already routes
 * unauthenticated traffic, so this only settles the destination.
 */
export default async function Home({ params }: Props) {
  const { locale } = await params;
  setRequestLocale(locale);

  redirect({ href: "/dashboard", locale });
}
