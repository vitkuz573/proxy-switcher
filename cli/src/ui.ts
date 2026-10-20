/**
 * The terminal interface.
 *
 * Everything the tool draws goes through here, so the look is one decision
 * rather than a decision per command. Three things matter for it to feel right
 * in a terminal:
 *
 * - colour only when the output is a terminal, and never when `NO_COLOR` says
 *   otherwise, so a pipe gets plain text;
 * - the layout follows the width of the terminal rather than a fixed guess;
 * - the widths are measured in what the reader sees, so a value padded to
 *   twenty-two columns is padded by the characters that are actually printed.
 */

/** The semantic palette. */
const CODES = {
  reset: "\x1b[0m",
  bold: "\x1b[1m",
  dim: "\x1b[2m",
  italic: "\x1b[3m",
  underline: "\x1b[4m",

  red: "\x1b[31m",
  green: "\x1b[32m",
  yellow: "\x1b[33m",
  blue: "\x1b[34m",
  magenta: "\x1b[35m",
  cyan: "\x1b[36m",
  grey: "\x1b[90m",
  brightGreen: "\x1b[92m",
  brightCyan: "\x1b[96m",
} as const;

export type Style = keyof typeof CODES;

/**
 * Whether the output can carry colour.
 *
 * A terminal can. A pipe cannot: the escape sequences would end up in the
 * file. `NO_COLOR` overrides everything, as it is meant to, and `FORCE_COLOR`
 * overrides the other way for the cases where colour is wanted anyway.
 */
function colourEnabled(): boolean {
  if (process.env.NO_COLOR !== undefined && process.env.NO_COLOR !== "") {
    return false;
  }
  if (process.env.FORCE_COLOR !== undefined && process.env.FORCE_COLOR !== "0") {
    return true;
  }
  return Boolean(process.stdout.isTTY);
}

const colour = colourEnabled();

/**
 * The escape sequences that carry a style.
 *
 * They do not occupy space, so they are not counted when a string is measured
 * or cut. Written out rather than spelled with a control character, so the
 * source stays readable.
 */
const ESCAPE_SEQUENCE = new RegExp(`${String.fromCharCode(27)}\\[[0-9;]*m`, "g");

/** Wrap the text in a style, when the output can carry it. */
export function paint(style: Style, text: string): string {
  if (!colour) {
    return text;
  }
  return `${CODES[style]}${text}${CODES.reset}`;
}

export const bold = (text: string) => paint("bold", text);
export const dim = (text: string) => paint("dim", text);
export const italic = (text: string) => paint("italic", text);
export const ok = (text: string) => paint("green", text);
export const warn = (text: string) => paint("yellow", text);
export const err = (text: string) => paint("red", text);
export const muted = (text: string) => paint("grey", text);
export const accent = (text: string) => paint("brightCyan", text);
export const heading = (text: string) => paint("bold", text);

/** The width the layout should fit into. */
export function width(): number {
  const columns = process.stdout.columns ?? 80;
  return Math.max(60, Math.min(columns, 120));
}

/**
 * The length of the text as the reader sees it.
 *
 * The escape sequences do not occupy space, so they are not counted. A string
 * that is padded by its visible length stays in its column whatever styling is
 * applied to it.
 */
export function visibleLength(text: string): number {
  return text.replace(ESCAPE_SEQUENCE, "").length;
}

/** Pad the text to a width, measured in what the reader sees. */
export function pad(text: string, size: number, align: "left" | "right" = "left"): string {
  const missing = Math.max(0, size - visibleLength(text));
  if (align === "right") {
    return " ".repeat(missing) + text;
  }
  return text + " ".repeat(missing);
}

/** Cut the text to a width, measured in what the reader sees. */
export function truncate(text: string, size: number): string {
  if (visibleLength(text) <= size) {
    return text;
  }
  // The escape sequences are kept: cutting in the middle of one would leave it
  // unterminated and colour the rest of the line.
  const visible = text.replace(ESCAPE_SEQUENCE, "");
  const head = [...visible].slice(0, Math.max(0, size - 1)).join("");
  return `${head}…`;
}

/* ── Structure ─────────────────────────────────────────────────────────── */

