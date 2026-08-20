import { defineConfig } from "vite";

function daemonWsUrl(): string {
  const injected = process.env.VITE_NEOISM_DAEMON_URL?.trim();
  if (injected) return injected;
  const port = process.env.NEOISM_DAEMON_TCP_PORT?.trim();
  if (port) return `ws://127.0.0.1:${port}/session`;
  return "ws://127.0.0.1:7878/session";
}

const daemonWs = daemonWsUrl();
const daemonProxy = {
  "/ws": {
    target: daemonWs,
    ws: true,
    changeOrigin: true,
    rewrite: () => "",
  },
};

export default defineConfig({
  define: {
    "import.meta.env.VITE_NEOISM_DAEMON_URL": JSON.stringify(
      process.env.VITE_NEOISM_DAEMON_URL ?? daemonWs,
    ),
  },
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
