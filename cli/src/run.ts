import { CliError } from "./client.js";
import { dim, red } from "./format.js";

/**
 * Wraps a command body so every failure exits non-zero with a readable message
 * instead of a stack trace, while debug builds can still see the real cause.
 */
export async function runCli(body: () => Promise<void>): Promise<void> {
  try {
    await body();
  } catch (err) {
    if (err instanceof CliError) {
      console.error(`\n  ${red("✗")} ${err.message}`);
      if (err.hint) {
        console.error(`    ${dim(err.hint)}`);
      }
      process.exitCode = 1;
      return;
    }

    if (err instanceof Error) {
      console.error(`\n  ${red("✗")} ${err.message}`);
      if (process.env.DEBUG) {
        console.error(dim(err.stack ?? ""));
      }
      process.exitCode = 1;
      return;
    }

    console.error(`\n  ${red("✗")} ${String(err)}`);
    process.exitCode = 1;
  }
}
