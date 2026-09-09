import { defineConfig } from "vite";

// Two entry pages: the frameless widget and the full ledger window.
export default defineConfig({
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  envPrefix: ["VITE_", "TAURI_"],
  build: {
    target: "es2022",
    minify: true,
    sourcemap: false,
    rollupOptions: { input: { main: "index.html", widget: "widget.html" } },
  },
});
