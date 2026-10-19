"use client";

import { useLocale, useTranslations } from "next-intl";

/**
 * The flag of a country, as an emoji.
 *
 * A flag is two regional indicator symbols, one per letter of the code, so it
 * is derived rather than looked up: there is no table to keep in step with
 * the codes, and a country the code list has never heard of still gets a flag.
 */
export function countryFlag(code: string | null | undefined): string {
  if (!code) {
    return "";
  }

  const trimmed = code.trim().toUpperCase();
  if (trimmed.length !== 2 || !/^[A-Z]{2}$/.test(trimmed)) {
    return "";
  }

  return String.fromCodePoint(...[...trimmed].map((letter) => 0x1f1e6 + letter.charCodeAt(0) - 65));
}

/**
 * The name of a country, in the language of the reader.
 *
 * The platform carries the names of every country, in every language it
 * knows, so there is no list here to fall out of step with the codes.
 */
export function useCountryName() {
  const locale = useLocale();

  const display = new Intl.DisplayNames([locale], { type: "region" });

  return function countryName(code: string | null | undefined): string {
    if (!code) {
      return "—";
    }

    try {
      return display.of(code.toUpperCase()) ?? code.toUpperCase();
    } catch {
      // A code the platform does not know is still a code.
      return code.toUpperCase();
    }
  };
}

/** A country with its flag and its name. */
export function useCountryLabel() {
  const t = useTranslations("Common");
  const countryName = useCountryName();

  return function countryLabel(code: string | null | undefined): { flag: string; name: string } {
    if (!code) {
      return { flag: "", name: t("unknownCountry") };
    }
    return { flag: countryFlag(code), name: countryName(code) };
  };
}
