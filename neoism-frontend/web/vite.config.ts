import { defineConfig } from "vite";

const daemonProxy = {
  "/ws": {
    target: "ws://127.0.0.1:7878/session",
    ws: true,
    changeOrigin: true,
    rewrite: () => "",
  },
};

export default defineConfig({
  server: {
    port: 5173,
    strictPort: true,
    proxy: daemonProxy,
  },
  preview: {
    port: 5173,
    strictPort: true,
    proxy: daemonProxy,
  },
  build: {
    target: "es2022",
    sourcemap: true,
  },
});
