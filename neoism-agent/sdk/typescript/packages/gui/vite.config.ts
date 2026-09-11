import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
export default defineConfig({
    plugins: [react()],
    // web-tree-sitter contains conditional dynamic imports; preserve module workers.
    worker: { format: "es" },
    server: { port: 5174 },
    build: {
        rollupOptions: {
            output: {
                manualChunks(id) {
                    if (id.includes("node_modules")) {
                        if (
                            /\/node_modules\/(react|react-dom|scheduler|lucide-react)\//.test(
                                id,
                            )
                        )
                            return "react";
                        return "markdown";
                    }
                },
            },
        },
    },
});
