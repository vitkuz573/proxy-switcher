import type { NextConfig } from "next";

const nextConfig: NextConfig = {
  /*
   * A self-contained server bundle with a pruned dependency tree. The console is
   * deployed as its own release directory, so it must not depend on the
   * monorepo being present next to it.
   */
  output: "standalone",
  cacheComponents: true,
  partialPrefetching: true,
  turbopack: {
    rules: {
      "*.css": {
        loaders: ["@tailwindcss/turbopack"],
        as: "*.css",
      },
    },
  },
};

export default nextConfig;
