import React from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import "./generated/fonts.css";
import "./style.css";
import "./typography.css";

const global = globalThis as { __NEOISM_DAEMON_HTTP__?: string };
if (!global.__NEOISM_DAEMON_HTTP__) {
    fetch(location.href, { method: "GET", cache: "no-store", credentials: "same-origin" })
        .then(response => {
            const hinted = response.headers.get("x-neoism-daemon-http");
            if (hinted && /^http:\/\/(127\.0\.0\.1|localhost|\[::1\]):\d+$/.test(hinted))
                global.__NEOISM_DAEMON_HTTP__ = hinted;
        })
        .catch(() => { /* Share falls back to :7878. */ });
}

createRoot(document.getElementById("root")!).render(
    <React.StrictMode>
        <App />
    </React.StrictMode>,
);
