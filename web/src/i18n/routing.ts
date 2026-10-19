import { defineRouting } from "next-intl/routing";

/**
 * The languages the console speaks.
 *
 * English is the default and is served from the root, so the common case needs
 * no prefix in the address and no cookie. Russian lives under its own prefix.
 */
export const routing = defineRouting({
  locales: ["en", "ru"],
  defaultLocale: "en",
  // English at the root, the others under their own prefix.
  localePrefix: "always",
});

export type Locale = (typeof routing.locales)[number];

/** The languages offered in the switcher, in the order they are shown. */
export const LOCALES: { code: Locale; label: string }[] = [
  { code: "en", label: "English" },
  { code: "ru", label: "Русский" },
];
