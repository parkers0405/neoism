import { defineConfig, type Plugin } from "vite";
import { localRegistryProxy } from './scripts/localRegistryProxy';
import react from "@vitejs/plugin-react";

/** Public `/fonts` `/syntax` stay root-absolute in source; rewrite built files to relative. */
function relativePublicUrls(): Plugin {
    const rewrite = (text: string) => text
        .replaceAll("url(/fonts/", "url(../fonts/")
        .replaceAll("url(\"/fonts/", "url(\"../fonts/")
        .replaceAll("url('/fonts/", "url('../fonts/")
        .replaceAll("\"/fonts/", "\"../fonts/")
        .replaceAll("'/fonts/", "'../fonts/")
        .replaceAll("\"/syntax/", "\"../syntax/")
        .replaceAll("'/syntax/", "'../syntax/")
        .replaceAll("\"/favicon.svg\"", "\"./favicon.svg\"")
        .replaceAll("\"/assets/", "\"./assets/");
    return {
        name: "relative-public-urls",
        generateBundle(_, bundle) {
            for (const item of Object.values(bundle)) {
                if (item.type === "chunk") item.code = rewrite(item.code);
                else if (typeof item.source === "string") item.source = rewrite(item.source);
            }
        },
    };
}

export default defineConfig({
    plugins: [localRegistryProxy(), react(), relativePublicUrls()],
    // Relative so the same dist works at `/` (Vite/Agent :4096) and `/agent-gui/`.
    base: "./",
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
