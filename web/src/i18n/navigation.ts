import { createNavigation } from "next-intl/navigation";

import { routing } from "./routing";

/**
 * Navigation that knows about the locales.
 *
 * A link written with these keeps the visitor in the language they chose,
 * and a redirect carries the prefix with it.
 */
export const { Link, redirect, usePathname, useRouter, getPathname } = createNavigation(routing);