/** A section title with a rule under it. */
export function section(title: string): void {
  const rule = "─".repeat(Math.max(0, width() - 2));
  console.log(`\n${heading(title)}\n${muted(rule)}`);
}

/**
 * A panel: a box with a title, holding rows of a label and a value.
 *
 * The label column is as wide as the widest label in the panel, so the values
 * line up under one another instead of each row deciding for itself.
 */
export function panel(title: string, rows: Array<[string, string]>): void {
  const inner = width() - 4;
  const label = rows.reduce((widest, [name]) => Math.max(widest, visibleLength(name)), 0);
  const top = `╭─ ${title} ${"─".repeat(Math.max(0, inner - title.length - 2))}╮`;

  console.log(muted(top));
  for (const [name, value] of rows) {
    console.log(
      `${muted("│")} ${muted(pad(name, label))}  ${pad(value, Math.max(0, inner - label - 2))}${muted("│")}`,
    );
  }
  console.log(muted(`╰${"─".repeat(Math.max(0, inner + 1))}╯`));
}

/** A label and a value, aligned. */
export function field(label: string, value: string | number | null, size = 20): void {
  const rendered = value === null ? muted("—") : String(value);
  console.log(`  ${muted(pad(label, size))} ${rendered}`);
}

/** A field whose value carries a style of its own. */
export function styledField(label: string, value: string, size = 20, style: Style = "bold"): void {
  console.log(`  ${muted(pad(label, size))} ${paint(style, value)}`);
}

/**
 * The lines of a panel, without printing them.
 *
 * A view that redraws itself needs the lines rather than the printing, so the
 * panel and the fields are built the same way either way.
 */
export function panelLines(title: string, rows: Array<[string, string]>): string[] {
  const inner = width() - 4;
  const label = rows.reduce((widest, [name]) => Math.max(widest, visibleLength(name)), 0);
  const top = `╭─ ${title} ${"─".repeat(Math.max(0, inner - title.length - 2))}╮`;

  const lines = [muted(top)];
  for (const [name, value] of rows) {
    lines.push(
      `${muted("│")} ${muted(pad(name, label))}  ${pad(value, Math.max(0, inner - label - 2))}${muted("│")}`,
    );
  }
  lines.push(muted(`╰${"─".repeat(Math.max(0, inner + 1))}╯`));
  return lines;
}

/** The lines of a group of fields, without printing them. */
export function fieldLines(title: string, rows: Array<[string, string]>): string[] {
  const label = rows.reduce((widest, [name]) => Math.max(widest, visibleLength(name)), 0);

  return [
    "",
    bold(title),
    muted("─".repeat(width() - 2)),
    ...rows.map(([name, value]) => `  ${muted(pad(name, label))} ${value}`),
  ];
}

/* ── Tables ───────────────────────────────────────────────────────────── */

/** How a column is aligned and how wide it may grow. */
export interface Column {
  header: string;
  align?: "left" | "right";
  /** The most the column may take. A longer value is cut. */
  max?: number;
}

/** A table with a frame, sized to the terminal. */
export function table(columns: Column[], rows: string[][]): void {
  if (rows.length === 0) {
    console.log(`  ${muted("(nothing to show)")}`);
    return;
  }

  const natural = columns.map((column, index) =>
    Math.max(visibleLength(column.header), ...rows.map((row) => visibleLength(row[index] ?? ""))),
  );

  // The table shares the width with its frame. The columns are given what they
  // need, and the widest one gives up the difference.
  const available = width() - 4 - (columns.length - 1) * 2;
  const total = natural.reduce((sum, size) => sum + size, 0);

  const widths =
    total <= available
      ? natural
      : natural.map((size, index) => {
          const limit = columns[index]?.max ?? size;
          return Math.min(size, limit);
        });

  // Whatever is still too wide comes off the widest column.
  let overflow = widths.reduce((sum, size) => sum + size, 0) - available;
  for (let index = widths.length - 1; index >= 0 && overflow > 0; index -= 1) {
    const limit = columns[index]?.max ?? widths[index] ?? 0;
    const reducible = Math.max(0, (widths[index] ?? 0) - Math.min(limit, 8));
    const taken = Math.min(reducible, overflow);
    widths[index] = (widths[index] ?? 0) - taken;
    overflow -= taken;
  }

  const line = (cells: string[]) =>
    cells
      .map((cell, index) =>
        pad(truncate(cell ?? "", widths[index] ?? 0), widths[index] ?? 0, columns[index]?.align),
      )
      .join("  ");

  console.log(`  ${line(columns.map((column) => bold(column.header)))}`);
  for (const row of rows) {
    console.log(`  ${line(row)}`);
  }
}

