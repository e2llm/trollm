import { defineConfig } from "vite";
import { bundleInventoryPlugin } from "./scripts/bundle-inventory.mjs";
export default defineConfig({
  plugins: [bundleInventoryPlugin()],
  server: { proxy: { "/api": "http://127.0.0.1:8080" } },
  build: { chunkSizeWarningLimit: 700 },
});
