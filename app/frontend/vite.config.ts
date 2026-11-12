import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import wails from "@wailsio/runtime/plugins/vite";
import { xtermFontMetrics } from "./xterm-font-metrics.ts";

// https://vitejs.dev/config/
export default defineConfig({
  server: {
    host: "127.0.0.1",
    port: Number(process.env.WAILS_VITE_PORT) || 9245,
    strictPort: true,
  },
  plugins: [xtermFontMetrics(), react(), wails("./bindings")],
  optimizeDeps: {
    rolldownOptions: { plugins: [xtermFontMetrics()] },
  },
});