/**
 * A view that redraws itself in place.
 *
 * The lines are printed, and on every tick the cursor is moved back over them
 * and they are cleared. A dashboard that is watched does not fill the terminal
 * with a new copy of itself every two seconds.
 */
export function live(render: () => Promise<string[]>): { stop: () => void } {
  let printed = 0;
  let running = false;

  const draw = async () => {
    // A slow request must not overlap with the next tick.
    if (running) {
      return;
    }
    running = true;

    try {
      const lines = await render();

      if (printed > 0) {
        // Back over the previous copy, and clear it.
        process.stdout.write(`\u001b[${printed}A`);
        process.stdout.write("\u001b[0J");
      }

      if (lines.length > 0) {
        process.stdout.write(`${lines.join("\n")}\n`);
      }
      printed = lines.length;
    } finally {
      running = false;
    }
  };

  void draw();
  const timer = setInterval(() => void draw(), 2000);
  timer.unref();

  return {
    stop() {
      clearInterval(timer);
    },
  };
}

/**
 * Machine-readable output.
 *
 * A tool that draws a table is pleasant to read and impossible to parse. The
 * flag makes the same data available as JSON, so the tool can be part of a
 * script without the script scraping the screen.
 */
let json = false;

/** Whether the output is machine-readable. */
export function wantsJson(): boolean {
  return json;
}

/** Turn the machine-readable output on or off. */
export function setJson(value: boolean): void {
  json = value;
}

/** Print the data as JSON, and say whether it was printed. */
export function emit(data: unknown): boolean {
  if (!json) {
    return false;
  }
  process.stdout.write(`${JSON.stringify(data, null, 2)}\n`);
  return true;
}

/* ── Progress ─────────────────────────────────────────────────────────── */

const FRAMES = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/**
 * A spinner for an operation that takes a while.
 *
 * The frames are written over the same line, and the line is cleared when the
 * operation ends, so the output is a single line rather than a hundred of
 * them. Without a terminal there are no frames: the text is printed once, at
 * the end, which is what a log wants.
 */
export function spinner(text: string): {
  update: (message: string) => void;
  stop: (message?: string) => void;
} {
  if (!process.stdout.isTTY) {
    return {
      update: () => {},
      stop: (message?: string) => {
        if (message) {
          console.log(message);
        }
      },
    };
  }

  let frame = 0;
  let current = text;

  const timer = setInterval(() => {
    frame = (frame + 1) % FRAMES.length;
    process.stdout.write(`\r${accent(FRAMES[frame] ?? "")} ${current}`);
  }, 80);
  timer.unref();

  return {
    update(message: string) {
      current = message;
    },
    stop(message?: string) {
      clearInterval(timer);
      if (message) {
        process.stdout.write(`\r${" ".repeat(width() - 1)}\r${message}\n`);
        return;
      }
      process.stdout.write(`\r${" ".repeat(width() - 1)}\r`);
    },
  };
}

/** A progress bar. */
/** Bytes in the unit a reader can picture. */
export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} kB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
  return `${(bytes / 1024 / 1024 / 1024).toFixed(1)} GB`;
}

export function bar(done: number, total: number, size = 24): string {
  if (total <= 0) {
    return muted("─".repeat(size));
  }

  const filled = Math.min(size, Math.round((done / total) * size));
  return `${accent("━".repeat(filled))}${muted("─".repeat(size - filled))}`;
}

/** A small coloured marker for a state. */
export function badge(state: "ok" | "warn" | "err" | "idle"): string {
  switch (state) {
    case "ok":
      return ok("●");
    case "warn":
      return warn("●");
    case "err":
      return err("●");
    default:
      return muted("○");
  }
}
