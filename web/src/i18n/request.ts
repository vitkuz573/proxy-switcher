import { hasLocale } from "next-intl";
import { getRequestConfig } from "next-intl/server";

import { routing } from "./routing";

export default getRequestConfig(async ({ requestLocale }) => {
  // The locale the proxy matched. It is already the one in the address, or
  // the one the visitor's preference implies.
  const requested = await requestLocale;
  const locale =
    requested && hasLocale(routing.locales, requested) ? requested : routing.defaultLocale;

  return {
    locale,
    // The catalogue for the language, loaded only for the one in use.
    messages: (await import(`../../messages/${locale}.json`)).default,
  };
});
