import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import path from "path";

const apiPort = process.env.VITE_API_PORT || "9001";

// https://vitejs.dev/config/
export default defineConfig({
  base: "./",
  plugins: [react()],
  resolve: {
    alias: {
      // Import TypeScript bindings directly from where ts-rs generates them
      "@generated": path.resolve(
        __dirname,
        "../../../bindings/src/fold_node/static-react/src/types",
      ),
    },
  },
  server: {
    proxy: {
      "/api": {
        target: `http://localhost:${apiPort}`,
        changeOrigin: true,
        secure: false,
      },
      "/ingestion": {
        target: `http://localhost:${apiPort}`,
        changeOrigin: true,
        secure: false,
      },
    },
    watch: {
      ignored: ["**/node_modules/**"],
    },
  },
  build: {
    minify: false,
    sourcemap: true,
  },
});
