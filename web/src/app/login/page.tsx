import { LoginForm } from "@/components/login-form";

export const metadata = { title: "Sign in" };

/**
 * Fully static. `proxy.ts` already bounces signed-in visitors to the dashboard,
 * so the page itself never needs to read the session.
 */
export default function LoginPage() {
  return (
    <main className="flex flex-1 items-center justify-center p-6">
      <LoginForm />
    </main>
  );
}