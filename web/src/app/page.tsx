import { redirect } from "next/navigation";

/**
 * Sends visitors to the sign-in form or the console. `proxy.ts` already routes
 * unauthenticated traffic, so this only settles the destination.
 */
export default function Home() {
  redirect("/dashboard");
}
