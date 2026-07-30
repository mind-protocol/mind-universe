import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    host: "127.0.0.1",
    port: 1420,
    strictPort: true,
    // Allow importing the graph-materialized visual mapping authority from the
    // repo-level fixtures/ directory (single source of truth for the renderer).
    fs: { allow: ["../.."] }
  },
  envPrefix: ["VITE_", "TAURI_"]
});
