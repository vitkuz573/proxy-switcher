import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

/** The languages the tool speaks. */
export const LOCALES = ["en", "ru"] as const;
export type Locale = (typeof LOCALES)[number];

/** The language used when nothing else says. */
export const DEFAULT_LOCALE: Locale = "en";

type Catalogue = Record<string, unknown>;

const catalogues = new Map<Locale, Catalogue>();

function load(locale: Locale): Catalogue {
  const cached = catalogues.get(locale);
  if (cached) {
    return cached;
  }

  // The catalogues sit next to the compiled code, so they travel with the
  // build rather than being looked up in the source tree.
  const here = dirname(fileURLToPath(import.meta.url));
  const candidates = [
    join(here, "..", "messages", `${locale}.json`),
    join(here, "messages", `${locale}.json`),
  ];

  const path = candidates.find((candidate) => existsSync(candidate));
  if (!path) {
    throw new Error(`the ${locale} catalogue is missing`);
  }

  const catalogue = JSON.parse(readFileSync(path, "utf8")) as Catalogue;
  catalogues.set(locale, catalogue);
  return catalogue;
}

/**
 * The language to speak.
 *
 * An explicit choice wins, then the environment, then English. The
 * environment is what a user sets once for the whole machine, and it is the
 * usual way a command line tool is told what language to speak.
 */
export function resolveLocale(explicit?: string): Locale {
  if (explicit && (LOCALES as readonly string[]).includes(explicit)) {
    return explicit as Locale;
  }

  for (const name of ["LC_ALL", "LC_MESSAGES", "LANG", "LANGUAGE"]) {
    const value = process.env[name];
    if (!value) {
      continue;
    }

    // LANGUAGE holds a colon-separated list of preferences.
    for (const candidate of value.split(":")) {
      const tag = candidate.split(".")[0]?.replace("_", "-").toLowerCase();
      if (!tag) {
        continue;
      }

      const exact = LOCALES.find((locale) => locale === tag);
      if (exact) {
        return exact;
      }

      // A regional variant falls back to its language.
      const language = tag.split("-")[0];
      const base = LOCALES.find((locale) => locale === language);
      if (base) {
        return base;
      }
    }
  }

  return DEFAULT_LOCALE;
}

/**
 * A translator for one language.
 *
 * The keys are dotted paths into the catalogue, so a message is addressed by
 * where it lives rather than by a constant: `t("Proxies.list.title")`.
 */
export function translator(locale: Locale) {
  const catalogue = load(locale);

  function lookup(path: string): unknown {
    let current: unknown = catalogue;

    for (const part of path.split(".")) {
      if (typeof current !== "object" || current === null) {
        return undefined;
      }
      current = (current as Record<string, unknown>)[part];
    }

    return current;
  }

  function interpolate(text: string, values: Record<string, string | number>): string {
    return text.replace(/\{(\w+)\}/g, (match, name: string) =>
      name in values ? String(values[name]) : match,
    );
  }

  function t(path: string, values: Record<string, string | number> = {}): string {
    const value = lookup(path);
    return typeof value === "string" ? interpolate(value, values) : path;
  }

  /** The message at a path, or nothing when the path is not in the catalogue. */
  t.maybe = function maybe(path: string): string | undefined {
    const value = lookup(path);
    return typeof value === "string" ? value : undefined;
  };

  /** A list of strings, for table headings. */
  t.list = function list(path: string): string[] {
    const value = lookup(path);
    return Array.isArray(value) ? value.map((entry) => String(entry)) : [path];
  };

  return t;
}

/**
 * The flag of a country, as an emoji.
 *
 * A flag is two regional indicator symbols, one per letter of the code, so it
 * is derived rather than looked up. Not every terminal has a font with the
 * flag glyphs, so a terminal that cannot show them is given the letters of the
 * code instead.
 */
export function countryFlag(code: string | null | undefined): string {
  if (!code) {
    return "";
  }

  const trimmed = code.trim().toUpperCase();
  if (trimmed.length !== 2 || !/^[A-Z]{2}$/.test(trimmed)) {
    return "";
  }

  if (!process.stdout.isTTY) {
    return trimmed;
  }

  return String.fromCodePoint(...[...trimmed].map((letter) => 0x1f1e6 + letter.charCodeAt(0) - 65));
}

/**
 * The name of a country, in the language of the reader.
 *
 * The platform carries the names of every country, in every language it
 * knows, so there is no list here to fall out of step with the codes.
 */
export function countryName(code: string | null | undefined, locale = "en"): string {
  if (!code) {
    return "—";
  }

  try {
    const display = new Intl.DisplayNames([locale], { type: "region" });
    return display.of(code.toUpperCase()) ?? code.toUpperCase();
  } catch {
    // A code the platform does not know is still a code.
    return code.toUpperCase();
  }
}

export type Translate = ReturnType<typeof translator>;
