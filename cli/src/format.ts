const DIM = "\x1b[2m";
const BOLD = "\x1b[1m";
const GREEN = "\x1b[32m";
const RED = "\x1b[31m";
const YELLOW = "\x1b[33m";
const RESET = "\x1b[0m";

const colour = process.stdout.isTTY ?? process.env.FORCE_COLOR === "1";

function paint(code: string, text: string): string {
  return colour ? `${code}${text}${RESET}` : text;
}

export const bold = (text: string): string => paint(BOLD, text);
export const dim = (text: string): string => paint(DIM, text);
export const green = (text: string): string => paint(GREEN, text);
export const red = (text: string): string => paint(RED, text);
export const yellow = (text: string): string => paint(YELLOW, text);

export function heading(text: string): void {
  console.log(`\n${bold(text)}\n${dim("─".repeat(text.length))}`);
}

export function keyValue(label: string, value: string | number | null): void {
  const rendered = value === null ? dim("—") : String(value);
  console.log(`  ${label.padEnd(22)} ${rendered}`);
}

/** Truncate for columnar output so wide tables stay readable. */
export function truncate(text: string, width: number): string {
  return text.length <= width ? text.padEnd(width) : `${text.slice(0, width - 1)}…`;
}

export function table(headers: string[], rows: string[][]): void {
  if (rows.length === 0) {
    console.log(dim("  (nothing to show)"));
    return;
  }

  const widths = headers.map((header, i) =>
    Math.max(header.length, ...rows.map((row) => (row[i] ?? "").length)),
  );

  console.log(`  ${headers.map((header, i) => bold(header.padEnd(widths[i] ?? 0))).join("  ")}`);
  console.log(`  ${dim(widths.map((w) => "─".repeat(w)).join("  "))}`);

  for (const row of rows) {
    console.log(`  ${row.map((cell, i) => (cell ?? "").padEnd(widths[i] ?? 0)).join("  ")}`);
  }
}
