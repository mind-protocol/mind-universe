import { defineConfig, configDefaults } from "vitest/config";
import react from "@vitejs/plugin-react";
import universeStream from "./scripts/vite-plugin-universe-stream.mjs";

export default defineConfig({
  plugins: [react(), universeStream()],
  clearScreen: false,
  server: {
    host: "127.0.0.1",
    port: 1420,
    strictPort: true,
    // Allow importing the graph-materialized visual mapping authority from the
    // repo-level fixtures/ directory (single source of truth for the renderer).
    fs: { allow: ["../.."] }
  },
  envPrefix: ["VITE_", "TAURI_"],
  test: {
    // Never scan nested git worktrees (leftover agent scaffolding under .claude/):
    // a stale worktree carries duplicate *.test.ts that reference unmaterialized
    // artifacts and would fail the app's own suite with foreign noise.
    exclude: [...configDefaults.exclude, "**/.claude/**"]
  }
});
