/**
 * The root layout.
 *
 * Everything the visitor sees lives under `[locale]`, and that segment carries
 * the document with the right language. This file exists because Next.js needs
 * a root layout, and it deliberately renders nothing of its own: a second
 * `<html>` element here would nest inside the one the locale layout emits.
 */
export default function RootLayout({ children }: LayoutProps<"/">) {
  return children;
}
