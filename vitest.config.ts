import { defineConfig, mergeConfig } from "vitest/config";
import viteConfig from "./vite.config";

export default mergeConfig(
  viteConfig,
  defineConfig({
    test: {
      environment: "jsdom",
      globals: true,
      setupFiles: ["./src/vitest-setup.ts"],
      // Vitest's own default excludes don't cover a linked worktree checked
      // out under the repo root (see using-git-worktrees) -- without this,
      // `npm test` from the main checkout also picks up every test file
      // inside .worktrees/<name>/src, each resolving against that
      // worktree's own separate node_modules (a different React install),
      // which surfaces as unrelated `useMemoCache`-style failures.
      exclude: ["**/node_modules/**", "**/.worktrees/**", "**/worktrees/**"],
    },
  })
);
