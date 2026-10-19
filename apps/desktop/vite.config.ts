import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri expects a fixed dev port and no screen clearing. /__tp is only used
// when the UI runs in a plain browser against scripts/dev-bridge.py.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    proxy: { "/__tp": { target: "http://127.0.0.1:1421", changeOrigin: false } },
  },
  build: { target: "safari16", sourcemap: false },
});
